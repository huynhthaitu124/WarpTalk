//! Live channel of a conversation: Redis streams (shared by every replica) or in-process memory
//! (single replica / tests).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::streams::StreamRangeReply;
use serde_json::Value;
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult};

use crate::application::chat_ports::{ChatEvent, ChatEventBus};

const MAX_EVENTS: usize = 1_000;
const READ_BATCH: usize = 200;
const STREAM_TTL_SECS: i64 = 3_600;
const STOP_TTL_SECS: u64 = 600;

pub fn events_key(conversation: Uuid) -> String {
    format!("assistant:conversation:{conversation}:events")
}

pub fn stop_key(message: Uuid) -> String {
    format!("assistant:stop:{message}")
}

fn redis_err(e: redis::RedisError) -> AppError {
    AppError::unavailable(format!("redis: {e}"))
}

/// Redis stream per conversation (`XADD MAXLEN ~ 1000`, expires an hour after the last event).
/// Readers poll with `XRANGE` because a blocking `XREAD` would stall the shared multiplexed
/// connection.
#[derive(Clone)]
pub struct RedisChatEventBus {
    conn: ConnectionManager,
}

impl RedisChatEventBus {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl ChatEventBus for RedisChatEventBus {
    async fn publish(&self, conversation: Uuid, event: ChatEvent) -> AppResult<()> {
        let key = events_key(conversation);
        let mut conn = self.conn.clone();
        redis::pipe()
            .cmd("XADD")
            .arg(&key)
            .arg("MAXLEN")
            .arg("~")
            .arg(MAX_EVENTS)
            .arg("*")
            .arg("name")
            .arg(&event.name)
            .arg("data")
            .arg(event.data.to_string())
            .ignore()
            .cmd("EXPIRE")
            .arg(&key)
            .arg(STREAM_TTL_SECS)
            .ignore()
            .query_async::<()>(&mut conn)
            .await
            .map_err(redis_err)
    }

    async fn cursor_now(&self, conversation: Uuid) -> AppResult<String> {
        let mut conn = self.conn.clone();
        let reply: StreamRangeReply = redis::cmd("XREVRANGE")
            .arg(events_key(conversation))
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(1)
            .query_async(&mut conn)
            .await
            .map_err(redis_err)?;
        Ok(reply
            .ids
            .first()
            .map(|e| e.id.clone())
            .unwrap_or_else(|| "0-0".to_owned()))
    }

    async fn read_after(
        &self,
        conversation: Uuid,
        cursor: &str,
    ) -> AppResult<Vec<(String, ChatEvent)>> {
        let mut conn = self.conn.clone();
        let reply: StreamRangeReply = redis::cmd("XRANGE")
            .arg(events_key(conversation))
            .arg(format!("({cursor}"))
            .arg("+")
            .arg("COUNT")
            .arg(READ_BATCH)
            .query_async(&mut conn)
            .await
            .map_err(redis_err)?;
        Ok(reply
            .ids
            .iter()
            .filter_map(|e| {
                let name: String = e.get("name")?;
                let data: String = e.get("data")?;
                let data = serde_json::from_str::<Value>(&data).ok()?;
                Some((e.id.clone(), ChatEvent { name, data }))
            })
            .collect())
    }

    async fn request_stop(&self, message: Uuid) -> AppResult<()> {
        let mut conn = self.conn.clone();
        redis::cmd("SET")
            .arg(stop_key(message))
            .arg(1)
            .arg("EX")
            .arg(STOP_TTL_SECS)
            .query_async::<()>(&mut conn)
            .await
            .map_err(redis_err)
    }

    async fn stop_requested(&self, message: Uuid) -> AppResult<bool> {
        let mut conn = self.conn.clone();
        redis::cmd("EXISTS")
            .arg(stop_key(message))
            .query_async::<bool>(&mut conn)
            .await
            .map_err(redis_err)
    }
}

/// Single-process bus: events live in memory (last 1000 per conversation). Only correct with
/// one API replica; `apps/api` uses it when Redis is not configured.
#[derive(Default)]
pub struct InMemoryChatEventBus {
    state: Mutex<MemState>,
}

#[derive(Default)]
struct MemState {
    next: u64,
    events: HashMap<Uuid, Vec<(u64, ChatEvent)>>,
    stops: HashSet<Uuid>,
}

impl InMemoryChatEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> AppResult<std::sync::MutexGuard<'_, MemState>> {
        self.state
            .lock()
            .map_err(|_| AppError::internal("chat bus lock poisoned"))
    }
}

fn parse_cursor(cursor: &str) -> u64 {
    cursor
        .split('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

#[async_trait]
impl ChatEventBus for InMemoryChatEventBus {
    async fn publish(&self, conversation: Uuid, event: ChatEvent) -> AppResult<()> {
        let mut s = self.lock()?;
        s.next += 1;
        let id = s.next;
        let list = s.events.entry(conversation).or_default();
        list.push((id, event));
        if list.len() > MAX_EVENTS {
            let excess = list.len() - MAX_EVENTS;
            list.drain(..excess);
        }
        Ok(())
    }

    async fn cursor_now(&self, conversation: Uuid) -> AppResult<String> {
        let s = self.lock()?;
        let last = s
            .events
            .get(&conversation)
            .and_then(|l| l.last())
            .map_or(0, |(id, _)| *id);
        Ok(format!("{last}-0"))
    }

    async fn read_after(
        &self,
        conversation: Uuid,
        cursor: &str,
    ) -> AppResult<Vec<(String, ChatEvent)>> {
        let after = parse_cursor(cursor);
        let s = self.lock()?;
        Ok(s.events
            .get(&conversation)
            .map(|l| {
                l.iter()
                    .filter(|(id, _)| *id > after)
                    .take(READ_BATCH)
                    .map(|(id, e)| (format!("{id}-0"), e.clone()))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn request_stop(&self, message: Uuid) -> AppResult<()> {
        self.lock()?.stops.insert(message);
        Ok(())
    }

    async fn stop_requested(&self, message: Uuid) -> AppResult<bool> {
        Ok(self.lock()?.stops.contains(&message))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn memory_bus_reads_after_cursor() {
        let bus = InMemoryChatEventBus::new();
        let c = Uuid::now_v7();
        let ev = |n: &str| ChatEvent {
            name: n.to_owned(),
            data: json!({}),
        };
        bus.publish(c, ev("a")).await.unwrap();
        let cur = bus.cursor_now(c).await.unwrap();
        bus.publish(c, ev("b")).await.unwrap();
        let got = bus.read_after(c, &cur).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1.name, "b");
        assert!(bus.read_after(c, &got[0].0).await.unwrap().is_empty());
        let m = Uuid::now_v7();
        assert!(!bus.stop_requested(m).await.unwrap());
        bus.request_stop(m).await.unwrap();
        assert!(bus.stop_requested(m).await.unwrap());
    }
}
