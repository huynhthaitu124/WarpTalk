//! In-process [`Throttle`] (default without Redis, and for tests with a fixed clock).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use warptalk_kernel::{AppError, AppResult, Clock};

use super::ports::Throttle;

pub struct MemoryThrottle {
    clock: Arc<dyn Clock>,
    /// key -> (count, window end)
    slots: Mutex<HashMap<String, (u32, DateTime<Utc>)>>,
}

impl MemoryThrottle {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            slots: Mutex::new(HashMap::new()),
        }
    }

    fn bump(&self, key: &str, window_secs: u64, only_if_free: bool) -> AppResult<(u32, bool)> {
        let now = self.clock.now();
        let mut slots = self
            .slots
            .lock()
            .map_err(|_| AppError::internal("throttle lock poisoned"))?;
        if slots.len() > 10_000 {
            slots.retain(|_, (_, end)| *end > now);
        }
        let secs = i64::try_from(window_secs).unwrap_or(i64::MAX / 4);
        match slots.get_mut(key) {
            Some((count, end)) if *end > now => {
                if only_if_free {
                    return Ok((*count, false));
                }
                *count += 1;
                Ok((*count, true))
            }
            _ => {
                slots.insert(key.to_owned(), (1, now + Duration::seconds(secs)));
                Ok((1, true))
            }
        }
    }
}

#[async_trait::async_trait]
impl Throttle for MemoryThrottle {
    async fn cooldown(&self, key: &str, secs: u64) -> AppResult<bool> {
        Ok(self.bump(key, secs, true)?.1)
    }

    async fn hit(&self, key: &str, window_secs: u64, max: u32) -> AppResult<bool> {
        Ok(self.bump(key, window_secs, false)?.0 <= max)
    }
}
