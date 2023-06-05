#[macro_use]
extern crate log;

use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Context};
use clap::Parser;
use gstreamer::prelude::GstObjectExt;
use gstreamer::MessageView;
use video::VideoSource;

pub mod frames;
pub mod http;
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

    /// Stream from a fake video source instead of opening a real video device.
    ///
    /// Optional argument is the pattern to show. See `gst-inspect-1.0 testvideosrc` (property "pattern") for options.
    #[arg(long, default_missing_value = "smpte", num_args(0..=1))]
    test_video: Option<String>,

    /// URL path to use for the stream.
    #[arg(long, default_value = "/stream")]
    stream_path: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.verbose > 0 {
        dbg!(&args);
    }
    stderrlog::new()
        .module(module_path!())
        .verbosity(args.verbose as usize + 1)
        .init()
        .unwrap();

    Video::gst_init()?;
    let video = Arc::new(Video::new(
        args.test_video
            .map(VideoSource::Test)
            .unwrap_or_else(|| VideoSource::V4L(args.device.clone())),
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
                    error!(
                        "Error from {:?}: {} ({:?})",
                        e.src().map(|s| s.path_string()),
                        e.error(),
                        e.debug(),
                    );
                }
                _ => (),
            }),
    );

    let frames = Arc::new(Frames::new(video));
    http::serve(args.port, args.stream_path.clone(), frames).await?;

    Ok(())
}
