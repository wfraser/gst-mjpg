use anyhow::Context;
use gstreamer::event::Eos;
use gstreamer::prelude::*;
use gstreamer::{
    Caps,
    ClockTime,
    DebugLevel,
    Element,
    ElementFactory,
    FlowError,
    FlowSuccess,
    MessageView,
    Pipeline,
    State,
};
use gstreamer_app::{
    AppSink,
    AppSinkCallbacks,
};

pub struct Video {
    pipeline: Pipeline,
}

impl Video {
    pub fn gst_init() -> anyhow::Result<()> {
        gstreamer::init().context("failed to init gstreamer")?;
        gstreamer::debug_set_active(true);
        gstreamer::debug_set_colored(true);
        gstreamer::debug_set_default_threshold(DebugLevel::Warning);
        Ok(())
    }

    pub fn new(device: &str, size: Option<(u32, u32)>) -> anyhow::Result<Self> {
        let pipeline = Pipeline::new(Some("pipeline"));

        let camera = ElementFactory::make("v4l2src")
            .name("camera")
            .property_from_str("device", device)
            .build()
            .context("failed to make v4l2src")?;

        let enc = ElementFactory::make("jpegenc").build().context("failed to make jpegenc")?;

        let sink_caps = {
            let mut b = Caps::builder("image/jpeg");
            if let Some((w, h)) = size {
                b = b
                    .field("width", i32::try_from(w).context("width out of range")?)
                    .field("height", i32::try_from(h).context("height out of range")?);
            }
            b.build()
        };

        let appsink = AppSink::builder()
            .caps(&sink_caps)
            .name("appsink")
            .build();

        let callbacks = AppSinkCallbacks::builder()
            .new_sample(|sink| {
                let sample = match sink.pull_sample() {
                    Ok(sample) => sample,
                    Err(e) => {
                        println!("failed to pull sample: {e}");
                        return Err(FlowError::Error);
                    }
                };

                let buf = match sample.buffer() {
                    Some(buf) => buf,
                    None => {
                        println!("sample has no buffer: {sample:?}");
                        return Err(FlowError::Error);
                    }
                };

                println!("sample #{}: {} bytes @ {:?}; caps = {:?}",
                    buf.offset(),
                    buf.size(),
                    buf.dts(),
                    sample.caps(),
                );

                // TODO: do something with it

                Ok(FlowSuccess::Ok)
            })
            .build();

        appsink.set_callbacks(callbacks);

        let elts = &[&camera, &enc, appsink.upcast_ref()];
        pipeline.add_many(elts).context("failed to add elements to pipeline")?;
        Element::link_many(elts).context("failed to link elements")?;

        Ok(Self { pipeline })
    }

    pub fn start(&self) -> anyhow::Result<()> {
        self.pipeline.set_state(State::Playing)
            .context("failed to set pipeline to Playing state")?;
        Ok(())
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        self.pipeline.send_event(Eos::new());
        Ok(())
    }

    pub fn event_loop(&self) -> anyhow::Result<()> {
        let bus = self.pipeline.bus().unwrap();
        for msg in bus.iter_timed(ClockTime::NONE) {
            match msg.view() {
                MessageView::Eos(..) => {
                    println!("got EOS");
                    break;
                }
                MessageView::Error(e) => {
                    println!("Error from {:?}: {} ({:?})",
                        e.src().map(|s| s.path_string()),
                        e.error(),
                        e.debug(),
                    );
                    break;
                }
                _ => (),
            }
        }
        self.pipeline.set_state(State::Null)
            .context("failed to set pipeline to Null state")?;
        Ok(())
    }
}