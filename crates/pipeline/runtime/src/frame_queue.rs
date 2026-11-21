//! Bounded frame queue with **drop-oldest** overflow and a synchronous, never-blocking push.
//!
//! A tokio `mpsc` cannot do this (a full channel either blocks or drops the *newest*). For live
//! audio the newest frame is the one that matters: if the speaker task stalls (slow STT socket)
//! we would rather lose stale audio than add seconds of delay or push back on the source.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::Notify;

use crate::messages::AudioFrame;

pub const DROPPED_FRAMES_METRIC: &str = "warptalk_pipeline_frames_dropped_total";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Accepted,
    /// Accepted after evicting the oldest frame.
    DroppedOldest,
    /// Queue closed; frame discarded.
    Closed,
}

#[derive(Debug)]
struct Inner {
    q: VecDeque<AudioFrame>,
    closed: bool,
}

#[derive(Debug)]
pub struct FrameQueue {
    inner: Mutex<Inner>,
    notify: Notify,
    capacity: usize,
    dropped: AtomicU64,
}

impl FrameQueue {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            inner: Mutex::new(Inner {
                q: VecDeque::with_capacity(capacity),
                closed: false,
            }),
            notify: Notify::new(),
            capacity,
            dropped: AtomicU64::new(0),
        }
    }

    /// Never blocks and never awaits.
    pub fn push(&self, frame: AudioFrame) -> PushOutcome {
        let outcome = {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            if g.closed {
                return PushOutcome::Closed;
            }
            let evicted = g.q.len() >= self.capacity;
            if evicted {
                g.q.pop_front();
            }
            g.q.push_back(frame);
            if evicted {
                PushOutcome::DroppedOldest
            } else {
                PushOutcome::Accepted
            }
        };
        if outcome == PushOutcome::DroppedOldest {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            metrics::counter!(DROPPED_FRAMES_METRIC).increment(1);
        }
        self.notify.notify_one();
        outcome
    }

    /// Next frame; `None` once the queue is closed **and** drained.
    pub async fn pop(&self) -> Option<AudioFrame> {
        loop {
            {
                let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(f) = g.q.pop_front() {
                    return Some(f);
                }
                if g.closed {
                    return None;
                }
            }
            self.notify.notified().await;
        }
    }

    /// No more input; the consumer still drains what is queued.
    pub fn close(&self) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
        self.notify.notify_one();
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .q
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use warptalk_audio::Frame;

    fn frame(v: f32) -> AudioFrame {
        AudioFrame::now(Frame::new(vec![v; 4], 16_000))
    }

    #[tokio::test]
    async fn overflow_drops_oldest_and_counts() {
        let q = FrameQueue::new(3);
        let outcomes: Vec<_> = (0..6).map(|i| q.push(frame(i as f32))).collect();
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| **o == PushOutcome::DroppedOldest)
                .count(),
            3
        );
        assert_eq!(q.dropped(), 3);
        q.close();
        let mut got = Vec::new();
        while let Some(f) = q.pop().await {
            got.push(f.frame.samples[0]);
        }
        assert_eq!(got, vec![3.0, 4.0, 5.0]);
        assert_eq!(q.push(frame(9.0)), PushOutcome::Closed);
    }
}
