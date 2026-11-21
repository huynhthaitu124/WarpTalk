//! Per-key publish ordering.
//!
//! Translation runs concurrently (right for latency) but a later sentence must never overtake an
//! earlier one of the same speaker and language: a long A and a short B raced upstream and the
//! listener heard B, then A late and sped up (production kept 14 inversions in the retained
//! window). Only the PUBLISH waits: B's first publish waits until A's task has finished.
//!
//! The wait is bounded (`publish_order_wait`, 10 s upstream): a stalled A must not silence
//! everyone after it. When the bound hits, B goes out and A is still published when it lands;
//! nothing is dropped either way. Liveness is structural: a [`Ticket`] that is dropped (skip,
//! panic, early return) counts as finished.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::watch;

pub type OrderKey = (String, String, String);

#[derive(Default)]
pub struct OrderGate {
    chains: Mutex<HashMap<OrderKey, watch::Receiver<bool>>>,
}

pub struct Ticket {
    prev: Option<watch::Receiver<bool>>,
    done: watch::Sender<bool>,
}

impl OrderGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the next place in `key`'s chain. Call in arrival order.
    pub fn ticket(&self, key: OrderKey) -> Ticket {
        let (done, rx) = watch::channel(false);
        let mut chains = self.chains.lock().unwrap_or_else(PoisonError::into_inner);
        if chains.len() > 1024 {
            // Finished chains (sender dropped) carry no information.
            chains.retain(|_, r| r.has_changed().is_ok());
        }
        let prev = chains.insert(key, rx);
        Ticket { prev, done }
    }

    pub fn forget_room(&self, room: &str) {
        self.chains
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(r, _, _), _| r != room);
    }
}

impl Ticket {
    /// Waits for the predecessor. `true` when it finished, `false` when `max` elapsed first.
    pub async fn wait_turn(&mut self, max: Duration) -> bool {
        let Some(prev) = self.prev.as_mut() else {
            return true;
        };
        // Err means the predecessor's sender was dropped: it finished (or died). Either way go.
        match tokio::time::timeout(max, prev.wait_for(|done| *done)).await {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    /// Marks this ticket's work complete, releasing the successor.
    pub fn finish(self) {
        let _ = self.done.send(true);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn key() -> OrderKey {
        ("r".into(), "s".into(), "vi".into())
    }

    #[tokio::test]
    async fn successor_waits_for_finish() {
        let g = OrderGate::new();
        let a = g.ticket(key());
        let mut b = g.ticket(key());
        let waiter = tokio::spawn(async move { b.wait_turn(Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiter.is_finished());
        a.finish();
        assert!(waiter.await.unwrap());
    }

    #[tokio::test]
    async fn dropped_predecessor_releases_and_timeout_reports() {
        let g = OrderGate::new();
        let a = g.ticket(key());
        let mut b = g.ticket(key());
        drop(a);
        assert!(b.wait_turn(Duration::from_secs(5)).await);
        let _c_prev_alive = g.ticket(key());
        let mut d = g.ticket(key());
        assert!(!d.wait_turn(Duration::from_millis(30)).await);
    }

    #[tokio::test]
    async fn other_keys_are_independent() {
        let g = OrderGate::new();
        let _a = g.ticket(key());
        let mut other = g.ticket(("r".into(), "s".into(), "en".into()));
        assert!(other.wait_turn(Duration::from_millis(10)).await);
    }
}
