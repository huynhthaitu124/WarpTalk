//! Time abstraction for testability.

use chrono::{DateTime, Utc};
use std::sync::Mutex;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Settable clock for tests.
#[derive(Debug)]
pub struct FixedClock(Mutex<DateTime<Utc>>);

impl FixedClock {
    pub fn new(at: DateTime<Utc>) -> Self {
        Self(Mutex::new(at))
    }

    pub fn set(&self, at: DateTime<Utc>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = at;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn fixed_clock_can_be_set() {
        let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let c = FixedClock::new(t1);
        assert_eq!(c.now(), t1);
        c.set(t2);
        assert_eq!(c.now(), t2);
        let _ = SystemClock.now();
    }
}
