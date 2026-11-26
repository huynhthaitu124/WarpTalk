//! Tenancy, members, roles, invitations, verified domains, documents, policies, entitlements, admin audit (schema workspace).
//!
//! Slice 1: workspaces, settings and members; slice 2: knowledge documents (see
//! [`WorkspaceModule::with_documents`]). Layers (dependencies point inward):
//! - `domain`: value objects and authorization policy. No IO.
//! - `application`: use cases and ports (traits) this module needs from the outside.
//! - `infrastructure`: adapters implementing the ports (sqlx).
//! - `http`: axum handlers and request/response DTOs.

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;

use std::sync::Arc;

use sqlx::PgPool;
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::{AppResult, Clock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_web::JwtKeys;

use application::documents::{DocumentIndexPublisher, DocumentRepo, DocumentService};
use application::invitations::{InvitationRepo, InvitationService};
use application::ports::{SettingsObserver, UserDirectory, WorkspaceRepo};
use application::service::WorkspaceService;
use infrastructure::pg_documents::PgDocumentRepo;
use infrastructure::pg_invitations::PgInvitationRepo;
use infrastructure::pg_repo::PgWorkspaceRepo;
use infrastructure::redis_documents::DocumentResultHandler;
use warptalk_storage::ObjectStore;

pub use application::documents::{
    DocumentIndexRequest, DocumentsConfig, IndexOutcome, NoopDocumentIndexPublisher,
};
pub use application::invitations::{DEFAULT_INVITATION_TTL_DAYS, InvitationMail, InvitationMailer};
pub use application::ports::{
    NoopUserDirectory, SettingsObserver as WorkspaceSettingsObserver, UserSummary,
};
pub use domain::AiPolicy;
pub use domain::entitlements::{EntitlementKind, EntitlementSnapshot, EntitlementValue};
pub use infrastructure::mailers::{
    DevInvitationMailCapture, EmailInvitationMailer, LogInvitationMailer,
};
pub use infrastructure::redis_documents::{
    RedisDocumentIndexPublisher, document_results_consumer_config,
};

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "workspace",
    migrations: &[
        Migration {
            version: 1,
            name: "init",
            sql: include_str!("../migrations/0001_init.sql"),
        },
        Migration {
            version: 2,
            name: "invitations",
            sql: include_str!("../migrations/0002_invitations.sql"),
        },
        Migration {
            version: 3,
            name: "documents",
            sql: include_str!("../migrations/0003_documents.sql"),
        },
        Migration {
            version: 4,
            name: "entitlement_snapshots",
            sql: include_str!("../migrations/0004_entitlement_snapshots.sql"),
        },
    ],
};

pub struct WorkspaceDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub users: Arc<dyn UserDirectory>,
}

/// Collaborators of the documents feature.
pub struct DocumentsDeps {
    pub store: Arc<dyn ObjectStore>,
    /// Index / purge requests to the knowledge module (`RedisDocumentIndexPublisher`).
    pub publisher: Arc<dyn DocumentIndexPublisher>,
    pub config: DocumentsConfig,
}

pub struct WorkspaceModule {
    service: Arc<WorkspaceService>,
    invitations: Arc<InvitationService>,
    repo: Arc<dyn WorkspaceRepo>,
    doc_repo: Arc<dyn DocumentRepo>,
    documents: Option<Arc<DocumentService>>,
    clock: Arc<dyn Clock>,
    jwt: JwtKeys,
}

impl WorkspaceModule {
    /// Invitations are delivered through [`LogInvitationMailer`] with the default 7-day expiry.
    /// Use [`WorkspaceModule::with_invitations`] to choose the mailer and lifetime.
    pub fn new(deps: WorkspaceDeps) -> Self {
        Self::with_invitations(
            deps,
            Arc::new(LogInvitationMailer),
            DEFAULT_INVITATION_TTL_DAYS,
        )
    }

    /// `mailer`: `LogInvitationMailer` or, dev/test only, `DevInvitationMailCapture`.
    /// `invitation_ttl_days`: values <= 0 fall back to `DEFAULT_INVITATION_TTL_DAYS`.
    pub fn with_invitations(
        deps: WorkspaceDeps,
        mailer: Arc<dyn InvitationMailer>,
        invitation_ttl_days: i64,
    ) -> Self {
        let repo: Arc<dyn WorkspaceRepo> = Arc::new(PgWorkspaceRepo::new(deps.pool.clone()));
        let doc_repo: Arc<dyn DocumentRepo> = Arc::new(PgDocumentRepo::new(deps.pool.clone()));
        let inv_repo: Arc<dyn InvitationRepo> = Arc::new(PgInvitationRepo::new(deps.pool));
        let invitations = Arc::new(InvitationService::new(
            inv_repo,
            repo.clone(),
            deps.users.clone(),
            mailer,
            deps.clock.clone(),
            invitation_ttl_days,
        ));
        let service = Arc::new(WorkspaceService::new(
            repo.clone(),
            deps.users,
            deps.clock.clone(),
        ));
        Self {
            service,
            invitations,
            repo,
            doc_repo,
            documents: None,
            clock: deps.clock,
            jwt: deps.jwt,
        }
    }

