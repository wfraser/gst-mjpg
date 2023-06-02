use clap::Parser;
use gstreamer::prelude::*;
use gstreamer::{
    Caps,
    ClockTime,
    Element,
    ElementFactory,
    FlowSuccess,
    MessageView,
    Pipeline,
    State,
};
use gstreamer_app::{
    AppSink,
    AppSinkCallbacks,
};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    width: Option<u32>,

    #[arg(long)]
    height: Option<u32>,

    #[arg(long, default_value = "/dev/video0")]
    device: String,
}

fn main() {
    let args = Args::parse();
    gstreamer::init().expect("failed to init gstreamer");

    let sink_caps = {
        let mut b = Caps::builder("image/jpeg");
        if let Some(w) = args.width {
            b = b.field("width", i32::try_from(w).expect("width out of range"));
        }
        if let Some(h) = args.height {
            b = b.field("height", i32::try_from(h).expect("height out of range"));
        }
        b.build()
    };
    println!("{sink_caps:?}");

    let appsink = AppSink::builder()
        .caps(&sink_caps)
        .name("app_sink")
        .build();

    appsink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(|sink| {
                println!("notified of sample");

                let sample = match sink.pull_sample() {
                    Ok(sample) => sample,
                    Err(e) => {
                        println!("failed to pull sample: {e}");
                        return Ok(FlowSuccess::Ok);
                    }
                };

                println!("got a sample: ({:?} bytes), {:?}",
                    sample.buffer().map(|b| b.size()),
                    sample.caps(),
                );

                // TODO: do something with the frame

                Ok(FlowSuccess::Ok)
            })
            .build(),
    );

    let input = ElementFactory::make("v4l2src")
        .name("camera")
        .property_from_str("device", &args.device)
        .build()
        .unwrap();

    let jpegenc = ElementFactory::make("jpegenc").build().unwrap();

    let pipeline = Pipeline::new(Some("my-pipeline"));
    pipeline
        .add_many(&[
            &input,
            &jpegenc,
            appsink.upcast_ref(),
        ])
        .unwrap();

    Element::link_many(
        &[
            &input,
            &jpegenc,
            appsink.upcast_ref(),
        ])
        .unwrap();

    pipeline
        .set_state(State::Playing)
        .expect("failed to set pipeline to Playing state");
    println!("set to playing");

    let bus = pipeline.bus().unwrap();
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

    pipeline.set_state(State::Null)
        .expect("failed to set the pipeline to Null state");
}
