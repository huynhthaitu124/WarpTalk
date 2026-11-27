//! Meeting minutes (biên bản họp), paths relative to `/api/v1`, camelCase bodies.
//!
//! The draft is built from the room's newest completed AI summary. `content` is the web's JSON
//! document, stored verbatim. Lifecycle: DRAFT -> (sign) IN_REVIEW -> (approve) APPROVED;
//! `revise` on an approved record opens version N+1 as a new DRAFT and keeps the approved one.
//!
//! Access: anyone who may see the room's summary may read; the host or a workspace OWNER/ADMIN
//! may draft, edit, sign, approve and revise.

use axum::Router;
use axum::extract::{FromRef, State};
use axum::routing::{get, post, put};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, RoomId};
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{AuthUser, JwtKeys};

use super::summary::SummaryApi;
use crate::summary::{RoomAccessInfo, SummaryContent};

#[derive(Clone)]
struct State_ {
    api: SummaryApi,
    jwt: JwtKeys,
}

impl FromRef<State_> for JwtKeys {
    fn from_ref(s: &State_) -> JwtKeys {
        s.jwt.clone()
    }
}

pub fn router(api: SummaryApi, jwt: JwtKeys) -> Router {
    Router::new()
        .route("/rooms/{room_id}/minutes", get(get_minutes))
        .route("/rooms/{room_id}/minutes/draft", post(create_draft))
        .route("/rooms/{room_id}/minutes/translation", get(get_translation))
        .route(
            "/rooms/{room_id}/minutes/export.docx",
            get(export_unavailable),
        )
        .route(
            "/rooms/{room_id}/minutes/export.pdf",
            get(export_unavailable),
        )
        .route("/rooms/{room_id}/minutes/{minutes_id}", put(update_content))
        .route(
            "/rooms/{room_id}/minutes/{minutes_id}/secretary",
            put(designate_secretary),
        )
        .route("/rooms/{room_id}/minutes/{minutes_id}/sign", post(sign))
        .route(
            "/rooms/{room_id}/minutes/{minutes_id}/approve",
            post(approve),
        )
        .route("/rooms/{room_id}/minutes/{minutes_id}/revise", post(revise))
        .with_state(State_ { api, jwt })
}

// ------------------------------------------------------------------ row + DTO

const COLS: &str = "id, room_id, minutes_no, meeting_title, status, version, is_current, \
    previous_minutes_id, drafted_by_engine, drafted_at, secretary_participant_id, \
    secretary_signed_at, chair_user_id, chair_approved_at, edit_count_vs_draft, content, \
    created_at, updated_at";

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    room_id: Uuid,
    minutes_no: String,
    meeting_title: Option<String>,
    status: String,
    version: i32,
    is_current: bool,
    previous_minutes_id: Option<Uuid>,
    drafted_by_engine: Option<String>,
    drafted_at: Option<DateTime<Utc>>,
    secretary_participant_id: Option<Uuid>,
    secretary_signed_at: Option<DateTime<Utc>>,
    chair_user_id: Option<Uuid>,
    chair_approved_at: Option<DateTime<Utc>>,
    edit_count_vs_draft: i32,
    content: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinutesView {
    id: Uuid,
    translation_room_id: Uuid,
    minutes_no: String,
    meeting_title: Option<String>,
    status: String,
    version: i32,
    is_current: bool,
    previous_minutes_id: Option<Uuid>,
    based_on_transcript_version: Option<i32>,
    drafted_by_engine: Option<String>,
    drafted_at: Option<DateTime<Utc>>,
    secretary_participant_id: Option<Uuid>,
    secretary_name: Option<String>,
    secretary_signed_at: Option<DateTime<Utc>>,
    chair_participant_id: Option<Uuid>,
    chair_name: Option<String>,
    chair_approved_at: Option<DateTime<Utc>>,
    edit_count_vs_draft: i32,
    content: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    can_edit: bool,
    can_designate_secretary: bool,
    can_approve: bool,
}

