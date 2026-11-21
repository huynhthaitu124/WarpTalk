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
use warptalk_bus::publish_json;
use warptalk_config::JwtConfig;
use warptalk_contracts::events::{DomainEventEnvelope, MEETING_PRODUCER};
use warptalk_identity::{
    IdentityDeps, IdentityModule, IdentityQueries, Policy, VerificationMailer,
};
use warptalk_kernel::{
    AppError, AppResult, Clock, RoomId, SystemClock, UserId, WorkspaceId, WorkspaceRole,
};
use warptalk_livekit::{LiveKitConfig, NoopRoomService, RoomServiceClient};
use warptalk_meeting::application::ports::{
    MeetingEvent, MeetingEventPublisher, NoopMeetingEventPublisher, ParticipantInfo, RoomDirectory,
    RoomInfo,
};
use warptalk_meeting::{DEFAULT_TOKEN_TTL, MeetingDeps, MeetingModule, RoomService};
use warptalk_room::RoomDeps;
use warptalk_room::RoomModule;
use warptalk_room::RoomQueries;
use warptalk_room::application::ports::{
    NoopRoomEventPublisher, NoopVoiceConsentDirectory, RoomEventPublisher, VoiceConsentDirectory,
    WorkspacePolicy,
};
use warptalk_room::infrastructure::redis_publisher::RedisRoomEventPublisher;
use warptalk_settings::{SettingsStore, namespaces};
use warptalk_web::JwtKeys;
use warptalk_workspace::application::ports::UserDirectory;
use warptalk_workspace::{UserSummary, WorkspaceDeps, WorkspaceModule, WorkspaceQueries};

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
pub fn build(
    pool: PgPool,
    jwt: JwtKeys,
    jwt_cfg: &JwtConfig,
    cors_origins: &[String],
    mailer: Arc<dyn VerificationMailer>,
    redis: Option<ConnectionManager>,
    opts: ApiOptions,
) -> Router {
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
    let workspace = WorkspaceModule::new(WorkspaceDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: clock.clone(),
        users: Arc::new(IdentityUserDirectory(identity.queries())),
    });

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
    let events: Arc<dyn RoomEventPublisher> = match redis {
        Some(conn) => Arc::new(RedisRoomEventPublisher::new(conn)),
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
        .merge(settings_router);
    let router = Router::new()
        .route("/health", get(warptalk_web::health))
        .nest("/api/v1", api);
    warptalk_web::common_layers(router, cors_origins)
}
