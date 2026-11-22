-- Knowledge documents (05a workspace.workspace_documents) plus the audit trail.
-- Additions over the dictionary: access_level (PUBLIC | MEMBERS_ONLY | ADMIN_ONLY),
-- ingestion_failure_reason and revision (monotonic per re-index; part of the index job id).
-- document_type holds the category (GENERAL, TECHNICAL, FINANCIAL, LEGAL, MEETING_NOTES).

CREATE TABLE IF NOT EXISTS workspace.workspace_documents (
    id                       uuid PRIMARY KEY,
    workspace_id             uuid NOT NULL,
    uploaded_by              uuid,
    owner_id                 uuid,
    name                     varchar(255) NOT NULL,
    file_name                varchar(255) NOT NULL,
    file_extension           varchar(20) NOT NULL,
    mime_type                varchar(100) NOT NULL,
    size_bytes               bigint NOT NULL,
    storage_provider         varchar(50) NOT NULL,
    storage_key              varchar(500) NOT NULL,
    source_type              varchar(50) NOT NULL DEFAULT 'upload',
    source_id                uuid,
    document_type            varchar(50) NOT NULL DEFAULT 'GENERAL',
    source_language          varchar(20),
    detected_language        varchar(20),
    business_domain          varchar(100),
    summary                  text,
    keywords                 jsonb,
    ai_eligible              boolean NOT NULL DEFAULT true,
    is_ai_allowed            boolean NOT NULL DEFAULT true,
    ai_usage_policy          jsonb,
    ingestion_status         varchar(30) NOT NULL DEFAULT 'pending',
    last_indexed_at          timestamptz,
    index_version            varchar(50),
    confidentiality_level    varchar(30) NOT NULL DEFAULT 'public_internal',
    retention_state          varchar(30) NOT NULL DEFAULT 'active',
    status                   varchar(30) NOT NULL DEFAULT 'active',
    created_at               timestamptz NOT NULL,
    updated_at               timestamptz,
    deleted_at               timestamptz,
    deleted_by               uuid,
    access_level             varchar(20) NOT NULL DEFAULT 'MEMBERS_ONLY'
        CHECK (access_level IN ('PUBLIC', 'MEMBERS_ONLY', 'ADMIN_ONLY')),
    ingestion_failure_reason text,
    revision                 integer NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS ix_workspace_documents_ws_status_created
    ON workspace.workspace_documents (workspace_id, status, created_at DESC);

CREATE TABLE IF NOT EXISTS workspace.workspace_document_audits (
    id           uuid PRIMARY KEY,
    document_id  uuid NOT NULL,
    workspace_id uuid NOT NULL,
    actor_id     uuid,
    action       varchar(50) NOT NULL,
    action_at    timestamptz NOT NULL,
    metadata     jsonb,
    ip_address   varchar(64),
    user_agent   varchar(500)
);

CREATE INDEX IF NOT EXISTS ix_workspace_document_audits_doc
    ON workspace.workspace_document_audits (workspace_id, document_id, action_at DESC);