fn view(r: Row, info: &RoomAccessInfo) -> MinutesView {
    let manage = info.can_manage();
    let approved = r.status == "APPROVED";
    MinutesView {
        id: r.id,
        translation_room_id: r.room_id,
        minutes_no: r.minutes_no,
        meeting_title: r.meeting_title,
        can_edit: manage && !approved,
        can_designate_secretary: manage && r.secretary_signed_at.is_none() && !approved,
        can_approve: manage && !approved,
        status: r.status,
        version: r.version,
        is_current: r.is_current,
        previous_minutes_id: r.previous_minutes_id,
        based_on_transcript_version: None,
        drafted_by_engine: r.drafted_by_engine,
        drafted_at: r.drafted_at,
        secretary_participant_id: r.secretary_participant_id,
        secretary_name: None,
        secretary_signed_at: r.secretary_signed_at,
        chair_participant_id: r.chair_user_id,
        chair_name: None,
        chair_approved_at: r.chair_approved_at,
        edit_count_vs_draft: r.edit_count_vs_draft,
        content: r.content,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRequest {
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretaryRequest {
    participant_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct TranslationQuery {
    language: Option<String>,
}

/// Largest stored document; the web's own documents are a few KB.
const MAX_CONTENT: usize = 512 * 1024;

// ------------------------------------------------------------------ helpers

fn pool(s: &State_) -> &PgPool {
    s.api.service.repo().pool()
}

async fn access(s: &State_, room: RoomId, auth: &AuthUser) -> AppResult<RoomAccessInfo> {
    let info = s
        .api
        .access
        .room_access(room, auth.user_id())
        .await?
        .ok_or_else(|| AppError::not_found("meeting not found"))?;
    if !info.can_view() {
        return Err(AppError::forbidden(
            "not allowed to see this meeting's minutes",
        ));
    }
    Ok(info)
}

fn require_manage(info: &RoomAccessInfo) -> AppResult<()> {
    if info.can_manage() {
        Ok(())
    } else {
        Err(AppError::forbidden("only the host can change the minutes"))
    }
}

async fn current(s: &State_, info: &RoomAccessInfo, room: RoomId) -> AppResult<Option<Row>> {
    sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLS} FROM assistant.meeting_minutes \
         WHERE workspace_id = $1 AND room_id = $2 AND is_current"
    ))
    .bind(info.workspace_id.as_uuid())
    .bind(room.as_uuid())
    .fetch_optional(pool(s))
    .await
    .map_err(map_sqlx_error)
}

/// The current record, which must be `minutes_id` (a stale tab edits nothing).
async fn current_by_id(
    s: &State_,
    info: &RoomAccessInfo,
    room: RoomId,
    minutes_id: Uuid,
) -> AppResult<Row> {
    let row = current(s, info, room)
        .await?
        .ok_or_else(|| AppError::not_found("minutes not found"))?;
    if row.id != minutes_id {
        return Err(AppError::conflict(
            "MINUTES_NOT_CURRENT",
            "these minutes have been superseded; reload to see the current version",
        ));
    }
    Ok(row)
}

fn items(texts: &[String]) -> Value {
    Value::Array(texts.iter().map(|t| json!({ "text": t })).collect())
}

/// The web's `MeetingMinutesContent`, filled from the AI summary.
fn draft_content(summary: Option<&SummaryContent>) -> Value {
    let mut sections = Vec::new();
    if let Some(c) = summary {
        let text = if c.summary_detailed.trim().is_empty() {
            &c.summary_short
        } else {
            &c.summary_detailed
        };
        if !text.trim().is_empty() {
            sections.push(json!({ "key": "summary", "kind": "paragraph", "text": text }));
        }
        if !c.topics.is_empty() {
            sections.push(json!({ "key": "topics", "kind": "items", "items": items(&c.topics) }));
        }
        if !c.decisions.is_empty() {
            sections
                .push(json!({ "key": "decisions", "kind": "items", "items": items(&c.decisions) }));
        }
        if !c.action_items.is_empty() {
            let list: Vec<Value> = c
                .action_items
                .iter()
                .map(|a| {
                    let text = match &a.due {
                        Some(due) if !due.trim().is_empty() => format!("{} ({due})", a.task),
                        _ => a.task.clone(),
                    };
                    json!({ "text": text, "owner": a.owner })
                })
                .collect();
            sections.push(json!({ "key": "actionItems", "kind": "items", "items": list }));
        }
        if !c.open_questions.is_empty() {
            sections.push(json!({
                "key": "openQuestions", "kind": "items", "items": items(&c.open_questions)
            }));
        }
    }
    json!({
        "primaryLanguage": summary.map(|c| c.language.clone()).filter(|l| !l.is_empty()),
        "attendance": { "present": [], "absent": [], "invitedCount": 0, "presentCount": 0 },
        "sections": sections,
        "votes": [],
    })
}

// ------------------------------------------------------------------ handlers

async fn get_minutes(
    State(s): State<State_>,
    auth: AuthUser,
    Path(room): Path<Uuid>,
) -> AppResult<Json<MinutesView>> {
    let room = RoomId::from_uuid(room);
    let info = access(&s, room, &auth).await?;
    let row = current(&s, &info, room)
        .await?
        .ok_or_else(|| AppError::not_found("no minutes for this meeting yet"))?;
    Ok(Json(view(row, &info)))
}

async fn create_draft(
    State(s): State<State_>,
    auth: AuthUser,
    Path(room): Path<Uuid>,
) -> AppResult<Json<MinutesView>> {
    let room = RoomId::from_uuid(room);
    let info = access(&s, room, &auth).await?;
    require_manage(&info)?;
    if let Some(row) = current(&s, &info, room).await? {
        // Idempotent while a record exists; an approved one is changed through `revise`.
        return Ok(Json(view(row, &info)));
    }
    let summary = s
        .api
        .service
        .repo()
        .latest_completed(room)
        .await?
        .filter(|j| j.workspace_id == info.workspace_id);
    let content = draft_content(summary.as_ref().and_then(|j| j.content.as_ref()));
    let now = s.api.clock.now();
    let id = Uuid::now_v7();
    let short = room.as_uuid().simple().to_string();
    let minutes_no = format!(
        "BB-{}-{}",
        now.format("%Y%m%d"),
        short
            .get(short.len().saturating_sub(6)..)
            .unwrap_or("")
            .to_uppercase()
    );
    let row = sqlx::query_as::<_, Row>(&format!(
        "INSERT INTO assistant.meeting_minutes \
           (id, workspace_id, room_id, minutes_no, status, version, is_current, \
            based_on_summary_id, drafted_by_engine, drafted_at, content, created_by, \
            created_at, updated_at) \
         VALUES ($1, $2, $3, $4, 'DRAFT', 1, true, $5, $6, $7, $8, $9, $7, $7) \
         ON CONFLICT (room_id) WHERE is_current DO NOTHING \
         RETURNING {COLS}"
    ))
    .bind(id)
    .bind(info.workspace_id.as_uuid())
    .bind(room.as_uuid())
    .bind(&minutes_no)
    .bind(summary.as_ref().map(|j| j.id))
    .bind(summary.as_ref().and_then(|j| j.model.clone()))
    .bind(now)
    .bind(content.to_string())
    .bind(auth.user_id().as_uuid())
    .fetch_optional(pool(&s))
    .await
    .map_err(map_sqlx_error)?;
    let row = match row {
        Some(r) => r,
        // Lost a race with a second click: return the one that won.
        None => current(&s, &info, room)
            .await?
            .ok_or_else(|| AppError::internal("minutes draft vanished"))?,
    };
    Ok(Json(view(row, &info)))
}

async fn update_content(
    State(s): State<State_>,
    auth: AuthUser,
    Path((room, minutes_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateRequest>,
) -> AppResult<Json<MinutesView>> {
    let room = RoomId::from_uuid(room);
    let info = access(&s, room, &auth).await?;
    require_manage(&info)?;
    if req.content.len() > MAX_CONTENT {
        return Err(AppError::validation(
            "INVALID_REQUEST",
            "minutes content is too large",
        ));
    }
    if serde_json::from_str::<Value>(&req.content).is_err() {
        return Err(AppError::validation(
            "INVALID_REQUEST",
            "content must be a JSON document",
        ));
    }
    let row = current_by_id(&s, &info, room, minutes_id).await?;
    if row.status == "APPROVED" {
        return Err(AppError::conflict(
            "MINUTES_APPROVED",
            "approved minutes cannot be edited; open a revision",
        ));
    }
    let row = sqlx::query_as::<_, Row>(&format!(
        "UPDATE assistant.meeting_minutes \
         SET content = $3, edit_count_vs_draft = edit_count_vs_draft + 1, updated_at = $4 \
         WHERE workspace_id = $1 AND id = $2 RETURNING {COLS}"
    ))
    .bind(info.workspace_id.as_uuid())
    .bind(row.id)
    .bind(&req.content)
    .bind(s.api.clock.now())
    .fetch_one(pool(&s))
    .await
    .map_err(map_sqlx_error)?;
    Ok(Json(view(row, &info)))
}

async fn designate_secretary(
    State(s): State<State_>,
    auth: AuthUser,
    Path((room, minutes_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<SecretaryRequest>,
) -> AppResult<Json<MinutesView>> {
    let room = RoomId::from_uuid(room);
    let info = access(&s, room, &auth).await?;
    require_manage(&info)?;
    let row = current_by_id(&s, &info, room, minutes_id).await?;
    if row.secretary_signed_at.is_some() || row.status == "APPROVED" {
        return Err(AppError::conflict(
            "MINUTES_SIGNED",
            "the secretary cannot change after signing",
        ));
    }
    let row = sqlx::query_as::<_, Row>(&format!(
        "UPDATE assistant.meeting_minutes SET secretary_participant_id = $3, updated_at = $4 \
         WHERE workspace_id = $1 AND id = $2 RETURNING {COLS}"
    ))
    .bind(info.workspace_id.as_uuid())
    .bind(row.id)
    .bind(req.participant_id)
    .bind(s.api.clock.now())
    .fetch_one(pool(&s))
    .await
    .map_err(map_sqlx_error)?;
    Ok(Json(view(row, &info)))
}

async fn sign(
    State(s): State<State_>,
    auth: AuthUser,
    Path((room, minutes_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<MinutesView>> {
    let room = RoomId::from_uuid(room);
    let info = access(&s, room, &auth).await?;
    require_manage(&info)?;
    let row = current_by_id(&s, &info, room, minutes_id).await?;
    if row.status != "DRAFT" {
        return Err(AppError::conflict(
            "MINUTES_NOT_DRAFT",
            "only a draft can be signed",
        ));
    }
    let row = sqlx::query_as::<_, Row>(&format!(
        "UPDATE assistant.meeting_minutes \
         SET status = 'IN_REVIEW', secretary_signed_at = $3, updated_at = $3 \
         WHERE workspace_id = $1 AND id = $2 RETURNING {COLS}"
    ))
    .bind(info.workspace_id.as_uuid())
    .bind(row.id)
    .bind(s.api.clock.now())
    .fetch_one(pool(&s))
    .await
    .map_err(map_sqlx_error)?;
    Ok(Json(view(row, &info)))
}

async fn approve(
    State(s): State<State_>,
    auth: AuthUser,
    Path((room, minutes_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<MinutesView>> {
    let room = RoomId::from_uuid(room);
    let info = access(&s, room, &auth).await?;
    require_manage(&info)?;
    let row = current_by_id(&s, &info, room, minutes_id).await?;
    if row.status == "APPROVED" {
        return Ok(Json(view(row, &info)));
    }
    let now = s.api.clock.now();
    let row = sqlx::query_as::<_, Row>(&format!(
        "UPDATE assistant.meeting_minutes \
         SET status = 'APPROVED', chair_user_id = $3, chair_approved_at = $4, \
             secretary_signed_at = COALESCE(secretary_signed_at, $4), updated_at = $4 \
         WHERE workspace_id = $1 AND id = $2 RETURNING {COLS}"
    ))
    .bind(info.workspace_id.as_uuid())
    .bind(row.id)
    .bind(auth.user_id().as_uuid())
    .bind(now)
    .fetch_one(pool(&s))
    .await
    .map_err(map_sqlx_error)?;
    Ok(Json(view(row, &info)))
}

async fn revise(
    State(s): State<State_>,
    auth: AuthUser,
    Path((room, minutes_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<MinutesView>> {
    let room = RoomId::from_uuid(room);
    let info = access(&s, room, &auth).await?;
    require_manage(&info)?;
    let row = current_by_id(&s, &info, room, minutes_id).await?;
    if row.status != "APPROVED" {
        return Err(AppError::conflict(
            "MINUTES_NOT_APPROVED",
            "only approved minutes can be revised",
        ));
    }
    let now = s.api.clock.now();
    let mut tx = pool(&s).begin().await.map_err(map_sqlx_error)?;
    sqlx::query(
        "UPDATE assistant.meeting_minutes SET is_current = false, updated_at = $3 \
         WHERE workspace_id = $1 AND id = $2",
    )
    .bind(info.workspace_id.as_uuid())
    .bind(row.id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(map_sqlx_error)?;
    let next = sqlx::query_as::<_, Row>(&format!(
        "INSERT INTO assistant.meeting_minutes \
           (id, workspace_id, room_id, minutes_no, meeting_title, status, version, is_current, \
            previous_minutes_id, drafted_by_engine, drafted_at, secretary_participant_id, \
            content, created_by, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, 'DRAFT', $6, true, $7, $8, $9, $10, $11, $12, $13, $13) \
         RETURNING {COLS}"
    ))
    .bind(Uuid::now_v7())
    .bind(info.workspace_id.as_uuid())
    .bind(room.as_uuid())
    .bind(&row.minutes_no)
    .bind(&row.meeting_title)
    .bind(row.version + 1)
    .bind(row.id)
    .bind(&row.drafted_by_engine)
    .bind(row.drafted_at)
    .bind(row.secretary_participant_id)
    .bind(&row.content)
    .bind(auth.user_id().as_uuid())
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_sqlx_error)?;
    tx.commit().await.map_err(map_sqlx_error)?;
    Ok(Json(view(next, &info)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationView {
    language: String,
    sections: Option<Value>,
    status: &'static str,
    unavailable_reason: Option<&'static str>,
}

/// Reading the minutes in another language is not built yet; the web shows the reason.
async fn get_translation(
    State(s): State<State_>,
    auth: AuthUser,
    Path(room): Path<Uuid>,
    Query(q): Query<TranslationQuery>,
) -> AppResult<Json<TranslationView>> {
    access(&s, RoomId::from_uuid(room), &auth).await?;
    Ok(Json(TranslationView {
        language: q.language.unwrap_or_default(),
        sections: None,
        status: "unavailable",
        unavailable_reason: Some("Translated minutes are not available yet."),
    }))
}

async fn export_unavailable(
    State(s): State<State_>,
    auth: AuthUser,
    Path(room): Path<Uuid>,
) -> AppResult<Json<Value>> {
    access(&s, RoomId::from_uuid(room), &auth).await?;
    Err(AppError::unavailable(
        "minutes export is not available on this server yet",
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn empty_summary_gives_empty_document() {
        let v = draft_content(None);
        assert_eq!(v["sections"].as_array().unwrap().len(), 0);
        assert_eq!(v["attendance"]["presentCount"], 0);
    }
}
