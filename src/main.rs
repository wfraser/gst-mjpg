#[macro_use]
extern crate log;

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Context};
use clap::Parser;
use futures::StreamExt;
use gstreamer::MessageView;
use gstreamer::glib::uuid_string_random;
use gstreamer::prelude::GstObjectExt;
use hyper::http::HeaderValue;
use hyper::{Request, Response, Body, Server, HeaderMap};
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use multipart_stream::Part;

pub mod frames;
pub mod video;

use crate::frames::Frames;
use crate::video::Video;

#[derive(Debug, Clone)]
struct Size {
    width: u32,
    height: u32,
}

impl FromStr for Size {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (ws, hs) = match s.split_once('x') {
            Some(v) => v,
            None => bail!("size must be WIDTHxHEIGHT; missing 'x' char"),
        };
        let width = ws.parse::<u32>().context("invalid width")?;
        let height = hs.parse::<u32>().context("invalid height")?;
        Ok(Self { width, height })
    }
}

#[derive(Debug, Parser)]
struct Args {
    /// WIDTHxHEIGHT. If unspecified, use whatever the camera's native resolution is.
    #[arg(long)]
    size: Option<Size>,

    /// Video device to open.
    #[arg(long, default_value = "/dev/video0")]
    device: String,

    /// TCP port to listen on for HTTP server.
    #[arg(long, default_value = "5001")]
    port: u16,

    /// Verbose output. Specify multiple times to increase level.
    /// 0x = Error/Warning, 1x = Info, 2x = Debug, 3x = Trace.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

async fn handle_request(req: Request<Body>, _remote: SocketAddr, frames: Arc<Frames>) -> anyhow::Result<Response<Body>> {
    match req.uri().path() {
        "/" => return Ok(index()?),
        "/stream" => (),
        other => return Ok(Response::builder()
            .status(404)
            .body(format!("nothing configured for the path {other:?}").into())?),
    }

    let bdry = uuid_string_random();
    let stream = frames.stream().await;
    let parts = stream.map(|buf| {
        let mut headers = HeaderMap::new();
        headers.append("Content-Type", HeaderValue::from_static("image/jpeg"));
        Ok::<_, Infallible>(Part {
            headers,
            body: buf,
        })
    });
    let body = Body::wrap_stream(multipart_stream::serialize(parts, bdry.as_str()));
    Ok(Response::new(body))
}

fn index() -> anyhow::Result<Response<Body>> {
    Ok(Response::builder()
        .header("Content-Type", "text/html")
        .body(format!("<html><body><h1><code>gst-mjpg</code></h1>
            <p><a href=\"/stream\">start stream</a>
            <address>gst-mjpg/v{}", env!("CARGO_PKG_VERSION")).into())
        .context("failed to build index response")?)
}

fn server_error(e: anyhow::Error) -> Result<Response<Body>, Infallible> {
    Ok(Response::builder()
        .status(500)
        .header("Content-Type", "text/plain")
        .body(format!("server error: {e}").into())
        .unwrap())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    stderrlog::new().module(module_path!()).verbosity(args.verbose as usize + 1).init().unwrap();

    Video::gst_init()?;
    let video = Arc::new(Video::new(
        &args.device,
        args.size.map(|s| (s.width, s.height)),
    )?);

    tokio::spawn(
        video
            .clone()
            .foreach_message(move |_video, msg| match msg.view() {
                MessageView::Eos(..) => {
                    error!("got EOS from video");
                }
                MessageView::Error(e) => {
                    error!("Error from {:?}: {} ({:?})",
                        e.src().map(|s| s.path_string()),
                        e.error(),
                        e.debug(),
                    );
                }
                _ => (),
            })
    );

    let frames = Arc::new(Frames::new(video));

    let make_svc = make_service_fn(move |conn: &AddrStream| {
        let remote = conn.remote_addr();
        let frames = frames.clone();
        async move {
            Ok::<_, Infallible>(service_fn(move |req: Request<Body>| {
                info!("HTTP request from {} ({:?}): {} {}",
                    remote, req.headers().get("user-agent").unwrap_or(&HeaderValue::from_static("<no useragent>")), req.method(), req.uri());
                let frames = frames.clone();
                async move {
                    let mut resp = handle_request(req, remote, frames).await
                        .or_else(server_error)
                        .unwrap();
                    let hdrs = resp.headers_mut();
                    hdrs.insert("Server", HeaderValue::from_str(&format!("gst-mjpg/v{}", env!("CARGO_PKG_VERSION"))).unwrap());
                    hdrs.insert("Cache-Control", HeaderValue::from_static("no-store, no-cache, must-revalidate, pre-check=0, post-check=0, max-age=0"));
                    hdrs.insert("Pragma", HeaderValue::from_static("no-cache"));
                    hdrs.insert("Expires", HeaderValue::from_static("Mon, 3 Jan 2000 12:34:56 GMT"));
                    Ok::<_, Infallible>(resp)
                }
            }))
        }
    });


    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, args.port));
    Server::bind(&addr).serve(make_svc).await?;

    Ok(())
}
