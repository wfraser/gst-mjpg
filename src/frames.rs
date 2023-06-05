use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures::Stream;
use tokio::sync::broadcast::{self, Sender};
use tokio::sync::{Mutex, MutexGuard};
use tokio_stream::wrappers::BroadcastStream;

use crate::video::Video;

pub struct Frames {
    video: Arc<Video>,
    inner: Mutex<FramesInner>,
}

struct FramesInner {
    count: u64,
    sender: Sender<(Bytes, Option<Duration>)>,
}

impl Frames {
    pub fn new(video: Arc<Video>) -> Self {
        let (sender, _) = broadcast::channel(16);
        let inner = FramesInner { count: 0, sender };
        Self {
            video,
            inner: Mutex::new(inner),
        }
    }

    pub async fn stream(self: Arc<Self>) -> FrameStream {
        debug!("new streamer");
        let mut inner = self.inner.lock().await;
        let receiver = if inner.count == 0 {
            info!("first streamer");
            let (sender, receiver) = broadcast::channel(16);
            inner.sender = sender;
            inner.count = 1;
            self.start(&inner);
            receiver
        } else {
            debug!("{} previous streams; subscribing", inner.count);
            inner.count += 1;
            inner.sender.subscribe()
        };
        FrameStream {
            parent: self.clone(),
            stream: BroadcastStream::new(receiver),
        }
    }

    fn start(&self, inner: &MutexGuard<'_, FramesInner>) {
        info!("starting video");
        if let Err(e) = self.video.start() {
            error!("error starting video: {e}");
            return;
        }
        let sender = inner.sender.clone();
        tokio::spawn(
            self.video
                .clone()
                .foreach_frame(move |_video, _sample, buf| {
                    debug!("frame {}", buf.offset());
                    let mut bytes = BytesMut::new();
                    for mem in buf.iter_memories() {
                        bytes.extend_from_slice(mem.map_readable().unwrap().as_slice());
                    }
                    let ts = match buf.dts().map(Duration::try_from) {
                        Some(Ok(dur)) => Some(dur),
                        _ => None,
                    };
                    if let Err(e) = sender.send((bytes.freeze(), ts)) {
                        error!("failed to broadcast frame: {e}");
                    }
                }),
        );
    }

    pub async fn stop(&self) {
        let mut inner = self.inner.lock().await;
        inner.count = inner.count.saturating_sub(1);
        if inner.count != 0 {
            debug!("have {} streamers still", inner.count);
            return;
        }
        info!("last streamer went away; stopping video");
        if let Err(e) = self.video.stop() {
            error!("error stopping video: {e}");
        }
    }
}

pub struct FrameStream {
    parent: Arc<Frames>,
    stream: BroadcastStream<(Bytes, Option<Duration>)>,
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        debug!("FrameStream dropped");
        let video = self.parent.clone();
        tokio::spawn(async move { video.stop().await });
    }
}

impl Stream for FrameStream {
    type Item = (Bytes, Option<Duration>);
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let stream = Pin::new(&mut self.stream);
        match stream.poll_next(cx) {
            Poll::Ready(Some(Ok(stuff))) => Poll::Ready(Some(stuff)),
            Poll::Ready(Some(Err(lag))) => {
                warn!("lag: {lag}");
                self.poll_next(cx)
            }
            Poll::Ready(None) => {
                warn!("FrameStream returned none");
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
