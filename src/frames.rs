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

/// If a consumer is slow to pull frames from a `FrameStream`, we'll buffer up to this many frames,
/// but then begin discarding old frames.
const MAX_BUFFERED_FRAMES: usize = 16;

/// Capture frames from a video source for multiple consumers.
pub struct Frames {
    video: Arc<Video>,
    inner: Mutex<FramesInner>,
}

struct FramesInner {
    count: u64,
    sender: Sender<(Bytes, Option<Duration>)>,
}

impl Frames {
    /// Create a new instance from the given video source.
    pub fn new(video: Arc<Video>) -> Self {
        // placeholder sender until someone calls stream()
        let (sender, _) = broadcast::channel(MAX_BUFFERED_FRAMES);
        let inner = FramesInner { count: 0, sender };
        Self {
            video,
            inner: Mutex::new(inner),
        }
    }

    /// Subscribe to frames. If no other subscribers currently exist, this will start the video.
    pub async fn stream(self: Arc<Self>) -> FrameStream {
        debug!("new streamer");
        let mut inner = self.inner.lock().await;
        let receiver = if inner.count == 0 {
            info!("first streamer");
            let (sender, receiver) = broadcast::channel(MAX_BUFFERED_FRAMES);
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

    /// Start the underlying video source and begin capturing and broadcasting frames to all
    /// subscribers.
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
                    // We have to copy the BufferRef into a Bytes because that's what Hyper will
                    // eventually need.
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

    /// Call when a subscriber is dropped. If this makes the number of subscribers zero, this stops
    /// the underlying video source.
    async fn subscriber_stopped(&self) {
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

/// A stream of video frames and their timestamps.
pub struct FrameStream {
    parent: Arc<Frames>,
    stream: BroadcastStream<(Bytes, Option<Duration>)>,
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        debug!("FrameStream dropped");
        let video = self.parent.clone();
        tokio::spawn(async move { video.subscriber_stopped().await });
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
