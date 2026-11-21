//! Tiny TTL cache driven by the injected clock (policy lookups, spec 12 §8).

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

pub struct TtlCache<K, V> {
    ttl: Duration,
    entries: Mutex<HashMap<K, (V, DateTime<Utc>)>>,
}

impl<K: Eq + Hash, V: Clone> TtlCache<K, V> {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, key: &K, now: DateTime<Utc>) -> Option<V> {
        let map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        map.get(key)
            .filter(|(_, at)| now - *at < self.ttl)
            .map(|(v, _)| v.clone())
    }

    pub fn put(&self, key: K, value: V, now: DateTime<Utc>) {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > 10_000 {
            let ttl = self.ttl;
            map.retain(|_, (_, at)| now - *at < ttl);
        }
        map.insert(key, (value, now));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn expires() {
        let c = TtlCache::new(Duration::minutes(5));
        let t0 = Utc::now();
        c.put("a", true, t0);
        assert_eq!(c.get(&"a", t0 + Duration::minutes(4)), Some(true));
        assert_eq!(c.get(&"a", t0 + Duration::minutes(6)), None);
        assert_eq!(c.get(&"b", t0), None);
    }
}
