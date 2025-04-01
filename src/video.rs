use std::sync::Arc;

use anyhow::Context;
use futures::StreamExt;
use gstreamer::prelude::*;
use gstreamer::{
    Bin, BufferRef, Caps, DebugLevel, Element, ElementFactory, Message, Pipeline, Sample, State,
};
use gstreamer_app::AppSink;

#[derive(Debug, Clone)]
pub enum VideoSource {
    V4L(String),
    Test(String),
}

pub struct Video {
    pipeline: Pipeline,
    appsink: AppSink,
}

impl Video {
    pub fn gst_init() -> anyhow::Result<()> {
        gstreamer::init().context("failed to init gstreamer")?;
        gstreamer::log::set_active(true);
        gstreamer::log::set_colored(true);
        gstreamer::log::set_default_threshold(DebugLevel::Warning);
        Ok(())
    }

    pub fn new(source: VideoSource, size: Option<(u32, u32)>, filter: Option<&str>) -> anyhow::Result<Self> {
        let pipeline = Pipeline::with_name("pipeline");
        let mut elts = vec![];

        let camera = match source {
            VideoSource::V4L(device) => ElementFactory::make("v4l2src")
                .name("camera")
                .property_from_str("device", &device)
                .build()
                .context("failed to make v4l2src")?,
            VideoSource::Test(pattern) => ElementFactory::make("videotestsrc")
                .name("camera")
                .property_from_str("pattern", &pattern)
                .build()
                .context("failed to make videotestsrc")?,
        };
        elts.push(&camera);

        let filt: Bin;
        if let Some(desc) = filter {
            filt = gstreamer::parse::bin_from_description(desc, true)
                .with_context(|| format!("failed to create elements described by {desc:?}"))?;
            elts.push(filt.upcast_ref());
        }

        let enc = ElementFactory::make("jpegenc")
            .build()
            .context("failed to make jpegenc")?;
        elts.push(&enc);

        let sink_caps = {
            let mut b = Caps::builder("image/jpeg");
            if let Some((w, h)) = size {
                b = b
                    .field("width", i32::try_from(w).context("width out of range")?)
                    .field("height", i32::try_from(h).context("height out of range")?);
            }
            b.build()
        };

        let appsink = AppSink::builder().caps(&sink_caps).name("appsink").build();
        elts.push(appsink.upcast_ref());

        pipeline
            .add_many(&elts)
            .context("failed to add elements to pipeline")?;
        Element::link_many(&elts).context("failed to link elements")?;

        Ok(Self { pipeline, appsink })
    }

    pub async fn foreach_frame(self: Arc<Self>, f: impl Fn(&Video, &Sample, &BufferRef)) {
        while let Some(sample) = self.appsink.stream().next().await {
            let buf = match sample.buffer() {
                Some(buf) => buf,
                None => {
                    println!("sample has no buffer: {sample:?}");
                    continue;
                }
            };

            f(self.as_ref(), &sample, buf);
        }
        println!("no more frames");
    }

    pub async fn foreach_message(self: Arc<Self>, f: impl Fn(&Video, Message)) {
        let bus = self.pipeline.bus().unwrap();
        while let Some(msg) = bus.stream().next().await {
            f(self.as_ref(), msg);
        }
    }

    pub fn start(&self) -> anyhow::Result<()> {
        self.pipeline
            .set_state(State::Playing)
            .context("failed to set pipeline to Playing state")?;
        Ok(())
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        //self.pipeline.send_event(gstreamer::event::Eos::new());
        self.pipeline
            .set_state(State::Null)
            .context("failed to set pipeline to Null state")?;
        Ok(())
    }
}
