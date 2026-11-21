//! Module construction and cross-module adapters.

use std::collections::HashMap;
use std::sync::Arc;

use crate::settings_http::{self, SettingsHttpState};
use async_trait::async_trait;
use axum::Router;
use axum::extract::Request;
use axum::http::Method;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use warptalk_ai_plane::ai::{AiProviders, PlatformKeys};
use warptalk_ai_plane::knowledge::{KnowledgeParts, build_knowledge_module, build_vector_store};
use warptalk_ai_plane::{
    BaseUrlPolicy, ByoDeps, LlmKeyValidator, PgExternalLlmPolicy, RedisConfigNotifier,
    vault_from_env,
};
use warptalk_assistant::application::ports::{
    ConfigNotifier, KeyValidator, NoopNotifier, SecretVault, WorkspaceAccess,
};
use warptalk_assistant::{AssistantDeps, AssistantModule};
use warptalk_bus::publish_json;
use warptalk_config::JwtConfig;
use warptalk_contracts::events::{DomainEventEnvelope, MEETING_PRODUCER};
use warptalk_identity::{
    IdentityDeps, IdentityModule, IdentityQueries, Policy, VerificationMailer,
};
use warptalk_kernel::{
    AppError, AppResult, Clock, RoomId, SystemClock, UserId, WorkspaceId, WorkspaceRole,
};
use warptalk_knowledge::application::ResultPublisher;
use warptalk_knowledge::infrastructure::{
    INDEX_RESULTS_STREAM, MemoryResultPublisher, RedisResultPublisher,
};
use warptalk_knowledge::{AccessScope, DocumentAccess};
use warptalk_livekit::{LiveKitConfig, NoopRoomService, RoomServiceClient};
use warptalk_meeting::application::ports::{
    BridgeInfo, CHAT_TRANSLATE_REQUESTS_STREAM, EVENT_CHAT_TRANSLATE, MeetingEvent,
    MeetingEventPublisher, NoopMeetingEventPublisher, ParticipantInfo, RoomDirectory, RoomInfo,
};
use warptalk_meeting::{DEFAULT_TOKEN_TTL, MeetingDeps, MeetingModule, RoomService};
use warptalk_qdrant::VectorStore;
use warptalk_room::RoomDeps;
use warptalk_room::RoomModule;
use warptalk_room::RoomQueries;
use warptalk_room::application::ports::{
    NoopRoomEventPublisher, NoopVoiceConsentDirectory, RoomEventPublisher, VoiceConsentDirectory,
    WorkspacePolicy,
};
use warptalk_room::infrastructure::redis_publisher::RedisRoomEventPublisher;
use warptalk_settings::{SettingsStore, namespaces};
use warptalk_transcript::application::glossary_ports::WorkspaceRoles;
use warptalk_transcript::application::ports::{
    AccessTarget, AlwaysSaveRoomPolicy, RoomDetails, RoomHost, StaticAiPolicy, TranscriptReadAccess,
};
use warptalk_transcript::{TranscriptDeps, TranscriptModule};
use warptalk_web::JwtKeys;
use warptalk_workspace::application::ports::UserDirectory;
use warptalk_workspace::{
    DEFAULT_INVITATION_TTL_DAYS, InvitationMailer, LogInvitationMailer, UserSummary, WorkspaceDeps,
    WorkspaceModule, WorkspaceQueries,
};

/// Adapter: workspace's `UserDirectory` port backed by identity's read facade.
pub struct IdentityUserDirectory(pub IdentityQueries);

#[async_trait]
impl UserDirectory for IdentityUserDirectory {
    async fn summaries(&self, ids: &[UserId]) -> AppResult<HashMap<UserId, UserSummary>> {
        let mut out = HashMap::with_capacity(ids.len());
        // TODO batch query
        for id in ids {
            if let Some(u) = self.0.user_summary(*id).await? {
                out.insert(
                    *id,
                    UserSummary {
                        email: u.email,
                        full_name: u.full_name,
                        avatar_url: u.avatar_url,
                    },
                );
            }
        }
        Ok(out)
    }
}

/// Adapter: room's `WorkspacePolicy` port backed by workspace's read facade.
pub struct WorkspacePolicyAdapter(pub WorkspaceQueries);

#[async_trait]
impl WorkspacePolicy for WorkspacePolicyAdapter {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        self.0.member_role(ws, user).await
    }

    async fn is_active(&self, ws: WorkspaceId) -> AppResult<bool> {
        self.0.is_active(ws).await
    }
}

