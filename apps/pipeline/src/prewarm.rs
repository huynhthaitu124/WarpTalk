//! TTS pre-warm: a [`Synthesizer`] decorator that can open contexts ahead of time.
//!
//! The TTS stage opens a context when a translated sentence reaches it. With a warm vendor
//! socket that costs about 0.1 ms, but when the pool is empty or a socket died the dial is
//! about 490 ms on the critical path (spec 08 section 3.2: cold 669 ms against 180 ms warm).
//! [`PrewarmSynthesizer::prewarm`] is called when a segment enters MT, so the dial overlaps the
//! 650-900 ms the translation takes; the stage's own `open_context` then claims the ready
//! context. Contexts nobody claims are cancelled after `max_age`, and at most
//! [`MAX_PER_KEY`] are kept per (language, voice).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Notify;
use warptalk_tts::{ContextParams, SynthContext, Synthesizer, TtsError};

/// Pre-opened contexts kept per (language, voice).
pub const MAX_PER_KEY: usize = 1;

type Key = (String, String);

struct Ready {
    ctx: Box<dyn SynthContext>,
    opened: Instant,
}

/// A dial in flight. `done` is checked after `notified()` is created, so a finish that races
/// the claimer is never missed.
#[derive(Default)]
pub struct Dial {
    done: std::sync::atomic::AtomicBool,
    notify: Notify,
}

impl Dial {
    async fn wait(&self) {
        use std::sync::atomic::Ordering::Acquire;
        let notified = self.notify.notified();
        if self.done.load(Acquire) {
            return;
        }
        // Bounded: a dial that hangs must not hold the claimer past a cold dial of its own.
        let _ = tokio::time::timeout(Duration::from_secs(2), notified).await;
    }

    fn finish(&self) {
        self.done.store(true, std::sync::atomic::Ordering::Release);
        self.notify.notify_waiters();
    }
}

enum Slot {
    /// A dial is in flight (counts towards the per-key limit); a claimer waits for it instead
    /// of dialing its own.
    Dialing(Arc<Dial>),
    Ready(Ready),
}

#[derive(Default)]
struct Pool {
    slots: HashMap<Key, Vec<Slot>>,
}

pub struct PrewarmSynthesizer {
    inner: Arc<dyn Synthesizer>,
    pool: Arc<Mutex<Pool>>,
    stats: Arc<Stats>,
}

#[derive(Debug, Default)]
pub struct Stats {
    pub started: std::sync::atomic::AtomicU64,
    pub claimed: std::sync::atomic::AtomicU64,
    pub expired: std::sync::atomic::AtomicU64,
}

fn lock(p: &Mutex<Pool>) -> std::sync::MutexGuard<'_, Pool> {
    p.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PrewarmSynthesizer {
    pub fn new(inner: Arc<dyn Synthesizer>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            pool: Arc::new(Mutex::new(Pool::default())),
            stats: Arc::new(Stats::default()),
        })
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Opens a context for (language, voice) in the background unless one is ready or dialing.
    /// Returns immediately; never fails (a failed dial is simply not pooled).
    pub fn prewarm(self: &Arc<Self>, language: &str, voice_id: &str, max_age: Duration) {
        use std::sync::atomic::Ordering::Relaxed;
        let key: Key = (language.to_string(), voice_id.to_string());
        let notify = Arc::new(Dial::default());
        {
            let mut p = lock(&self.pool);
            let slots = p.slots.entry(key.clone()).or_default();
            if slots.len() >= MAX_PER_KEY {
                return;
            }
            slots.push(Slot::Dialing(notify.clone()));
        }
        self.stats.started.fetch_add(1, Relaxed);
        let me = self.clone();
        tokio::spawn(async move {
            let params = ContextParams {
                language: key.0.clone(),
                voice_id: key.1.clone(),
            };
            let opened = me.inner.open_context(&params).await;
            let mut stale = None;
            {
                let mut p = lock(&me.pool);
                let slots = p.slots.entry(key.clone()).or_default();
                if let Some(i) = slots
                    .iter()
                    .position(|s| matches!(s, Slot::Dialing(n) if Arc::ptr_eq(n, &notify)))
                {
                    slots.remove(i);
                }
                if let Ok(ctx) = opened {
                    if slots.len() < MAX_PER_KEY {
                        slots.push(Slot::Ready(Ready {
                            ctx,
                            opened: Instant::now(),
                        }));
                    } else {
                        stale = Some(ctx);
                    }
                }
            }
            notify.finish();
            if let Some(c) = stale {
                c.cancel().await;
            }
            // Unclaimed contexts must not sit on a vendor slot: reap after max_age.
            tokio::time::sleep(max_age).await;
            let expired = {
                let mut p = lock(&me.pool);
                let mut out = Vec::new();
                if let Some(slots) = p.slots.get_mut(&key) {
                    let mut i = 0;
                    while i < slots.len() {
                        let old =
                            matches!(&slots[i], Slot::Ready(r) if r.opened.elapsed() >= max_age);
                        if old && let Slot::Ready(r) = slots.remove(i) {
                            out.push(r.ctx);
                        } else {
                            i += 1;
                        }
                    }
                }
                out
            };
            for c in expired {
                me.stats.expired.fetch_add(1, Relaxed);
                c.cancel().await;
            }
        });
    }

    /// A ready context, or the wait for the one being dialed (`Err` carries the waiter).
    fn claim(&self, p: &ContextParams) -> Result<Option<Box<dyn SynthContext>>, Arc<Dial>> {
        let key: Key = (p.language.clone(), p.voice_id.clone());
        let mut pool = lock(&self.pool);
        let Some(slots) = pool.slots.get_mut(&key) else {
            return Ok(None);
        };
        if let Some(i) = slots.iter().position(|s| matches!(s, Slot::Ready(_))) {
            return match slots.remove(i) {
                Slot::Ready(r) => Ok(Some(r.ctx)),
                Slot::Dialing(_) => Ok(None),
            };
        }
        match slots.iter().find_map(|s| match s {
            Slot::Dialing(n) => Some(n.clone()),
            Slot::Ready(_) => None,
        }) {
            Some(n) => Err(n),
            None => Ok(None),
        }
    }
}

#[async_trait]
impl Synthesizer for PrewarmSynthesizer {
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    async fn open_context(&self, p: &ContextParams) -> Result<Box<dyn SynthContext>, TtsError> {
        let claimed = match self.claim(p) {
            Ok(c) => c,
            Err(dialing) => {
                dialing.wait().await;
                self.claim(p).unwrap_or(None)
            }
        };
        if let Some(ctx) = claimed {
            self.stats
                .claimed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(ctx);
        }
        self.inner.open_context(p).await
    }
}
