//! Bounded chunk queue with drop-oldest overflow; `push` is synchronous and never blocks
//! (same rule as `warptalk-pipeline-runtime`'s `FrameQueue`, but chunks here are arbitrary
//! length / rate and carry their timeline position so the consumer can detect gaps).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use tokio::sync::Notify;

use crate::types::PushStatus;

#[derive(Debug, Clone)]
pub struct Chunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Position of the chunk start on the channel's push timeline (ms since channel start).
    pub start_ms: u64,
}

struct Inner {
    q: VecDeque<Chunk>,
    closed: bool,
}

pub struct ChunkQueue {
    inner: Mutex<Inner>,
    notify: Notify,
    capacity: usize,
    dropped: AtomicU64,
}

impl ChunkQueue {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            inner: Mutex::new(Inner {
                q: VecDeque::with_capacity(capacity.min(4096)),
                closed: false,
            }),
            notify: Notify::new(),
            capacity,
            dropped: AtomicU64::new(0),
        }
    }

    /// Never blocks, never awaits.
    pub fn push(&self, chunk: Chunk) -> PushStatus {
        let evicted = {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            if g.closed {
                return PushStatus::Closed;
            }
            let evicted = g.q.len() >= self.capacity;
            if evicted {
                g.q.pop_front();
            }
            g.q.push_back(chunk);
            evicted
        };
        self.notify.notify_one();
        if evicted {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            PushStatus::DroppedOldest
        } else {
            PushStatus::Accepted
        }
    }

    /// Next chunk; `None` once closed and drained.
    pub async fn pop(&self) -> Option<Chunk> {
        loop {
            {
                let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(c) = g.q.pop_front() {
                    return Some(c);
                }
                if g.closed {
                    return None;
                }
            }
            self.notify.notified().await;
        }
    }

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

    fn chunk(i: u64) -> Chunk {
        Chunk {
            samples: vec![0.0; 4],
            sample_rate: 16_000,
            start_ms: i,
        }
    }

    #[test]
    fn push_never_blocks_and_drops_oldest() {
        // No consumer and no runtime: if push could block or await this would hang/not compile.
        let q = ChunkQueue::new(3);
        let mut dropped = 0;
        for i in 0..100 {
            if q.push(chunk(i)) == PushStatus::DroppedOldest {
                dropped += 1;
            }
        }
        assert_eq!(dropped, 97);
        assert_eq!(q.dropped(), 97);
        assert_eq!(q.len(), 3);
    }

    #[tokio::test]
    async fn keeps_newest_in_order_and_drains_after_close() {
        let q = ChunkQueue::new(3);
        for i in 0..10 {
            q.push(chunk(i));
        }
        q.close();
        assert_eq!(q.push(chunk(99)), PushStatus::Closed);
        let mut got = Vec::new();
        while let Some(c) = q.pop().await {
            got.push(c.start_ms);
        }
        assert_eq!(got, vec![7, 8, 9]);
    }
}
