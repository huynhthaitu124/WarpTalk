use futures::{Stream, StreamExt};
use redis::aio::ConnectionManager;
use serde::Serialize;

use crate::BusError;

/// PUBLISH the JSON encoding of `value`. Returns the receiver count.
pub async fn publish_json<T: Serialize>(
    conn: &mut ConnectionManager,
    channel: &str,
    value: &T,
) -> Result<i64, BusError> {
    let json = serde_json::to_string(value).map_err(|e| BusError::Decode(e.to_string()))?;
    Ok(redis::cmd("PUBLISH")
        .arg(channel)
        .arg(json)
        .query_async(conn)
        .await?)
}

/// Subscribe on a dedicated connection; yields `(channel, payload)`.
pub async fn subscribe(
    url: &str,
    channels: &[String],
) -> Result<impl Stream<Item = (String, String)>, BusError> {
    let client = redis::Client::open(url)?;
    let mut ps = client.get_async_pubsub().await?;
    for c in channels {
        ps.subscribe(c).await?;
    }
    Ok(ps.into_on_message().filter_map(|m| async move {
        let payload: String = m.get_payload().ok()?;
        Some((m.get_channel_name().to_string(), payload))
    }))
}
