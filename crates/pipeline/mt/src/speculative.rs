//! Speculative translation cache, keyed by (room, speaker, target language, normalised text).
//!
//! Finished sentences of a still-running turn are translated ahead of time; when the validated
//! segment arrives, a hit costs nothing (or only the remainder of an in-flight call). Output is
//! CACHE-ONLY: nothing here is ever published unvalidated. Speculation is disposable: it never
//! queues behind itself (one slot by default, `try_acquire`), so it cannot occupy the hot
//! Realtime pool needed by validated traffic; an entry unclaimed for `ttl` expires, and a call
//! over `timeout` is abandoned (the validated path then translates on demand).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::sync::Semaphore;

use crate::translator::TranslateOutput;

pub type SpecKey = (String, String, String, String);
pub type SpecFuture = Shared<BoxFuture<'static, Option<TranslateOutput>>>;

struct Entry {
    created: Instant,
    fut: SpecFuture,
}

pub struct SpeculativeCache {
    entries: Mutex<HashMap<SpecKey, Entry>>,
    slots: Arc<Semaphore>,
}

impl SpeculativeCache {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(max_concurrent.max(1))),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SpecKey, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Starts `work` in the background unless a fresh entry exists or no slot is free. Returns
    /// whether a call was started.
    pub fn try_prefetch<F>(&self, key: SpecKey, ttl: Duration, timeout: Duration, work: F) -> bool
    where
        F: std::future::Future<Output = Option<TranslateOutput>> + Send + 'static,
    {
        let mut entries = self.lock();
        entries.retain(|_, e| e.created.elapsed() <= ttl);
        if entries.contains_key(&key) {
            return false;
        }
        let Ok(permit) = self.slots.clone().try_acquire_owned() else {
            return false;
        };
        let handle = tokio::spawn(async move {
            let _permit = permit;
            tokio::time::timeout(timeout, work).await.ok().flatten()
        });
        let fut: BoxFuture<'static, Option<TranslateOutput>> =
            async move { handle.await.ok().flatten() }.boxed();
        entries.insert(
            key,
            Entry {
                created: Instant::now(),
                fut: fut.shared(),
            },
        );
        true
    }

    /// Claims the entry (a hit is consumed: one validated segment uses it once). `None` when
    /// absent or expired. Await the returned future; `None` inside means the speculative call
    /// was abandoned and the caller must translate for real.
    pub fn take(&self, key: &SpecKey, ttl: Duration) -> Option<SpecFuture> {
        let e = self.lock().remove(key)?;
        (e.created.elapsed() <= ttl).then_some(e.fut)
    }
}
