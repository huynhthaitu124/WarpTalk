//! Redis adapters: live follow-ups (`assistant:followups`) and the summary streams.

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::streams::StreamRangeReply;
use warptalk_contracts::streams::{ASSISTANT_SUMMARY_REQUESTS, ASSISTANT_SUMMARY_RESULTS};
use warptalk_kernel::RoomId;

use super::model::{SUMMARY_GROUP, SummaryRequest, SummaryResultEvent};
use super::ports::{FollowUpSource, LiveFollowUp, SummaryBus};
use crate::suggestion::{ASSISTANT_FOLLOWUPS, FollowUpEvent};

/// Entries of `assistant:followups` scanned (newest first) when looking for one room's.
const FOLLOWUP_SCAN: usize = 5000;
const STREAM_MAXLEN: usize = 10_000;

pub struct RedisFollowUpSource {
    conn: ConnectionManager,
}

impl RedisFollowUpSource {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl FollowUpSource for RedisFollowUpSource {
    async fn for_room(&self, room: RoomId, limit: u32) -> Vec<LiveFollowUp> {
        if limit == 0 {
            return Vec::new();
        }
        let mut conn = self.conn.clone();
        let reply: Result<StreamRangeReply, _> = redis::cmd("XREVRANGE")
            .arg(ASSISTANT_FOLLOWUPS)
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(FOLLOWUP_SCAN)
            .query_async(&mut conn)
            .await;
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%room, error = %e, "live follow-ups unavailable");
                return Vec::new();
            }
        };
        let mut out: Vec<LiveFollowUp> = reply
            .ids
            .iter()
            .filter_map(|id| {
                let raw: String = id.get("data")?;
                serde_json::from_str::<FollowUpEvent>(&raw).ok()
            })
            .filter(|e| e.meeting_id == room)
            .map(|e| LiveFollowUp {
                task: e.follow_up.task,
                owner: e.follow_up.owner,
                due: e.follow_up.due,
            })
            .take(limit as usize)
            .collect();
        out.reverse();
        out
    }
}

pub struct RedisSummaryBus {
    conn: ConnectionManager,
}

impl RedisSummaryBus {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }

    pub fn dead_letter_stream() -> String {
        format!("{ASSISTANT_SUMMARY_REQUESTS}:{SUMMARY_GROUP}:dead-letter")
    }
}

#[async_trait]
impl SummaryBus for RedisSummaryBus {
    async fn request(&self, req: &SummaryRequest) -> Result<(), String> {
        let mut conn = self.conn.clone();
        warptalk_bus::xadd_json(&mut conn, ASSISTANT_SUMMARY_REQUESTS, req, STREAM_MAXLEN)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn result(&self, ev: &SummaryResultEvent) {
        let mut conn = self.conn.clone();
        if let Err(e) =
            warptalk_bus::xadd_json(&mut conn, ASSISTANT_SUMMARY_RESULTS, ev, STREAM_MAXLEN).await
        {
            tracing::warn!(error = %e, "summary result not published");
        }
    }

    async fn dead_letter(&self, req: &SummaryRequest, error: &str) {
        let mut conn = self.conn.clone();
        let data = serde_json::to_string(req).unwrap_or_default();
        let fields = [("data", data), ("error", error.to_owned())];
        if let Err(e) = warptalk_bus::xadd(
            &mut conn,
            &Self::dead_letter_stream(),
            &fields,
            STREAM_MAXLEN,
        )
        .await
        {
            tracing::warn!(error = %e, "summary dead letter not written");
        }
    }
}
