//! The [`MeetingMemory`] port, its value types and the Postgres adapter.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppResult, RoomId, UserId, WorkspaceId};

use crate::summary::SummaryContent;

/// What to remember for one meeting. Everything is scoped to `workspace_id`.
#[derive(Debug, Clone)]
pub struct PriorQuery {
    pub workspace_id: WorkspaceId,
    /// The meeting being summarized / assisted: never part of its own prior context.
    pub room_id: RoomId,
    /// The host (copilot: the user the assistant works for). Without a host there is no memory.
    pub host: Option<UserId>,
    /// Series of the meeting, when it is one of a recurring series.
    pub series_id: Option<Uuid>,
    /// Title; a case-insensitive match ranks earlier summaries of the same name first.
    pub title: Option<String>,
    /// Only summaries completed before this instant count as "previous".
    pub before: DateTime<Utc>,
    /// Newest open action items to return (N).
    pub max_open_items: u32,
    /// Previous summaries to return (K).
    pub max_summaries: u32,
}

/// An action item of an earlier meeting that is still open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenItem {
    pub id: Uuid,
    pub room_id: Uuid,
    pub room_title: Option<String>,
    pub task: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub assignee_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// A previous meeting's summary, trimmed for use in prompts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriorSummary {
    pub room_id: Uuid,
    pub title: Option<String>,
    pub completed_at: DateTime<Utc>,
    pub summary_short: String,
    pub decisions: Vec<String>,
    pub open_questions: Vec<String>,
    /// Same series id or same title as the current meeting.
    pub same_series: bool,
}

/// Cross-meeting context of one meeting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriorContext {
    /// Open items of previous meetings involving the host, newest first.
    pub open_action_items: Vec<OpenItem>,
    /// Last completed summaries of the host's previous meetings, same series/title first, then
    /// newest first.
    pub last_summaries: Vec<PriorSummary>,
}

impl PriorContext {
    pub fn is_empty(&self) -> bool {
        self.open_action_items.is_empty() && self.last_summaries.is_empty()
    }
}

#[async_trait]
pub trait MeetingMemory: Send + Sync {
    /// Prior context for the meeting. Never includes the meeting itself, other workspaces,
    /// done items, or meetings the host was not the host of (nor items not involving them).
    async fn prior_context(&self, q: &PriorQuery) -> AppResult<PriorContext>;
}

/// Remembers nothing.
pub struct NoMemory;

#[async_trait]
impl MeetingMemory for NoMemory {
    async fn prior_context(&self, _: &PriorQuery) -> AppResult<PriorContext> {
        Ok(PriorContext::default())
    }
}

/// [`MeetingMemory`] over `assistant.meeting_summaries` / `assistant.action_items`.
#[derive(Clone)]
pub struct PgMeetingMemory {
    pool: PgPool,
}

impl PgMeetingMemory {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct ItemRow {
    id: Uuid,
    room_id: Uuid,
    room_title: Option<String>,
    task: String,
    owner: Option<String>,
    due: Option<String>,
    assignee_user_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    room_id: Uuid,
    title: Option<String>,
    completed_at: DateTime<Utc>,
    content: Option<Json<SummaryContent>>,
    same_series: bool,
}

#[async_trait]
impl MeetingMemory for PgMeetingMemory {
    async fn prior_context(&self, q: &PriorQuery) -> AppResult<PriorContext> {
        let Some(host) = q.host else {
            return Ok(PriorContext::default());
        };
        // Every previous room is represented by its latest completed summary (rn = 1).
        let latest = "WITH latest AS ( \
               SELECT s.*, row_number() OVER (PARTITION BY s.room_id ORDER BY s.version DESC) AS rn \
               FROM assistant.meeting_summaries s \
               WHERE s.workspace_id = $1 AND s.room_id <> $2 AND s.status = 'completed' \
                 AND s.completed_at < $3) ";
        let items = if q.max_open_items == 0 {
            Vec::new()
        } else {
            sqlx::query_as::<_, ItemRow>(&format!(
                "{latest} \
                 SELECT a.id, a.room_id, l.title AS room_title, a.task, a.owner, a.due, \
                        a.assignee_user_id, a.created_at \
                 FROM assistant.action_items a \
                 JOIN latest l ON l.id = a.summary_id AND l.rn = 1 \
                 WHERE a.workspace_id = $1 AND a.status = 'open' \
                   AND (l.host_user_id = $4 OR a.assignee_user_id = $4) \
                 ORDER BY a.created_at DESC, a.id LIMIT $5"
            ))
            .bind(q.workspace_id.as_uuid())
            .bind(q.room_id.as_uuid())
            .bind(q.before)
            .bind(host.as_uuid())
            .bind(i64::from(q.max_open_items))
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_error)?
        };
        let summaries = if q.max_summaries == 0 {
            Vec::new()
        } else {
            sqlx::query_as::<_, SummaryRow>(&format!(
                "{latest} \
                 SELECT l.room_id, l.title, l.completed_at, l.content, \
                        COALESCE(l.series_id = $5, false) \
                         OR COALESCE(lower(btrim(l.title)) = lower(btrim($6)), false) \
                        AS same_series \
                 FROM latest l \
                 WHERE l.rn = 1 AND l.host_user_id = $4 \
                 ORDER BY same_series DESC, l.completed_at DESC, l.room_id LIMIT $7"
            ))
            .bind(q.workspace_id.as_uuid())
            .bind(q.room_id.as_uuid())
            .bind(q.before)
            .bind(host.as_uuid())
            .bind(q.series_id)
            .bind(q.title.as_deref())
            .bind(i64::from(q.max_summaries))
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_error)?
        };
        Ok(PriorContext {
            open_action_items: items
                .into_iter()
                .map(|r| OpenItem {
                    id: r.id,
                    room_id: r.room_id,
                    room_title: r.room_title,
                    task: r.task,
                    owner: r.owner,
                    due: r.due,
                    assignee_user_id: r.assignee_user_id,
                    created_at: r.created_at,
                })
                .collect(),
            last_summaries: summaries
                .into_iter()
                .map(|r| {
                    let c = r.content.map(|j| j.0).unwrap_or_default();
                    PriorSummary {
                        room_id: r.room_id,
                        title: r.title,
                        completed_at: r.completed_at,
                        summary_short: c.summary_short,
                        decisions: c.decisions,
                        open_questions: c.open_questions,
                        same_series: r.same_series,
                    }
                })
                .collect(),
        })
    }
}