/// Adapter: meeting's `RoomDirectory` port backed by room's read facade.
pub struct RoomDirectoryAdapter(pub RoomQueries);

#[async_trait]
impl RoomDirectory for RoomDirectoryAdapter {
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomInfo>> {
        Ok(self.0.room_summary(room).await?.map(|r| RoomInfo {
            workspace_id: r.workspace_id,
            host_id: r.effective_host_id,
            status: r.status,
        }))
    }

    async fn participant(&self, room: RoomId, user: UserId) -> AppResult<Option<ParticipantInfo>> {
        Ok(self
            .0
            .participant_summary(room, user)
            .await?
            .map(|p| ParticipantInfo {
                status: p.status,
                display_name: p.display_name,
            }))
    }

    async fn end_room(&self, room: RoomId, actor: UserId) -> AppResult<()> {
        self.0.end_room(room, actor).await
    }

    async fn bridge(&self, room: RoomId) -> AppResult<Option<BridgeInfo>> {
        Ok(self.0.bridge_info(room).await?.map(|b| BridgeInfo {
            provider: b.provider,
            capturer_user_id: b.capturer_user_id,
            capturer_stale: b.capturer_stale,
        }))
    }
}

/// Adapter: knowledge's `DocumentAccess` port backed by workspace roles.
///
/// Owners and admins get the role (the knowledge module treats them as unscoped). Members get
/// empty allowlists (fail closed).
/// TODO: members' documents once workspace documents exist; members' meetings = rooms they
/// participated in (needs a `RoomQueries` method listing a user's rooms in a workspace).
pub struct KnowledgeAccessAdapter(pub WorkspaceQueries);

#[async_trait]
impl DocumentAccess for KnowledgeAccessAdapter {
    async fn access_scope(&self, ws: WorkspaceId, user: UserId) -> AppResult<AccessScope> {
        let role = self
            .0
            .member_role(ws, user)
            .await?
            .ok_or_else(|| AppError::forbidden("not a member of this workspace"))?;
        Ok(AccessScope {
            role,
            allowed_document_ids: Vec::new(),
            allowed_meeting_ids: Vec::new(),
        })
    }
}

/// Adapter: assistant's `WorkspaceAccess` port backed by workspace roles.
pub struct AssistantWorkspaceAccess(pub WorkspaceQueries);

#[async_trait]
impl WorkspaceAccess for AssistantWorkspaceAccess {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        self.0.member_role(ws, user).await
    }
}

/// Adapter: transcript's `WorkspaceRoles` port (glossary RBAC) backed by workspace roles.
pub struct TranscriptWorkspaceRoles(pub WorkspaceQueries);

#[async_trait]
impl WorkspaceRoles for TranscriptWorkspaceRoles {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        self.0.member_role(ws, user).await
    }
}

/// Adapter: transcript's room ports (directory, effective host, read access) backed by room's
/// read facade and workspace roles. Read access: workspace owner/admin or a participant of the room.
pub struct TranscriptRoomAdapter {
    pub rooms: RoomQueries,
    pub workspaces: WorkspaceQueries,
}

#[async_trait]
impl warptalk_transcript::application::ports::RoomDirectory for TranscriptRoomAdapter {
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomDetails>> {
        Ok(self
            .rooms
            .room_workspace(room)
            .await?
            .map(|workspace_id| RoomDetails { workspace_id }))
    }
}

#[async_trait]
impl RoomHost for TranscriptRoomAdapter {
    async fn effective_host(&self, room: RoomId) -> AppResult<Option<UserId>> {
        self.rooms.effective_host(room).await
    }
}

#[async_trait]
impl TranscriptReadAccess for TranscriptRoomAdapter {
    async fn can_read(&self, user: UserId, target: AccessTarget) -> AppResult<bool> {
        match self
            .workspaces
            .member_role(target.workspace_id, user)
            .await?
        {
            Some(WorkspaceRole::Owner | WorkspaceRole::Admin) => Ok(true),
            _ => Ok(self
                .rooms
                .participant_summary(target.room_id, user)
                .await?
                .is_some()),
        }
    }
}

/// Lifetime of `translationRoom:{id}:workspace` (refreshed on every routes update).
const ROOM_WORKSPACE_TTL_SECS: u64 = 172_800;

