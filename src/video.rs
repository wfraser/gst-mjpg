use std::sync::Arc;

use anyhow::Context;
use futures::StreamExt;
use gstreamer::prelude::*;
use gstreamer::{
    BufferRef, Caps, DebugLevel, Element, ElementFactory, Message, Pipeline, Sample, State,
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
        gstreamer::debug_set_active(true);
        gstreamer::debug_set_colored(true);
        gstreamer::debug_set_default_threshold(DebugLevel::Warning);
        Ok(())
    }

    pub fn new(source: VideoSource, size: Option<(u32, u32)>) -> anyhow::Result<Self> {
        let pipeline = Pipeline::new(Some("pipeline"));

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

        let enc = ElementFactory::make("jpegenc")
            .build()
            .context("failed to make jpegenc")?;

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

        let elts = &[&camera, &enc, appsink.upcast_ref()];
        pipeline
            .add_many(elts)
            .context("failed to add elements to pipeline")?;
        Element::link_many(elts).context("failed to link elements")?;

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
