use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use uuid::Uuid;
use warptalk_kernel::{AppResult, WorkspaceId};
use warptalk_web::AuthUser;

use super::AppState;
use super::dto::{
    ConfigResponse, CredentialDto, PutConfigRequest, PutCredentialRequest, TestResponse,
    parse_provider,
};
use crate::application::PutConfigCommand;

pub async fn get_config(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ConfigResponse>> {
    let v = s
        .llm_config
        .get_config(auth.user_id(), WorkspaceId::from_uuid(id))
        .await?;
    Ok(Json(ConfigResponse::new(&v, None)))
}

pub async fn put_config(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<PutConfigRequest>,
) -> AppResult<Json<ConfigResponse>> {
    let ack = req.acknowledge_workspace_controlled;
    let res = s
        .llm_config
        .put_config(
            auth.user_id(),
            WorkspaceId::from_uuid(id),
            PutConfigCommand {
                entries: req.into_entries()?,
                acknowledge_workspace_controlled: ack,
            },
        )
        .await?;
    Ok(Json(ConfigResponse::new(
        &res.view,
        Some(res.reindex_required),
    )))
}

pub async fn put_credential(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((id, provider)): Path<(Uuid, String)>,
    Json(req): Json<PutCredentialRequest>,
) -> AppResult<Json<CredentialDto>> {
    let meta = s
        .llm_config
        .put_credential(
            auth.user_id(),
            WorkspaceId::from_uuid(id),
            parse_provider(&provider)?,
            req.api_key,
            req.base_url,
        )
        .await?;
    Ok(Json(CredentialDto::from(&meta)))
}

pub async fn delete_credential(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((id, provider)): Path<(Uuid, String)>,
) -> AppResult<StatusCode> {
    s.llm_config
        .delete_credential(
            auth.user_id(),
            WorkspaceId::from_uuid(id),
            parse_provider(&provider)?,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn test_credential(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((id, provider)): Path<(Uuid, String)>,
) -> AppResult<Json<TestResponse>> {
    let o = s
        .llm_config
        .test_credential(
            auth.user_id(),
            WorkspaceId::from_uuid(id),
            parse_provider(&provider)?,
        )
        .await?;
    Ok(Json(TestResponse {
        ok: o.ok,
        latency_ms: o.latency_ms,
        error_code: o.error_code,
    }))
}