/// Decorator over the room event publisher: before a room's routes or start command go out, it
/// writes `translationRoom:{id}:workspace` so the pipeline (no database) can resolve the room's
/// workspace glossary. Best effort: a failure is logged and the event still goes out.
pub struct RoomWorkspaceKeyPublisher {
    inner: Arc<dyn RoomEventPublisher>,
    conn: ConnectionManager,
    rooms: Arc<std::sync::OnceLock<RoomQueries>>,
}

impl RoomWorkspaceKeyPublisher {
    pub fn new(
        inner: Arc<dyn RoomEventPublisher>,
        conn: ConnectionManager,
        rooms: Arc<std::sync::OnceLock<RoomQueries>>,
    ) -> Self {
        Self { inner, conn, rooms }
    }

    async fn remember(&self, room: RoomId) {
        let Some(rooms) = self.rooms.get() else {
            return;
        };
        let res = async {
            let Some(ws) = rooms.room_workspace(room).await? else {
                return Ok(());
            };
            let mut conn = self.conn.clone();
            redis::cmd("SET")
                .arg(format!("translationRoom:{room}:workspace"))
                .arg(ws.to_string())
                .arg("EX")
                .arg(ROOM_WORKSPACE_TTL_SECS)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| AppError::unavailable(e.to_string()))
        }
        .await;
        if let Err(e) = res {
            tracing::warn!(%room, error = %e, "could not record the room's workspace for the pipeline");
        }
    }
}

#[async_trait]
impl RoomEventPublisher for RoomWorkspaceKeyPublisher {
    async fn publish_command(
        &self,
        cmd: &warptalk_contracts::events::RoomCommand,
    ) -> AppResult<()> {
        if let warptalk_contracts::events::RoomCommand::RoomStarted { state, .. } = cmd {
            self.remember(state.room_id).await;
        }
        self.inner.publish_command(cmd).await
    }

    async fn publish_routes(
        &self,
        room: RoomId,
        snapshot: &warptalk_contracts::events::AudioRoutesSnapshot,
    ) -> AppResult<()> {
        self.remember(room).await;
        self.inner.publish_routes(room, snapshot).await
    }
}

/// Meeting events on Redis pub/sub; the channel is named by the event type.
pub struct RedisMeetingEventPublisher {
    conn: ConnectionManager,
}

impl RedisMeetingEventPublisher {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl MeetingEventPublisher for RedisMeetingEventPublisher {
    async fn publish(&self, event: MeetingEvent) -> AppResult<()> {
        let envelope = DomainEventEnvelope {
            event_id: uuid::Uuid::now_v7(),
            event_type: event.event_type.to_owned(),
            schema_version: 1,
            producer: MEETING_PRODUCER.to_owned(),
            occurred_at: chrono::Utc::now(),
            payload: event.payload,
        };
        let mut conn = self.conn.clone();
        if event.event_type == EVENT_CHAT_TRANSLATE {
            // Chat translation requests are durable work for the worker: a stream, not pub/sub.
            return warptalk_bus::xadd_json(
                &mut conn,
                CHAT_TRANSLATE_REQUESTS_STREAM,
                &envelope,
                10_000,
            )
            .await
            .map(|_| ())
            .map_err(|e| AppError::unavailable(format!("event bus unavailable: {e}")));
        }
        publish_json(&mut conn, event.event_type, &envelope)
            .await
            .map(|_| ())
            .map_err(|e| AppError::unavailable(format!("event bus unavailable: {e}")))
    }
}

/// LiveKit connection settings from the environment (`LIVEKIT_URL`, `LIVEKIT_API_KEY`,
/// `LIVEKIT_API_SECRET`).
#[derive(Clone)]
pub struct LiveKitSettings {
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
}

impl LiveKitSettings {
    /// `None` unless all three variables are set and non-empty.
    pub fn from_env() -> Option<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Some(Self {
            url: get("LIVEKIT_URL")?,
            api_key: get("LIVEKIT_API_KEY")?,
            api_secret: get("LIVEKIT_API_SECRET")?,
        })
    }

    pub fn into_config(self) -> LiveKitConfig {
        LiveKitConfig {
            url: self.url,
            api_key: self.api_key,
            api_secret: self.api_secret,
        }
    }
}