    /// Enables the document endpoints. Without it they answer 503 `STORAGE_NOT_CONFIGURED`.
    pub fn with_documents(mut self, deps: DocumentsDeps) -> Self {
        self.documents = Some(Arc::new(DocumentService::new(
            self.repo.clone(),
            self.doc_repo.clone(),
            deps.store,
            deps.publisher,
            self.clock.clone(),
            deps.config,
        )));
        self
    }

    /// Be told after every successful settings write (B-12/B-13: refreshes per-room AI keys).
    pub fn set_settings_observer(&self, observer: Arc<dyn SettingsObserver>) {
        self.service.set_observer(observer);
    }

    /// Handler for `embedding:index_results` (run with [`document_results_consumer_config`]);
    /// updates document ingestion status. Needs no object store.
    pub fn document_results_handler(&self) -> Arc<DocumentResultHandler> {
        Arc::new(DocumentResultHandler::new(
            self.doc_repo.clone(),
            self.clock.clone(),
        ))
    }

    /// Routes relative to the API prefix (`/api/v1`), state applied.
    pub fn router(&self) -> axum::Router {
        http::router(http::AppState {
            service: self.service.clone(),
            invitations: self.invitations.clone(),
            documents: self.documents.clone(),
            jwt: self.jwt.clone(),
        })
    }

    /// Write side of the entitlement read model, for the consumer of billing's entitlement events.
    pub fn entitlement_projector(&self) -> EntitlementProjector {
        EntitlementProjector {
            repo: self.repo.clone(),
            clock: self.clock.clone(),
        }
    }

    /// Read-only facade for other modules.
    pub fn queries(&self) -> WorkspaceQueries {
        WorkspaceQueries {
            repo: self.repo.clone(),
            documents: self.doc_repo.clone(),
        }
    }
}

/// Projects billing's resolved entitlements into `workspace_entitlement_snapshots`.
#[derive(Clone)]
pub struct EntitlementProjector {
    repo: Arc<dyn WorkspaceRepo>,
    clock: Arc<dyn Clock>,
}

impl EntitlementProjector {
    /// Idempotent per `event_id`; an older snapshot never replaces a newer one. True when stored.
    pub async fn apply(
        &self,
        ws: WorkspaceId,
        snapshot: &EntitlementSnapshot,
        event_id: uuid::Uuid,
    ) -> AppResult<bool> {
        self.repo
            .apply_entitlement_snapshot(ws, snapshot, event_id, self.clock.now())
            .await
    }
}

/// Public query facade for other modules (membership and workspace availability).
#[derive(Clone)]
pub struct WorkspaceQueries {
    repo: Arc<dyn WorkspaceRepo>,
    documents: Arc<dyn DocumentRepo>,
}

impl WorkspaceQueries {
    /// Role of `user` in `ws` if an active member of an active workspace.
    pub async fn member_role(
        &self,
        ws: WorkspaceId,
        user: UserId,
    ) -> AppResult<Option<WorkspaceRole>> {
        self.repo.member_role(ws, user).await
    }

    /// True when the workspace exists and is neither soft-deleted nor deactivated (BR-43).
    pub async fn is_active(&self, ws: WorkspaceId) -> AppResult<bool> {
        self.repo.is_active(ws).await
    }

    /// Effective AI policy flags of a workspace (`aiRetrievalAllowed`, `externalLlmAllowed`
    /// default false; `aiSuggestEnabled` defaults true).
    pub async fn ai_policy(&self, ws: WorkspaceId) -> AppResult<AiPolicy> {
        self.repo.ai_policy(ws).await
    }

    /// Documents `role` may have the assistant answer from: active, AI-allowed, indexed
    /// documents whose access level the role may see (spec 14 section 4.2; at most 500, most
    /// recently updated first).
    pub async fn ai_retrievable_document_ids(
        &self,
        ws: WorkspaceId,
        role: WorkspaceRole,
    ) -> AppResult<Vec<uuid::Uuid>> {
        application::documents::ai_retrievable_ids(&*self.documents, ws, role).await
    }
}
