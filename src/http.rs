use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::Context as _;
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use gstreamer::glib::uuid_string_random;
use http_body_util::{BodyExt, Full, StreamBody};
use http_body_util::combinators::BoxBody;
use hyper::body::{Frame, Incoming};
use hyper::http::HeaderValue;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Request, Response};
use hyper_util::rt::tokio::TokioIo;
use multipart_stream::Part;
use tokio::net::TcpListener;

use crate::frames::Frames;

type Body = BoxBody<Bytes, hyper::Error>;

fn body(b: impl Into<Bytes>) -> Body {
    BoxBody::new(Full::new(b.into()).map_err(|_| unreachable!()))
}

/// Configurable paths to the HTTP server's endpoints.
#[derive(Debug, Clone)]
pub struct Paths {
    pub stream: String,
    pub snapshot: String,
}

async fn handle_request(
    req: Request<Incoming>,
    _remote: SocketAddr,
    paths: Arc<Paths>,
    frames: Arc<Frames>,
) -> anyhow::Result<Response<Body>> {
    // use path+query so that we can emulate mjpg-streamer's `/?action=stream` endpoint.
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("");
    if path == "/" {
        index(&paths)
    } else if path == paths.stream {
        handle_stream(frames).await
    } else if path == paths.snapshot {
        handle_snapshot(frames).await
    } else {
        Ok(Response::builder()
            .status(404)
            .body(body(format!("nothing configured for the path {path:?}")))?)
    }
}

async fn handle_stream(frames: Arc<Frames>) -> anyhow::Result<Response<Body>> {
    let bdry = uuid_string_random();
    let stream = frames.stream().await;
    let parts = stream.map(|(buf, ts)| {
        let mut headers = HeaderMap::new();
        headers.append("Content-Type", HeaderValue::from_static("image/jpeg"));
        if let Some(ts) = ts {
            headers.append(
                "X-Timestamp",
                HeaderValue::from_str(&format!("{}.{:.06}", ts.as_secs(), ts.subsec_micros()))
                    .unwrap(),
            );
        }
        Ok::<_, hyper::Error>(Part { headers, body: buf })
    });
    let http_frames = multipart_stream::serializer::serialize(parts, bdry.as_str())
        .map_ok(Frame::data);
    let body = BoxBody::new(StreamBody::new(http_frames));
    let mut resp = Response::new(body);
    resp.headers_mut().insert(
        "Content-Type",
        HeaderValue::from_str(&format!(
            "multipart/x-mixed-replace;boundary={}",
            bdry.as_str()
        ))
        .unwrap(),
    );
    Ok(resp)
}

async fn handle_snapshot(frames: Arc<Frames>) -> anyhow::Result<Response<Body>> {
    let (frame, _ts) = match frames.stream().await.next().await {
        Some(frame) => frame,
        None => {
            return server_error(anyhow::anyhow!("no frames from video source")).map_err(Into::into)
        }
    };
    Response::builder()
        .header("Content-Type", "image/jpeg")
        .body(body(frame))
        .context("failed to make snapshot response")
}

fn index(paths: &Paths) -> anyhow::Result<Response<Body>> {
    Response::builder()
        .header("Content-Type", "text/html")
        .body(body(format!(
            "<html><body><h1><code>gst-mjpg</code></h1>\
            <p><a href=\"{}\">start stream</a>\
            <p><a href=\"{}\">get snapshot</a>\
            <address>gst-mjpg/v{}",
            paths.stream,
            paths.snapshot,
            env!("CARGO_PKG_VERSION")
        )))
        .context("failed to build index response")
}

fn server_error(e: anyhow::Error) -> Result<Response<Body>, Infallible> {
    Ok(Response::builder()
        .status(500)
        .header("Content-Type", "text/plain")
        .body(body(format!("server error: {e}")))
        .unwrap())
}

/// Start serving HTTP requests. Does not finish unless the server fails.
pub async fn serve(port: u16, paths: Arc<Paths>, frames: Arc<Frames>) -> anyhow::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let listener = TcpListener::bind(addr).await?;

    loop {
        let Ok((tcp, remote)) = listener.accept()
            .await
            .inspect_err(|e| error!("Failed to accept connection: {e:#}"))
        else {
            continue;
        };

        let io = TokioIo::new(tcp);

        let paths = Arc::clone(&paths);
        let frames = Arc::clone(&frames);

        tokio::task::spawn(async move {
            if let Err(e) = http1::Builder::new()
                .serve_connection(io, service_fn(move |req| {
                    info!(
                        "HTTP request from {} ({:?}): {} {}",
                        remote,
                        req.headers()
                            .get("user-agent")
                            .unwrap_or(&HeaderValue::from_static("<no useragent>")),
                        req.method(),
                        req.uri()
                    );
                    let frames = Arc::clone(&frames);
                    let paths = Arc::clone(&paths);
                    async move {
                        let mut resp = handle_request(req, remote, paths, frames)
                            .await
                            .or_else(server_error)
                            .unwrap();
                        let hdrs = resp.headers_mut();
                        hdrs.insert(
                            "Server",
                            HeaderValue::from_str(&format!("gst-mjpg/v{}", env!("CARGO_PKG_VERSION")))
                                .unwrap(),
                        );
                        hdrs.insert("Cache-Control", HeaderValue::from_static("no-store, no-cache, must-revalidate, pre-check=0, post-check=0, max-age=0"));
                        hdrs.insert("Pragma", HeaderValue::from_static("no-cache"));
                        hdrs.insert(
                            "Expires",
                            HeaderValue::from_static("Mon, 3 Jan 2000 12:34:56 GMT"),
                        );
                        Ok::<_, Infallible>(resp)
                    }
                }))
                .await
            {
                error!("Error serving connection: {e:#}");
            }
        });
    }
}
