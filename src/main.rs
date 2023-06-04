#[macro_use]
extern crate log;

use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use clap::Parser;
use gstreamer::prelude::GstObjectExt;
use gstreamer::MessageView;

pub mod frames;
pub mod video;

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

    #[arg(long, default_value = "/dev/video0")]
    device: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    stderrlog::new().module(module_path!()).init().unwrap();
    let args = Args::parse();

    Video::gst_init()?;
    let video = Arc::new(Video::new(
        &args.device,
        args.size.map(|s| (s.width, s.height)),
    )?);

    let error = Arc::new(AtomicBool::new(false));
    let error2 = error.clone();
    tokio::spawn(
        video
            .clone()
            .foreach_message(move |video, msg| match msg.view() {
                MessageView::Eos(..) => {
                    println!("got EOS");
                }
                MessageView::Error(e) => {
                    error2.store(true, SeqCst);
                    println!(
                        "Error from {:?}: {} ({:?})",
                        e.src().map(|s| s.path_string()),
                        e.error(),
                        e.debug(),
                    );
                    video.stop().unwrap();
                }
                _ => (),
            }),
    );

    video.start()?;

    let frames = tokio::spawn(video.clone().foreach_frame(|_video, sample, buf| {
        println!(
            "sample #{}: {} bytes @ {:?}; caps = {:?}",
            buf.offset(),
            buf.size(),
            buf.dts(),
            sample.caps(),
        );
    }));

    tokio::select! {
        _ = frames => {
            println!("frames ended before timeout");
        },
        _ = tokio::time::sleep(Duration::from_secs(5)) => (),
    }
    if error.load(SeqCst) {
        println!("aborting due to error");
        return Ok(());
    }

    println!("stopping");
    video.stop()?;

    println!("waiting 2 secs");
    tokio::time::sleep(Duration::from_secs(2)).await;

    println!("restarting for 1 more sec");
    video.start()?;

    tokio::spawn(video.clone().foreach_frame(|_video, sample, buf| {
        println!(
            "sample #{}: {} bytes @ {:?}; caps = {:?}",
            buf.offset(),
            buf.size(),
            buf.dts(),
            sample.caps(),
        );
    }));

    tokio::time::sleep(Duration::from_secs(1)).await;
    video.stop()?;

    println!("all done");
    Ok(())
}
