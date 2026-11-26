use redis::aio::ConnectionManager;
use warptalk_kernel::{AppError, AppResult};

use crate::application::ports::Throttle;

/// Redis-backed [`Throttle`]: `SET NX EX` cooldowns and an atomic INCR+EXPIRE window counter.
#[derive(Clone)]
pub struct RedisThrottle {
    conn: ConnectionManager,
}

impl RedisThrottle {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

fn redis_err(e: redis::RedisError) -> AppError {
    AppError::internal(format!("throttle redis: {e}"))
}

#[async_trait::async_trait]
impl Throttle for RedisThrottle {
    async fn cooldown(&self, key: &str, secs: u64) -> AppResult<bool> {
        let mut conn = self.conn.clone();
        let set: Option<String> = redis::cmd("SET")
            .arg(key)
            .arg(1)
            .arg("NX")
            .arg("EX")
            .arg(secs.max(1))
            .query_async(&mut conn)
            .await
            .map_err(redis_err)?;
        Ok(set.is_some())
    }

    async fn hit(&self, key: &str, window_secs: u64, max: u32) -> AppResult<bool> {
        let mut conn = self.conn.clone();
        let count: i64 = redis::Script::new(
            "local c = redis.call('INCR', KEYS[1]) \
             if c == 1 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end \
             return c",
        )
        .key(key)
        .arg(window_secs.max(1))
        .invoke_async(&mut conn)
        .await
        .map_err(redis_err)?;
        Ok(count <= i64::from(max))
    }
}
