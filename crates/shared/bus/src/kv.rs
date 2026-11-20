use redis::aio::ConnectionManager;

use crate::BusError;

/// `SET key 1 NX EX ttl`. True if this caller claimed the key (first time).
pub async fn claim_once(
    conn: &mut ConnectionManager,
    key: &str,
    ttl_secs: u64,
) -> Result<bool, BusError> {
    let r: Option<String> = redis::cmd("SET")
        .arg(key)
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(ttl_secs)
        .query_async(conn)
        .await?;
    Ok(r.is_some())
}

/// `SET key owner NX PX ttl_ms`. True if the lease was acquired.
pub async fn try_acquire_lease(
    conn: &mut ConnectionManager,
    key: &str,
    owner: &str,
    ttl_ms: u64,
) -> Result<bool, BusError> {
    let r: Option<String> = redis::cmd("SET")
        .arg(key)
        .arg(owner)
        .arg("NX")
        .arg("PX")
        .arg(ttl_ms)
        .query_async(conn)
        .await?;
    Ok(r.is_some())
}

/// Extend the lease TTL only if `owner` still holds it.
pub async fn renew_lease(
    conn: &mut ConnectionManager,
    key: &str,
    owner: &str,
    ttl_ms: u64,
) -> Result<bool, BusError> {
    let script = redis::Script::new(
        "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('PEXPIRE', KEYS[1], ARGV[2]) else return 0 end",
    );
    let n: i64 = script
        .key(key)
        .arg(owner)
        .arg(ttl_ms)
        .invoke_async(conn)
        .await?;
    Ok(n == 1)
}

/// Delete the lease only if `owner` holds it.
pub async fn release_lease(
    conn: &mut ConnectionManager,
    key: &str,
    owner: &str,
) -> Result<bool, BusError> {
    let script = redis::Script::new(
        "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) else return 0 end",
    );
    let n: i64 = script.key(key).arg(owner).invoke_async(conn).await?;
    Ok(n == 1)
}
