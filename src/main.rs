use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Context};
use clap::Parser;

mod video;

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
        let width = u32::from_str_radix(ws, 10).context("invalid width")?;
        let height = u32::from_str_radix(hs, 10).context("invalid height")?;
        Ok(Self { width, height })
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    size: Option<Size>,

    #[arg(long, default_value = "/dev/video0")]
    device: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    Video::gst_init()?;
    let video = Arc::new(Video::new(&args.device, args.size.map(|s| (s.width, s.height)))?);
    video.start()?;

    let loop_thread = std::thread::spawn({
        let video = video.clone();
        move || { video.event_loop() }
    });

    std::thread::sleep(std::time::Duration::from_secs(10));

    println!("stopping");
    video.stop()?;

    println!("waiting for main loop");
    let result = loop_thread.join();

    println!("all done: {result:?}");
    Ok(())
}