/// Optional wiring inputs beyond the core dependencies.
#[derive(Default)]
pub struct ApiOptions {
    /// `None`: meeting routes mount, but token-minting routes answer 503 `LIVEKIT_NOT_CONFIGURED`.
    pub livekit: Option<LiveKitConfig>,
    /// Overrides the media-server client (tests); default is the real client when `livekit` is set.
    pub room_service: Option<Arc<dyn RoomService>>,
    /// Redis URL for the settings invalidation listener (`None`: no listener).
    pub redis_url: Option<String>,
    /// Overrides platform AI provider resolution (tests); default reads env keys + settings.
    pub ai: Option<Arc<AiProviders>>,
    /// Overrides the vector store (tests); default is Qdrant when `QDRANT_URL` is set.
    pub vectors: Option<Arc<dyn VectorStore>>,
    /// Overrides the BYO key vault (tests); default reads `AI_KEYS_KEK` (BYO off when unset).
    pub vault: Option<Arc<dyn SecretVault>>,
    /// Overrides BYO key validation (tests); default is the real provider check with the
    /// strict `baseUrl` guard (`AI_ALLOW_INSECURE_BASE_URL` relaxes it).
    pub key_validator: Option<Arc<dyn KeyValidator>>,
    /// Workspace invitation mailer; `None` uses `LogInvitationMailer` (tokens are not sent).
    pub invitation_mailer: Option<Arc<dyn InvitationMailer>>,
}

/// Rejects the routes that mint LiveKit tokens when LiveKit is not configured.
async fn livekit_required(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if req.method() == Method::POST && (path.ends_with("/join") || path.ends_with("/bridge-token"))
    {
        return AppError::coded(503, "LIVEKIT_NOT_CONFIGURED", "LiveKit is not configured")
            .into_response();
    }
    next.run(req).await
}

