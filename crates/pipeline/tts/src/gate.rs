//! Concurrency gate for vendor generations (spec 08 item 10, spec 13 section 3).
//!
//! The vendor plan allows N concurrent generations and QUEUES the excess silently (no 429, only
//! latency), and an open idle context counts like a generating one. So the gate is held for a
//! context's whole life (first push until its end is confirmed), never across playout, and its
//! wait is reported as `tts_slot_wait` (zeros included). The limit is hot-resizable.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

pub struct SlotGate {
    state: Mutex<(usize, usize)>, // (limit, in_use)
    notify: Notify,
}

pub struct SlotPermit {
    gate: Arc<SlotGate>,
    pub waited: Duration,
}

impl SlotGate {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((limit.max(1), 0)),
            notify: Notify::new(),
        })
    }

    pub fn set_limit(&self, limit: usize) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).0 = limit.max(1);
        self.notify.notify_waiters();
    }

    pub fn in_use(&self) -> usize {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).1
    }

    pub async fn acquire(self: &Arc<Self>) -> SlotPermit {
        let started = Instant::now();
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                if s.1 < s.0 {
                    s.1 += 1;
                    return SlotPermit {
                        gate: self.clone(),
                        waited: started.elapsed(),
                    };
                }
            }
            notified.await;
        }
    }
}

impl Drop for SlotPermit {
    fn drop(&mut self) {
        let mut s = self
            .gate
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        s.1 = s.1.saturating_sub(1);
        drop(s);
        self.gate.notify.notify_waiters();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limits_and_releases_and_resizes() {
        let g = SlotGate::new(1);
        let p1 = g.acquire().await;
        assert_eq!(g.in_use(), 1);
        let g2 = g.clone();
        let w = tokio::spawn(async move { g2.acquire().await.waited });
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!w.is_finished());
        g.set_limit(2); // hot resize wakes the waiter
        let waited = w.await.unwrap();
        assert!(waited >= Duration::from_millis(30));
        drop(p1);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(g.in_use(), 0);
    }
}
