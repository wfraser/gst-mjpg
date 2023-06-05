use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::Context;
use futures::StreamExt;
use gstreamer::glib::uuid_string_random;
use hyper::http::HeaderValue;
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, HeaderMap, Request, Response, Server};
use multipart_stream::Part;

use crate::frames::Frames;

async fn handle_request(
    req: Request<Body>,
    _remote: SocketAddr,
    stream_path: String,
    frames: Arc<Frames>,
) -> anyhow::Result<Response<Body>> {
    match req.uri().path_and_query().map(|pq| pq.as_str()).unwrap_or("") {
        "/" => return index(),
        path if path == stream_path => (),
        other => {
            return Ok(Response::builder()
                .status(404)
                .body(format!("nothing configured for the path {other:?}").into())?)
        }
    }

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
        Ok::<_, Infallible>(Part { headers, body: buf })
    });
    let body = Body::wrap_stream(multipart_stream::serialize(parts, bdry.as_str()));
    let mut resp = Response::new(body);
    resp.headers_mut().insert("Content-Type", HeaderValue::from_str(&format!("multipart/x-mixed-replace;boundary={}", bdry.as_str())).unwrap());
    Ok(resp)
}

fn index() -> anyhow::Result<Response<Body>> {
    Response::builder()
        .header("Content-Type", "text/html")
        .body(
            format!(
                "<html><body><h1><code>gst-mjpg</code></h1>
            <p><a href=\"/stream\">start stream</a>
            <address>gst-mjpg/v{}",
                env!("CARGO_PKG_VERSION")
            )
            .into(),
        )
        .context("failed to build index response")
}

fn server_error(e: anyhow::Error) -> Result<Response<Body>, Infallible> {
    Ok(Response::builder()
        .status(500)
        .header("Content-Type", "text/plain")
        .body(format!("server error: {e}").into())
        .unwrap())
}

pub async fn serve(port: u16, path: String, frames: Arc<Frames>) -> Result<(), hyper::Error> {
    let make_svc = make_service_fn(move |conn: &AddrStream| {
        let remote = conn.remote_addr();
        let path = path.clone();
        let frames = frames.clone();
        async move {
            Ok::<_, Infallible>(service_fn(move |req: Request<Body>| {
                info!(
                    "HTTP request from {} ({:?}): {} {}",
                    remote,
                    req.headers()
                        .get("user-agent")
                        .unwrap_or(&HeaderValue::from_static("<no useragent>")),
                    req.method(),
                    req.uri()
                );
                let frames = frames.clone();
                let path = path.clone();
                async move {
                    let mut resp = handle_request(req, remote, path, frames)
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
        }
    });

    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    Server::bind(&addr).serve(make_svc).await?;

    Ok(())
}