/// Build the full API router (`/health` plus modules under `/api/v1`).
///
/// `redis` is the event-bus connection; `None` (tests, Redis-less runs) drops room events.
pub async fn build(
    pool: PgPool,
    jwt: JwtKeys,
    jwt_cfg: &JwtConfig,
    cors_origins: &[String],
    mailer: Arc<dyn VerificationMailer>,
    redis: Option<ConnectionManager>,
    opts: ApiOptions,
) -> Router {
    let results: Arc<dyn ResultPublisher> = match &redis {
        Some(conn) => Arc::new(RedisResultPublisher::new(
            conn.clone(),
            INDEX_RESULTS_STREAM,
        )),
        None => Arc::new(MemoryResultPublisher::new()),
    };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let refresh_secs = i64::try_from(jwt_cfg.refresh_ttl_seconds).unwrap_or(i64::from(i32::MAX));
    let policy = Policy {
        refresh_ttl: chrono::Duration::seconds(refresh_secs),
        ..Policy::default()
    };

    let identity = IdentityModule::new(IdentityDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: clock.clone(),
        mailer,
        policy,
    });
    let invitation_mailer = opts
        .invitation_mailer
        .unwrap_or_else(|| Arc::new(LogInvitationMailer));
    let workspace = WorkspaceModule::with_invitations(
        WorkspaceDeps {
            pool: pool.clone(),
            jwt: jwt.clone(),
            clock: clock.clone(),
            users: Arc::new(IdentityUserDirectory(identity.queries())),
        },
        invitation_mailer,
        DEFAULT_INVITATION_TTL_DAYS,
    );

    let consent: Arc<dyn VoiceConsentDirectory> = Arc::new(NoopVoiceConsentDirectory); // TODO: identity voice consent
    let meeting_events: Arc<dyn MeetingEventPublisher> = match &redis {
        Some(conn) => Arc::new(RedisMeetingEventPublisher::new(conn.clone())),
        None => Arc::new(NoopMeetingEventPublisher),
    };
    let settings = SettingsStore::new(pool.clone(), redis.clone());
    namespaces::register_defaults(&settings);
    if let Some(url) = &opts.redis_url {
        settings.spawn_invalidation_listener(url);
    }
    let ai_notifier: Arc<dyn ConfigNotifier> = match &redis {
        Some(conn) => Arc::new(RedisConfigNotifier::new(conn.clone())),
        None => Arc::new(NoopNotifier),
    };
    let transcript_redis = redis.clone();
    let room_queries_cell: Arc<std::sync::OnceLock<RoomQueries>> = Arc::default();
    let events: Arc<dyn RoomEventPublisher> = match redis {
        Some(conn) => Arc::new(RoomWorkspaceKeyPublisher::new(
            Arc::new(RedisRoomEventPublisher::new(conn.clone())),
            conn,
            room_queries_cell.clone(),
        )),
        None => Arc::new(NoopRoomEventPublisher),
    };
    let room = RoomModule::new(RoomDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: clock.clone(),
        workspace: Arc::new(WorkspacePolicyAdapter(workspace.queries())),
        consent,
        events,
    });

    let _ = room_queries_cell.set(room.queries());
    let transcript = transcript_redis.map(|conn| {
        let rooms = Arc::new(TranscriptRoomAdapter {
            rooms: room.queries(),
            workspaces: workspace.queries(),
        });
        TranscriptModule::new(TranscriptDeps {
            pool: pool.clone(),
            redis: conn,
            jwt: jwt.clone(),
            clock: Arc::new(SystemClock),
            rooms: rooms.clone(),
            // TODO: wire room SaveTranscript and the workspace AI policy once those facades exist.
            room_policy: Arc::new(AlwaysSaveRoomPolicy),
            workspace_ai: Arc::new(StaticAiPolicy {
                retrieval: false,
                external_llm: false,
            }),
            host: rooms.clone(),
            read_access: rooms,
        })
        .with_workspace_roles(Arc::new(TranscriptWorkspaceRoles(workspace.queries())))
    });

    let configured = opts.livekit.is_some();
    let livekit_cfg = opts.livekit.unwrap_or_else(|| LiveKitConfig {
        url: String::new(),
        api_key: String::new(),
        api_secret: String::new(),
    });
    let room_service: Arc<dyn RoomService> = match (opts.room_service, configured) {
        (Some(rs), _) => rs,
        (None, true) => match RoomServiceClient::new(livekit_cfg.clone()) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                tracing::error!(error = %e, "livekit room service client unavailable");
                Arc::new(NoopRoomService)
            }
        },
        (None, false) => Arc::new(NoopRoomService),
    };
    let meeting = MeetingModule::new(MeetingDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock,
        livekit: livekit_cfg,
        rooms: Arc::new(RoomDirectoryAdapter(room.queries())),
        room_service,
        events: meeting_events,
        token_ttl: DEFAULT_TOKEN_TTL,
    });
    let mut meeting_router = meeting.router();
    if !configured {
        meeting_router = meeting_router.layer(middleware::from_fn(livekit_required));
    }
    let vault = opts.vault.unwrap_or_else(vault_from_env);
    let assistant = AssistantModule::new(AssistantDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: Arc::new(SystemClock),
        access: Arc::new(AssistantWorkspaceAccess(workspace.queries())),
        vault: vault.clone(),
        validator: opts
            .key_validator
            .unwrap_or_else(|| Arc::new(LlmKeyValidator::new(BaseUrlPolicy::from_env()))),
        notifier: ai_notifier,
    });
    let ai = opts.ai.unwrap_or_else(|| {
        AiProviders::with_byo(
            settings.clone(),
            PlatformKeys::from_env(),
            ByoDeps::new(
                assistant.llm_config_repository(),
                vault,
                Arc::new(PgExternalLlmPolicy::new(pool.clone())),
            ),
        )
    });
    if let Some(url) = &opts.redis_url {
        // Config writes on any replica apply here at once.
        ai.spawn_invalidation_listener(url);
    }
    let vectors = opts.vectors.unwrap_or_else(|| {
        build_vector_store().unwrap_or_else(|e| {
            tracing::error!(error = %e, "vector store unavailable; using in-memory");
            Arc::new(warptalk_qdrant::InMemoryVectorStore::new())
        })
    });
    let knowledge = build_knowledge_module(KnowledgeParts {
        pool: pool.clone(),
        jwt: jwt.clone(),
        settings: settings.clone(),
        vectors,
        ai,
        access: Arc::new(KnowledgeAccessAdapter(workspace.queries())),
        results,
    })
    .await;
    let settings_router = settings_http::router(SettingsHttpState {
        store: settings,
        pool,
        jwt,
        workspaces: workspace.queries(),
        rooms: room.queries(),
    });

    let api = Router::new()
        .merge(identity.router())
        .merge(workspace.router())
        .merge(room.router())
        .merge(meeting_router)
        .merge(settings_router)
        .merge(assistant.router())
        .merge(knowledge.router());
    // Transcript routes (glossaries) need Redis for the pipeline snapshots; without Redis they are not mounted.
    let api = match transcript {
        Some(t) => api.merge(t.router()),
        None => api,
    };
    let router = Router::new()
        .route("/health", get(warptalk_web::health))
        .nest("/api/v1", api);
    warptalk_web::common_layers(router, cors_origins)
}
