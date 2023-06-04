use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

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
    sender: Sender<Bytes>,
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

    pub async fn stream(&self) -> FrameStream<'_> {
        let mut inner = self.inner.lock().await;
        let receiver = if inner.count == 0 {
            self.start(&inner);
            let (sender, receiver) = broadcast::channel(16);
            inner.sender = sender;
            inner.count = 1;
            receiver
        } else {
            inner.count += 1;
            inner.sender.subscribe()
        };
        FrameStream {
            parent: self,
            stream: BroadcastStream::new(receiver),
        }
    }

    fn start(&self, inner: &MutexGuard<'_, FramesInner>) {
        if let Err(e) = self.video.start() {
            error!("error starting video: {e}");
            return;
        }
        let sender = inner.sender.clone();
        tokio::spawn(
            self.video
                .clone()
                .foreach_frame(move |_video, _sample, buf| {
                    let mut bytes = BytesMut::new();
                    for mem in buf.iter_memories() {
                        bytes.extend_from_slice(mem.map_readable().unwrap().as_slice());
                    }
                    if let Err(e) = sender.send(bytes.freeze()) {
                        error!("failed to broadcast frame: {e}");
                    }
                }),
        );
    }

    pub fn stop(&self) {
        let mut inner = self.inner.blocking_lock();
        inner.count = inner.count.saturating_sub(1);
        if inner.count != 0 {
            return;
        }
        if let Err(e) = self.video.stop() {
            error!("error stopping video: {e}");
        }
    }
}

pub struct FrameStream<'a> {
    parent: &'a Frames,
    stream: BroadcastStream<Bytes>,
}

impl<'a> Drop for FrameStream<'a> {
    fn drop(&mut self) {
        self.parent.stop();
    }
}

impl<'a> Stream for FrameStream<'a> {
    type Item = Bytes;
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let stream = Pin::new(&mut self.stream);
        match stream.poll_next(cx) {
            Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(frame)),
            Poll::Ready(Some(Err(lag))) => {
                warn!("lag: {lag}");
                self.poll_next(cx)
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
