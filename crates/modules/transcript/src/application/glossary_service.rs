//! Glossary use cases (spec 06 section 5.2, spec 12 section 5.7, BR-57, BR-98).
//!
//! Every change re-publishes the affected Redis snapshot (`GlossaryFeed`) so the pipeline sees it
//! without a database. A feed failure is logged, not surfaced: the database stays the truth and the
//! next change (or a re-save) rewrites the snapshot.

use std::sync::Arc;

use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, Clock, UserId, WorkspaceId, WorkspaceRole};

use super::glossary_ports::{
    GlobalAuditEntry, GlossaryFeed, GlossaryPatch, GlossaryRepo, GlossaryScope, ImportOutcome,
    NewGlossary, PublishOutcome, TermFields, WorkspaceRoles,
};
use crate::domain::glossary::{
    GlobalAudit, GlobalStatus, GlobalTerm, GlobalTermInput, Glossary, GlossaryTerm,
    parse_import_csv, render_csv, validate_description, validate_global_term, validate_language,
    validate_name, validate_notes, validate_term_pair,
};

#[derive(Debug, Clone)]
pub struct CreateGlossary {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub description: Option<String>,
    pub source_language: String,
    pub target_language: String,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateGlossary {
    pub name: Option<String>,
    pub description: Option<String>,
    pub source_language: Option<String>,
    pub target_language: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TermInput {
    pub source_term: String,
    pub target_term: String,
    pub case_sensitive: bool,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GlobalInput {
    pub term: String,
    pub preferred_translation: String,
    pub source_language: Option<String>,
    pub target_language: Option<String>,
    pub business_domain: Option<String>,
    pub definition: Option<String>,
    pub usage_note: Option<String>,
    pub priority: Option<i32>,
}

impl GlobalInput {
    fn as_domain(&self) -> GlobalTermInput<'_> {
        GlobalTermInput {
            term: &self.term,
            preferred_translation: &self.preferred_translation,
            source_language: self.source_language.as_deref(),
            target_language: self.target_language.as_deref(),
            business_domain: self.business_domain.as_deref(),
            definition: self.definition.as_deref(),
            usage_note: self.usage_note.as_deref(),
            priority: self.priority,
        }
    }
}

pub struct GlossaryService {
    repo: Arc<dyn GlossaryRepo>,
    roles: Arc<dyn WorkspaceRoles>,
    feed: Arc<dyn GlossaryFeed>,
    clock: Arc<dyn Clock>,
}

const NOT_FOUND: &str = "glossary not found";
const TERM_NOT_FOUND: &str = "glossary term not found";
const GLOBAL_NOT_FOUND: &str = "global glossary term not found";

impl GlossaryService {
    pub fn new(
        repo: Arc<dyn GlossaryRepo>,
        roles: Arc<dyn WorkspaceRoles>,
        feed: Arc<dyn GlossaryFeed>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            repo,
            roles,
            feed,
            clock,
        }
    }

    async fn require_member(&self, ws: WorkspaceId, user: UserId) -> AppResult<WorkspaceRole> {
        self.roles
            .member_role(ws, user)
            .await?
            .ok_or_else(|| AppError::forbidden("not an active member of this workspace"))
    }

    async fn require_writer(&self, ws: WorkspaceId, user: UserId) -> AppResult<()> {
        match self.require_member(ws, user).await? {
            WorkspaceRole::Owner | WorkspaceRole::Admin => Ok(()),
            WorkspaceRole::Member => Err(AppError::forbidden(
                "workspace owner or admin role required",
            )),
        }
    }

    async fn load(&self, id: Uuid) -> AppResult<Glossary> {
        self.repo
            .glossary(id)
            .await?
            .ok_or_else(|| AppError::not_found(NOT_FOUND))
    }

    async fn load_for_read(&self, id: Uuid, user: UserId) -> AppResult<Glossary> {
        let g = self.load(id).await?;
        self.require_member(g.workspace_id, user).await?;
        Ok(g)
    }

    async fn load_for_write(&self, id: Uuid, user: UserId) -> AppResult<Glossary> {
        let g = self.load(id).await?;
        self.require_writer(g.workspace_id, user).await?;
        Ok(g)
    }

    async fn republish_workspace(&self, ws: WorkspaceId) {
        let res = async {
            let terms = self.repo.workspace_snapshot(ws).await?;
            self.feed
                .publish(&GlossaryScope::Workspace(ws), &terms)
                .await
        }
        .await;
        if let Err(e) = res {
            tracing::error!(workspace = %ws, error = %e, "glossary snapshot publish failed");
        }
    }

    async fn republish_global(&self) {
        let res = async {
            let terms = self.repo.published_global_snapshot().await?;
            self.feed.publish(&GlossaryScope::Global, &terms).await
        }
        .await;
        if let Err(e) = res {
            tracing::error!(error = %e, "global glossary snapshot publish failed");
        }
    }

    // ---------------------------------------------------------------- workspace glossaries

    pub async fn create(&self, user: UserId, c: CreateGlossary) -> AppResult<Glossary> {
        self.require_writer(c.workspace_id, user).await?;
        let new = NewGlossary {
            workspace_id: c.workspace_id,
            name: validate_name(&c.name)?,
            description: validate_description(c.description.as_deref())?,
            source_language: validate_language(&c.source_language)?,
            target_language: validate_language(&c.target_language)?,
            actor: user,
            now: self.clock.now(),
        };
        if new.source_language == new.target_language {
            return Err(AppError::validation(
                "INVALID_LANGUAGE",
                "source and target language must differ",
            ));
        }
        let g = self.repo.create_glossary(&new).await.map_err(dup_name)?;
        self.republish_workspace(g.workspace_id).await;
        Ok(g)
    }

    pub async fn list_for_workspace(
        &self,
        user: UserId,
        ws: WorkspaceId,
    ) -> AppResult<Vec<Glossary>> {
        self.require_member(ws, user).await?;
        self.repo.glossaries_by_workspace(ws).await
    }

    pub async fn get(&self, user: UserId, id: Uuid) -> AppResult<Glossary> {
        self.load_for_read(id, user).await
    }

    pub async fn update(&self, user: UserId, id: Uuid, u: UpdateGlossary) -> AppResult<Glossary> {
        let cur = self.load_for_write(id, user).await?;
        let patch = GlossaryPatch {
            name: u.name.as_deref().map(validate_name).transpose()?,
            description: match u.description.as_deref() {
                None => None,
                Some(d) => Some(validate_description(Some(d))?),
            },
            source_language: u
                .source_language
                .as_deref()
                .map(validate_language)
                .transpose()?,
            target_language: u
                .target_language
                .as_deref()
                .map(validate_language)
                .transpose()?,
        };
        let src = patch
            .source_language
            .as_ref()
            .unwrap_or(&cur.source_language);
        let tgt = patch
            .target_language
            .as_ref()
            .unwrap_or(&cur.target_language);
        if src == tgt {
            return Err(AppError::validation(
                "INVALID_LANGUAGE",
                "source and target language must differ",
            ));
        }
        let g = self
            .repo
            .update_glossary(id, &patch, user, self.clock.now())
            .await
            .map_err(dup_name)?
            .ok_or_else(|| AppError::not_found(NOT_FOUND))?;
        self.republish_workspace(g.workspace_id).await;
        Ok(g)
    }

    pub async fn delete(&self, user: UserId, id: Uuid) -> AppResult<()> {
        let g = self.load_for_write(id, user).await?;
        if !self
            .repo
            .delete_glossary(id, user, self.clock.now())
            .await?
        {
            return Err(AppError::not_found(NOT_FOUND));
        }
        self.republish_workspace(g.workspace_id).await;
        Ok(())
    }

    // ---------------------------------------------------------------- terms

    pub async fn terms(&self, user: UserId, glossary: Uuid) -> AppResult<Vec<GlossaryTerm>> {
        self.load_for_read(glossary, user).await?;
        self.repo.terms(glossary).await
    }

    fn term_fields(t: &TermInput) -> AppResult<TermFields> {
        let (source_term, target_term) = validate_term_pair(&t.source_term, &t.target_term)?;
        Ok(TermFields {
            source_term,
            target_term,
            case_sensitive: t.case_sensitive,
            notes: validate_notes(t.notes.as_deref())?,
        })
    }

    pub async fn add_term(
        &self,
        user: UserId,
        glossary: Uuid,
        t: TermInput,
    ) -> AppResult<GlossaryTerm> {
        let g = self.load_for_write(glossary, user).await?;
        let fields = Self::term_fields(&t)?;
        let term = self
            .repo
            .add_term(glossary, &fields, user, self.clock.now())
            .await?;
        self.republish_workspace(g.workspace_id).await;
        Ok(term)
    }

    pub async fn update_term(
        &self,
        user: UserId,
        glossary: Uuid,
        term: Uuid,
        t: TermInput,
    ) -> AppResult<GlossaryTerm> {
        let g = self.load_for_write(glossary, user).await?;
        let fields = Self::term_fields(&t)?;
        let out = self
            .repo
            .update_term(glossary, term, &fields, user, self.clock.now())
            .await?
            .ok_or_else(|| AppError::not_found(TERM_NOT_FOUND))?;
        self.republish_workspace(g.workspace_id).await;
        Ok(out)
    }

    pub async fn delete_term(&self, user: UserId, glossary: Uuid, term: Uuid) -> AppResult<()> {
        let g = self.load_for_write(glossary, user).await?;
        if !self
            .repo
            .delete_term(glossary, term, user, self.clock.now())
            .await?
        {
            return Err(AppError::not_found(TERM_NOT_FOUND));
        }
        self.republish_workspace(g.workspace_id).await;
        Ok(())
    }

    /// Bulk import of `source,target` CSV. Rows duplicated inside the payload keep the first;
    /// rows whose source term already exists are skipped.
    pub async fn import_csv(
        &self,
        user: UserId,
        glossary: Uuid,
        csv: &str,
    ) -> AppResult<ImportReport> {
        let g = self.load_for_write(glossary, user).await?;
        let parsed = parse_import_csv(csv)?;
        let outcome: ImportOutcome = self
            .repo
            .import_terms(glossary, &parsed.pairs, user, self.clock.now())
            .await?;
        if outcome.imported > 0 {
            self.republish_workspace(g.workspace_id).await;
        }
        Ok(ImportReport {
            imported: outcome.imported,
            skipped: outcome.skipped_existing + parsed.duplicates_in_payload,
        })
    }

    pub async fn export_csv(&self, user: UserId, glossary: Uuid) -> AppResult<String> {
        self.load_for_read(glossary, user).await?;
        let terms = self.repo.terms(glossary).await?;
        Ok(render_csv(&terms))
    }

    /// Published global terms (any authenticated user).
    pub async fn published_global(&self) -> AppResult<Vec<GlobalTerm>> {
        Ok(self
            .repo
            .global_terms(Some(GlobalStatus::Published), 1_000, 0)
            .await?
            .0)
    }

    // ---------------------------------------------------------------- global glossary (system admin)

    fn entry(&self, action: &'static str, actor: UserId) -> GlobalAuditEntry {
        GlobalAuditEntry {
            action,
            actor,
            now: self.clock.now(),
        }
    }

    pub async fn global_list(
        &self,
        status: Option<GlobalStatus>,
        page: i64,
        page_size: i64,
    ) -> AppResult<(Vec<GlobalTerm>, i64)> {
        let size = page_size.clamp(1, 200);
        let page = page.max(1);
        self.repo
            .global_terms(status, size, (page - 1) * size)
            .await
    }

    pub async fn global_get(&self, id: Uuid) -> AppResult<GlobalTerm> {
        self.repo
            .global_term(id)
            .await?
            .ok_or_else(|| AppError::not_found(GLOBAL_NOT_FOUND))
    }

    pub async fn global_create(&self, admin: UserId, i: GlobalInput) -> AppResult<GlobalTerm> {
        let f = validate_global_term(&i.as_domain())?;
        self.repo
            .create_global(Uuid::now_v7(), &f, &self.entry("created", admin))
            .await
    }

    pub async fn global_update(
        &self,
        admin: UserId,
        id: Uuid,
        i: GlobalInput,
    ) -> AppResult<GlobalTerm> {
        let f = validate_global_term(&i.as_domain())?;
        let t = self
            .repo
            .update_global(id, &f, &self.entry("updated", admin))
            .await?
            .ok_or_else(|| AppError::not_found(GLOBAL_NOT_FOUND))?;
        if t.status == GlobalStatus::Published {
            self.republish_global().await;
        }
        Ok(t)
    }

    pub async fn global_publish(&self, admin: UserId, id: Uuid) -> AppResult<GlobalTerm> {
        let cur = self.global_get(id).await?;
        if cur.status == GlobalStatus::Published {
            return Ok(cur);
        }
        if cur
            .definition
            .as_deref()
            .is_none_or(|d| d.trim().is_empty())
        {
            return Err(AppError::validation(
                "DEFINITION_REQUIRED",
                "a term needs a definition before it can be published",
            ));
        }
        match self
            .repo
            .publish_global(id, &self.entry("published", admin))
            .await?
        {
            PublishOutcome::Published(t) => {
                let t = *t;
                self.republish_global().await;
                Ok(t)
            }
            PublishOutcome::CapReached => Err(AppError::conflict(
                "GLOBAL_CAP_REACHED",
                format!(
                    "at most {} global terms can be published",
                    crate::domain::glossary::MAX_PUBLISHED_GLOBAL_TERMS
                ),
            )),
            PublishOutcome::NotFound => Err(AppError::not_found(GLOBAL_NOT_FOUND)),
        }
    }

    pub async fn global_archive(&self, admin: UserId, id: Uuid) -> AppResult<GlobalTerm> {
        let cur = self.global_get(id).await?;
        if cur.status == GlobalStatus::Archived {
            return Ok(cur);
        }
        let t = self
            .repo
            .archive_global(id, &self.entry("archived", admin))
            .await?
            .ok_or_else(|| AppError::not_found(GLOBAL_NOT_FOUND))?;
        if cur.status == GlobalStatus::Published {
            self.republish_global().await;
        }
        Ok(t)
    }

    pub async fn global_delete(&self, admin: UserId, id: Uuid) -> AppResult<()> {
        let cur = self.global_get(id).await?;
        if !self
            .repo
            .delete_global(id, &self.entry("deleted", admin))
            .await?
        {
            return Err(AppError::not_found(GLOBAL_NOT_FOUND));
        }
        if cur.status == GlobalStatus::Published {
            self.republish_global().await;
        }
        Ok(())
    }

    pub async fn global_audits(&self, id: Uuid) -> AppResult<Vec<GlobalAudit>> {
        // No existence check: the audit outlives a hard-deleted term.
        self.repo.global_audits(id).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: usize,
}

fn dup_name(e: AppError) -> AppError {
    match e {
        AppError::Conflict { code, .. } if code == "DUPLICATE" => {
            AppError::conflict("DUPLICATE_NAME", "a glossary with this name already exists")
        }
        other => other,
    }
}
