//! Module construction and cross-module adapters.

use std::collections::HashMap;
use std::sync::Arc;

use crate::assistant_chat::{AiAssistantLlm, KnowledgeSearch, PolicyConsent};
use crate::features_http::{self, FeaturesState};
use crate::room_effects::{
    RoomAiPolicyRefresher, RoomEffectsHandles, RoomEffectsPublisher, init_copilot_room_keys,
};
use crate::settings_http::{self, SettingsHttpState};
use crate::suggestions_http::{self, SuggestionsState};
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
use warptalk_assistant::application::chat_ports::ChatEventBus;
use warptalk_assistant::application::ports::{
    ConfigNotifier, KeyValidator, NoopNotifier, SecretVault, WorkspaceAccess,
};
use warptalk_assistant::infrastructure::{InMemoryChatEventBus, RedisChatEventBus};
use warptalk_assistant::summary::{
    ComposedRoomAccess, FollowUpSource, NoBus, NoFollowUps, PgSummaryRepository,
    PgTranscriptSource, RedisFollowUpSource, RedisSummaryBus, SummaryBus, SummaryDeps,
    SummaryService,
};
use warptalk_assistant::{AssistantDeps, AssistantModule, ChatDeps, SummaryHttpDeps};
use warptalk_billing::{BillingDeps, BillingModule};
use warptalk_bus::publish_json;
use warptalk_config::JwtConfig;
use warptalk_contracts::events::{DomainEventEnvelope, MEETING_PRODUCER};
use warptalk_copilot::application::ports::{
    CreatedRoom, Metering, NewSessionRoom, NoSttTokenIssuer, RoomState, SessionRooms,
    SttTokenIssuer, WorkspaceMembers,
};
use warptalk_copilot::infrastructure::openai_stt::{OpenAiSttConfig, OpenAiSttTokenIssuer};
use warptalk_copilot::infrastructure::redis_adapters::{RedisRateLimiter, RedisSegmentSink};
use warptalk_copilot::{CopilotDeps, CopilotModule};
use warptalk_identity::{
    CartesiaVoice, IdentityDeps, IdentityModule, IdentityQueries, Policy, RedisVoiceStatePublisher,
    VerificationMailer, VoiceDeps,
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
use warptalk_room::CreateCopilotRoom;
use warptalk_room::RoomDeps;
use warptalk_room::RoomModule;
use warptalk_room::RoomQueries;
use warptalk_room::application::ports::{
    NoopRoomEventPublisher, RoomEventPublisher, VoiceConsentDirectory, WorkspacePolicy,
};
use warptalk_room::infrastructure::redis_publisher::RedisRoomEventPublisher;
use warptalk_settings::{SettingsStore, namespaces};
use warptalk_storage::ObjectStore;
use warptalk_transcript::application::glossary_ports::WorkspaceRoles;
use warptalk_transcript::application::ports::{
    AccessTarget, AlwaysSaveRoomPolicy, RoomDetails, RoomHost, TranscriptReadAccess,
    WorkspaceAiPolicy,
};
use warptalk_transcript::{TranscriptDeps, TranscriptModule};
use warptalk_web::JwtKeys;
use warptalk_workspace::application::ports::UserDirectory;
use warptalk_workspace::{
    DEFAULT_INVITATION_TTL_DAYS, DocumentsConfig, DocumentsDeps, InvitationMailer,
    LogInvitationMailer, NoopDocumentIndexPublisher, RedisDocumentIndexPublisher, UserSummary,
    WorkspaceDeps, WorkspaceModule, WorkspaceQueries,
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

/// Adapter: knowledge's `DocumentAccess` port backed by workspace roles and documents.
///
/// Owners and admins get the role (the knowledge module treats them as unscoped). Members get
/// the documents they may see: active, AI-allowed, indexed documents whose access level is not
/// `ADMIN_ONLY` (spec 14 section 4.2; at most 500). Any failure surfaces as an error (fail
/// closed).
/// TODO: members' meetings = rooms they participated in (needs a `RoomQueries` method listing a
/// user's rooms in a workspace).
pub struct KnowledgeAccessAdapter(pub WorkspaceQueries);

#[async_trait]
impl DocumentAccess for KnowledgeAccessAdapter {
    async fn access_scope(&self, ws: WorkspaceId, user: UserId) -> AppResult<AccessScope> {
        let role = self
            .0
            .member_role(ws, user)
            .await?
            .ok_or_else(|| AppError::forbidden("not a member of this workspace"))?;
        let allowed_document_ids = match role {
            // Unscoped: the allowlist is ignored.
            WorkspaceRole::Owner | WorkspaceRole::Admin => Vec::new(),
            WorkspaceRole::Member => self.0.ai_retrievable_document_ids(ws, role).await?,
        };
        Ok(AccessScope {
            role,
            allowed_document_ids,
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

/// Adapter: transcript's `WorkspaceAiPolicy` backed by the workspace AI flags (B-13).
pub struct TranscriptWorkspaceAiPolicy(pub WorkspaceQueries);

#[async_trait]
impl WorkspaceAiPolicy for TranscriptWorkspaceAiPolicy {
    async fn ai_retrieval_allowed(&self, ws: WorkspaceId) -> AppResult<bool> {
        Ok(self.0.ai_policy(ws).await?.ai_retrieval_allowed)
    }

    async fn external_llm_allowed(&self, ws: WorkspaceId) -> AppResult<bool> {
        Ok(self.0.ai_policy(ws).await?.external_llm_allowed)
    }
}

/// Adapter: copilot's `WorkspaceMembers` port backed by workspace roles.
pub struct CopilotMembers(pub WorkspaceQueries);

#[async_trait]
impl WorkspaceMembers for CopilotMembers {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        self.0.member_role(ws, user).await
    }
}

/// Adapter: copilot's `SessionRooms` port backed by room's facade. Creating a session room also
/// writes the Redis keys the assistant needs (`ai_policy`, reader language); ending goes through
/// the room lifecycle, whose `RoomEnded` command the effects decorator turns into the
/// `__MEETING_END__` marker on `stt:results`.
pub struct CopilotRooms {
    pub rooms: RoomQueries,
    pub redis: ConnectionManager,
    pub effects: RoomEffectsHandles,
}

#[async_trait]
impl SessionRooms for CopilotRooms {
    async fn create(&self, user: UserId, new: NewSessionRoom) -> AppResult<CreatedRoom> {
        let host_language = new.host_language.clone();
        let room = self
            .rooms
            .create_copilot_room(
                user,
                CreateCopilotRoom {
                    workspace_id: new.workspace_id,
                    title: new.title,
                    host_display_name: new.host_display_name,
                    host_language: new.host_language,
                    other_language: new.other_language,
                },
            )
            .await?;
        init_copilot_room_keys(
            &self.redis,
            &self.effects,
            room.room_id,
            room.host_participant_id.as_uuid(),
            &host_language,
        )
        .await;
        Ok(CreatedRoom {
            room_id: room.room_id,
            host_participant_id: room.host_participant_id.as_uuid(),
            started_at: room.started_at,
        })
    }

    async fn state(&self, room: RoomId) -> AppResult<Option<RoomState>> {
        Ok(self
            .rooms
            .room_summary(room)
            .await?
            // Only copilot rooms are sessions, whatever the id the caller guessed.
            .filter(|r| r.room_type == "COPILOT_SESSION")
            .map(|r| RoomState {
                workspace_id: r.workspace_id,
                host_id: r.host_id,
                status: r.status,
                started_at: r.started_at,
                ended_at: r.ended_at,
            }))
    }

    async fn end(&self, room: RoomId, actor: UserId) -> AppResult<()> {
        self.rooms.end_room(room, actor).await
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

/// The config the API itself uses to call LiveKit (Twirp). `LIVEKIT_URL` is the address handed to
/// clients; inside Docker that is the host's `localhost`, which the API container cannot reach, so
/// `LIVEKIT_API_URL` (e.g. `http://livekit:7880`) overrides it for server calls when set.
fn server_side_livekit(cfg: &LiveKitConfig) -> LiveKitConfig {
    let mut c = cfg.clone();
    if let Some(u) = std::env::var("LIVEKIT_API_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        c.url = u;
    }
    c
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
    /// Overrides the document object store (tests); default is built from `STORAGE_PROVIDER`
    /// (`local` | `s3`). When neither works, document routes answer 503.
    pub object_store: Option<Arc<dyn ObjectStore>>,
    /// Overrides the document upload settings (tests); default reads `DOCUMENT_MAX_UPLOAD_MB`.
    pub documents_config: Option<DocumentsConfig>,
    /// `CARTESIA_API_KEY`: enables voice cloning and provider-side voice deletion. `None`:
    /// `POST /me/voice/enroll` answers 503 `VOICE_CLONING_UNAVAILABLE`.
    pub cartesia_api_key: Option<String>,
    /// Overrides the voice cloner / provider admin (tests); default is Cartesia when keyed.
    pub voice_cloner: Option<Arc<dyn warptalk_identity::VoiceCloner>>,
    pub voice_admin: Option<Arc<dyn warptalk_identity::VoiceProviderAdmin>>,
    /// Dev suggestion test mode (`warptalk_config::TestModeConfig`): mounts the transcript
    /// injection route. Never set in production (the config refuses it there).
    pub test_mode: bool,
    /// Overrides the ephemeral STT token issuer (tests); default is OpenAI when `OPENAI_API_KEY`
    /// is set, else `POST /copilot/sessions/{id}/stt-token` answers 503 `STT_NOT_CONFIGURED`.
    pub stt_token_issuer: Option<Arc<dyn SttTokenIssuer>>,
    /// Overrides the server-side STT of the copilot audio fallback socket (tests); default from
    /// `COPILOT_SERVER_STT` / `OPENAI_API_KEY` (see `copilot_stt`), else the socket answers 503.
    pub copilot_server_stt: Option<Arc<dyn warptalk_copilot::application::ports::ServerStt>>,
    /// Overrides the audio socket limits (tests); default reads `COPILOT_AUDIO_*` env.
    pub copilot_audio_config: Option<warptalk_copilot::application::audio::AudioConfig>,
    /// Overrides the billing config (tests); default reads `STRIPE_*` / `BILLING_*` env.
    pub billing_config: Option<warptalk_billing::application::service::BillingConfig>,
}

/// Room's consent port backed by identity's voice-consent facade (BR-94).
pub struct IdentityConsentDirectory(pub IdentityQueries);

#[async_trait]
impl VoiceConsentDirectory for IdentityConsentDirectory {
    async fn has_voice_clone_consent(&self, user: UserId) -> AppResult<bool> {
        self.0.voice_clone_consent(user).await
    }
}

fn voice_deps(opts: &ApiOptions, redis: Option<&ConnectionManager>) -> VoiceDeps {
    let mut deps = VoiceDeps::default();
    if let Some(key) = opts
        .cartesia_api_key
        .as_deref()
        .filter(|k| !k.trim().is_empty())
    {
        match CartesiaVoice::new(key) {
            Ok(c) => {
                let c = Arc::new(c);
                deps.cloner = c.clone();
                deps.admin = c;
            }
            Err(e) => tracing::error!(error = %e, "cartesia voice adapter unavailable"),
        }
    }
    if let Some(c) = &opts.voice_cloner {
        deps.cloner = c.clone();
    }
    if let Some(a) = &opts.voice_admin {
        deps.admin = a.clone();
    }
    if let Some(conn) = redis {
        deps.publisher = Arc::new(RedisVoiceStatePublisher::new(conn.clone()));
    }
    deps
}

/// The object store for workspace documents: the override, else `STORAGE_PROVIDER` (`local`
/// default | `s3`). `None` (logged) leaves the document routes answering 503.
fn document_store(over: Option<Arc<dyn ObjectStore>>) -> Option<Arc<dyn ObjectStore>> {
    if over.is_some() {
        return over;
    }
    let built = warptalk_storage::StorageConfig::from_env()
        .and_then(|cfg| warptalk_storage::build_store(&cfg));
    match built {
        Ok(store) => Some(store),
        Err(e) => {
            tracing::error!(error = %e, "object storage unavailable: document routes answer 503");
            None
        }
    }
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

    let identity = IdentityModule::with_extras(
        IdentityDeps {
            pool: pool.clone(),
            jwt: jwt.clone(),
            clock: clock.clone(),
            mailer,
            policy,
            voice: voice_deps(&opts, redis.as_ref()),
        },
        crate::auth_wiring::identity_extras(redis.as_ref()),
    );
    let notification =
        warptalk_notification::NotificationModule::new(warptalk_notification::NotificationDeps {
            pool: pool.clone(),
            jwt: jwt.clone(),
            clock: clock.clone(),
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
    let workspace = match document_store(opts.object_store) {
        Some(store) => workspace.with_documents(DocumentsDeps {
            store,
            publisher: match &redis {
                Some(conn) => Arc::new(RedisDocumentIndexPublisher::new(conn.clone())),
                None => Arc::new(NoopDocumentIndexPublisher),
            },
            config: opts
                .documents_config
                .unwrap_or_else(DocumentsConfig::from_env),
        }),
        None => workspace,
    };

    let consent: Arc<dyn VoiceConsentDirectory> =
        Arc::new(IdentityConsentDirectory(identity.queries()));
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
    let configured = opts.livekit.is_some();
    let livekit_cfg = opts.livekit.unwrap_or_else(|| LiveKitConfig {
        url: String::new(),
        api_key: String::new(),
        api_secret: String::new(),
    });
    let room_service: Arc<dyn RoomService> = match (opts.room_service, configured) {
        (Some(rs), _) => rs,
        (None, true) => match RoomServiceClient::new(server_side_livekit(&livekit_cfg)) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                tracing::error!(error = %e, "livekit room service client unavailable");
                Arc::new(NoopRoomService)
            }
        },
        (None, false) => Arc::new(NoopRoomService),
    };
    let transcript_redis = redis.clone();
    let suggestions_redis = redis.clone();
    let chat_redis = redis.clone();
    let copilot_redis = redis.clone();
    let summary_redis = redis.clone();
    let billing_redis = redis.clone();
    let effects = RoomEffectsHandles::default();
    let policy_conn = redis.clone();
    let room_queries_cell: Arc<std::sync::OnceLock<RoomQueries>> = Arc::default();
    let events: Arc<dyn RoomEventPublisher> = match redis {
        Some(conn) => Arc::new(RoomEffectsPublisher::new(
            Arc::new(RoomWorkspaceKeyPublisher::new(
                Arc::new(RedisRoomEventPublisher::new(conn.clone())),
                conn.clone(),
                room_queries_cell.clone(),
            )),
            conn,
            room_service.clone(),
            effects.clone(),
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
    // Bridge rooms whose capture went silent (meeting closed, app quit) end on their own so they
    // never stay "In Progress" and their summary gets written.
    {
        let room = room.queries();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                match room
                    .end_idle_bridge_rooms(std::time::Duration::from_secs(10 * 60))
                    .await
                {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(ended = n, "auto-ended idle bridge rooms"),
                    Err(e) => tracing::warn!(error = %e, "idle bridge sweep failed"),
                }
            }
        });
    }
    let _ = effects.rooms.set(room.queries());
    let _ = effects.workspaces.set(workspace.queries());
    if let Some(conn) = policy_conn {
        // B-12/B-13: a settings write refreshes `ai_policy` of the workspace's live rooms.
        workspace.set_settings_observer(Arc::new(RoomAiPolicyRefresher {
            conn,
            handles: effects.clone(),
        }));
    }
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
            workspace_ai: Arc::new(TranscriptWorkspaceAiPolicy(workspace.queries())),
            host: rooms.clone(),
            read_access: rooms,
        })
        .with_workspace_roles(Arc::new(TranscriptWorkspaceRoles(workspace.queries())))
    });

    // Copilot sessions (ADR 0001) write `stt:results`, so they need Redis like the transcript routes.
    let stt_tokens: Arc<dyn SttTokenIssuer> = opts.stt_token_issuer.unwrap_or_else(|| {
        match OpenAiSttConfig::from_env().map(OpenAiSttTokenIssuer::new) {
            Some(Ok(issuer)) => Arc::new(issuer),
            Some(Err(e)) => {
                tracing::error!(error = %e, "openai stt token issuer unavailable");
                Arc::new(NoSttTokenIssuer)
            }
            None => {
                tracing::warn!("OPENAI_API_KEY not set: stt-token answers 503 STT_NOT_CONFIGURED");
                Arc::new(NoSttTokenIssuer)
            }
        }
    });
    let billing = BillingModule::new(BillingDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: Arc::new(SystemClock),
        access: Arc::new(crate::billing_wiring::BillingWorkspaceAccess(
            workspace.queries(),
        )),
        gateway: crate::billing_wiring::stripe_gateway_from_env(),
        state_sink: billing_redis.map(|c| {
            Arc::new(warptalk_billing::infrastructure::redis_state::RedisStateSink::new(c))
                as Arc<dyn warptalk_billing::application::ports::ServiceStateSink>
        }),
        config: opts
            .billing_config
            .unwrap_or_else(crate::billing_wiring::billing_config_from_env),
    });
    let server_stt = opts
        .copilot_server_stt
        .or_else(crate::copilot_stt::server_stt_from_env);
    let audio_config = opts
        .copilot_audio_config
        .unwrap_or_else(warptalk_copilot::application::audio::AudioConfig::from_env);
    let glossary_service = transcript.as_ref().map(|t| t.glossaries());
    let copilot = copilot_redis.map(|conn| {
        let usage_conn = conn.clone();
        let ticket_conn = conn.clone();
        let limiter_conn = conn.clone();
        let mut module = CopilotModule::new(CopilotDeps {
            pool: pool.clone(),
            jwt: jwt.clone(),
            clock: Arc::new(SystemClock),
            rooms: Arc::new(CopilotRooms {
                rooms: room.queries(),
                redis: conn.clone(),
                effects: effects.clone(),
            }),
            members: Arc::new(CopilotMembers(workspace.queries())),
            sink: Arc::new(RedisSegmentSink::new(conn.clone())),
            stt_tokens,
            rate_limiter: Arc::new(RedisRateLimiter::new(conn)),
        })
        .with_metering(Metering {
            usage: Arc::new(
                warptalk_copilot::infrastructure::redis_adapters::RedisUsageSink::new(usage_conn),
            ),
            gate: Arc::new(crate::billing_wiring::BillingCreditGate(billing.queries())),
            stt_model: crate::billing_wiring::copilot_stt_model(),
        });
        if let Some(stt) = server_stt {
            module = module.with_server_stt(warptalk_copilot::application::audio::ServerAudio {
                stt,
                tickets: Arc::new(
                    warptalk_copilot::infrastructure::redis_adapters::RedisTicketStore::new(
                        ticket_conn,
                    ),
                ),
                config: audio_config,
                limiter: Arc::new(
                    warptalk_copilot::infrastructure::redis_adapters::RedisConnectionLimiter::new(
                        limiter_conn,
                    ),
                ),
            });
        }
        if let Some(g) = glossary_service {
            module = module.with_glossary(Arc::new(crate::copilot_stt::CopilotGlossary(g)));
        }
        module
    });

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
    warptalk_assistant::summary::register_config(&settings);
    let summary_service = SummaryService::new(SummaryDeps {
        repo: PgSummaryRepository::new(pool.clone()),
        transcripts: Arc::new(PgTranscriptSource::new(pool.clone())),
        rooms: Arc::new(crate::assistant_adapters::room_meta_source(
            room.queries().reader(),
            identity.queries(),
            warptalk_copilot::CopilotQueries::new(pool.clone()),
        )),
        followups: match &summary_redis {
            Some(c) => Arc::new(RedisFollowUpSource::new(c.clone())) as Arc<dyn FollowUpSource>,
            None => Arc::new(NoFollowUps),
        },
        llm: ai.clone(),
        policy: Arc::new(crate::assistant_adapters::WorkspaceExternalLlmGate(
            workspace.queries(),
        )),
        settings: settings.clone(),
        bus: match &summary_redis {
            Some(c) => Arc::new(RedisSummaryBus::new(c.clone())) as Arc<dyn SummaryBus>,
            None => Arc::new(NoBus),
        },
        clock: Arc::new(SystemClock),
    });
    let summary_service = Arc::new(match &summary_redis {
        Some(c) => summary_service.with_usage(Arc::new(
            warptalk_assistant::usage::RedisUsageSink::new(c.clone()),
        )),
        None => summary_service,
    });
    let assistant = assistant.with_summary(SummaryHttpDeps {
        service: summary_service.clone(),
        access: Arc::new(ComposedRoomAccess::new(
            Arc::new(crate::assistant_adapters::RoomFactsAdapter(
                room.queries().reader(),
            )),
            Arc::new(AssistantWorkspaceAccess(workspace.queries())),
        )),
        clock: Arc::new(SystemClock),
    });
    let assistant = assistant.with_suggestion_feedback(
        warptalk_assistant::suggestion::feedback_http::FeedbackApi {
            store: Arc::new(warptalk_assistant::suggestion::PgSuggestionStore::new(
                pool.clone(),
            )),
            access: Arc::new(ComposedRoomAccess::new(
                Arc::new(crate::assistant_adapters::RoomFactsAdapter(
                    room.queries().reader(),
                )),
                Arc::new(AssistantWorkspaceAccess(workspace.queries())),
            )),
            members: Arc::new(AssistantWorkspaceAccess(workspace.queries())),
            clock: Arc::new(SystemClock),
        },
    );
    let vectors = opts.vectors.unwrap_or_else(|| {
        build_vector_store().unwrap_or_else(|e| {
            tracing::error!(error = %e, "vector store unavailable; using in-memory");
            Arc::new(warptalk_qdrant::InMemoryVectorStore::new())
        })
    });
    let knowledge_access: Arc<dyn DocumentAccess> =
        Arc::new(KnowledgeAccessAdapter(workspace.queries()));
    let chat_llm = ai.clone();
    let knowledge = build_knowledge_module(KnowledgeParts {
        pool: pool.clone(),
        jwt: jwt.clone(),
        settings: settings.clone(),
        vectors,
        ai: ai.clone(),
        access: knowledge_access.clone(),
        results,
        documents: None,
    })
    .await;
    // WarpBot "ask the meeting": SSE/JSON Q&A, recap, history (settings namespace `assistant_chat`).
    warptalk_assistant::chat::register_config(&settings);
    let (chat_limiter, chat_cache): (
        Arc<dyn warptalk_assistant::chat::RateLimiter>,
        Arc<dyn warptalk_assistant::chat::RecapCache>,
    ) = match &summary_redis {
        Some(c) => (
            Arc::new(warptalk_assistant::chat::RedisRateLimiter::new(c.clone())),
            Arc::new(warptalk_assistant::chat::RedisRecapCache::new(c.clone())),
        ),
        None => (
            Arc::new(warptalk_assistant::chat::MemoryRateLimiter::default()),
            Arc::new(warptalk_assistant::chat::MemoryRecapCache::default()),
        ),
    };
    let chat_usage: warptalk_assistant::usage::SharedUsageSink = match &summary_redis {
        Some(c) => Arc::new(warptalk_assistant::usage::RedisUsageSink::new(c.clone())),
        None => Arc::new(warptalk_assistant::usage::NoUsageSink),
    };
    let mut assistant = assistant.with_chat(warptalk_assistant::chat::ChatApi {
        service: Arc::new(warptalk_assistant::chat::ChatService::new(
            warptalk_assistant::chat::ChatDeps {
                repo: warptalk_assistant::chat::PgChatRepository::new(pool.clone()),
                transcripts: Arc::new(PgTranscriptSource::new(pool.clone())),
                rooms: Arc::new(crate::assistant_adapters::room_meta_source(
                    room.queries().reader(),
                    identity.queries(),
                    warptalk_copilot::CopilotQueries::new(pool.clone()),
                )),
                knowledge: Arc::new(warptalk_ai_plane::knowledge::ServiceRetriever::new(
                    knowledge.service(),
                )),
                prior: Arc::new(warptalk_assistant::chat::SummaryMemoryPriorContext::new(
                    summary_service,
                )),
                llm: chat_llm,
                policy: Arc::new(crate::assistant_adapters::WorkspaceExternalLlmGate(
                    workspace.queries(),
                )),
                settings: settings.clone(),
                limiter: chat_limiter,
                recap_cache: chat_cache,
                usage: chat_usage,
                clock: Arc::new(SystemClock),
            },
        )),
        access: Arc::new(ComposedRoomAccess::new(
            Arc::new(crate::assistant_adapters::RoomFactsAdapter(
                room.queries().reader(),
            )),
            Arc::new(AssistantWorkspaceAccess(workspace.queries())),
        )),
    });
    // WarpBot chat. Redis carries the live stream across replicas; without it, events stay in
    // this process (single replica only).
    let chat_bus: Arc<dyn ChatEventBus> = match chat_redis {
        Some(conn) => Arc::new(RedisChatEventBus::new(conn)),
        None => Arc::new(InMemoryChatEventBus::new()),
    };
    assistant.enable_chat(ChatDeps {
        bus: chat_bus,
        llm: Arc::new(AiAssistantLlm(ai)),
        consent: Arc::new(PolicyConsent(Arc::new(PgExternalLlmPolicy::new(
            pool.clone(),
        )))),
        knowledge: Some(Arc::new(KnowledgeSearch {
            service: knowledge.service(),
            access: knowledge_access,
        })),
        default_model: None,
    });
    let suggestions_router = suggestions_redis.map(|redis| {
        suggestions_http::router(SuggestionsState {
            redis,
            jwt: jwt.clone(),
            rooms: room.queries(),
            workspaces: workspace.queries(),
            test_mode: opts.test_mode,
        })
    });
    let features_router = features_http::router(FeaturesState {
        store: settings.clone(),
        jwt: jwt.clone(),
        workspaces: workspace.queries(),
    });
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
        .merge(billing.router())
        .merge(room.router())
        .merge(meeting_router)
        .merge(settings_router)
        .merge(features_router)
        .merge(assistant.router())
        .merge(knowledge.router())
        .merge(notification.router());
    let api = match suggestions_router {
        Some(r) => api.merge(r),
        None => api,
    };
    let api = match copilot {
        Some(c) => api.merge(c.router()),
        None => api,
    };
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
