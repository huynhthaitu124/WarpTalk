use redis::aio::ConnectionManager;
use serde::Serialize;

use crate::BusError;

/// XADD with approximate trimming (`MAXLEN ~ maxlen`). Returns the entry id.
pub async fn xadd(
    conn: &mut ConnectionManager,
    stream: &str,
    fields: &[(&str, String)],
    maxlen: usize,
) -> Result<String, BusError> {
    let mut cmd = redis::cmd("XADD");
    cmd.arg(stream).arg("MAXLEN").arg("~").arg(maxlen).arg("*");
    for (k, v) in fields {
        cmd.arg(*k).arg(v);
    }
    Ok(cmd.query_async(conn).await?)
}

/// XADD a single field `data` holding the JSON encoding of `value`.
pub async fn xadd_json<T: Serialize>(
    conn: &mut ConnectionManager,
    stream: &str,
    value: &T,
    maxlen: usize,
) -> Result<String, BusError> {
    let json = serde_json::to_string(value).map_err(|e| BusError::Decode(e.to_string()))?;
    xadd(conn, stream, &[("data", json)], maxlen).await
}
