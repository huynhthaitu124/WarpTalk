-- Column names/types follow docs/spec/05a-data-dictionary.md "Transcript" (glossaries).
-- Additions beyond 05a: glossary_terms.case_sensitive (spec 06 section 5.2 term DTO).
-- `usage_note` carries the term's free-text notes in the REST API.
-- Soft-deleted rows keep their data; uniqueness only applies to live rows.

CREATE TABLE IF NOT EXISTS transcript.glossaries (
    id              uuid PRIMARY KEY,
    workspace_id    uuid NOT NULL,
    name            varchar(150) NOT NULL,
    description     text,
    source_language varchar(15) NOT NULL,
    target_language varchar(15) NOT NULL,
    term_count      integer NOT NULL DEFAULT 0,
    is_active       boolean NOT NULL DEFAULT true,
    created_at      timestamptz NOT NULL,
    created_by      uuid,
    updated_at      timestamptz,
    updated_by      uuid,
    deleted_at      timestamptz,
    deleted_by      uuid
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_glossaries_ws_name
    ON transcript.glossaries (workspace_id, lower(name))
    WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS transcript.glossary_terms (
    id             uuid PRIMARY KEY,
    glossary_id    uuid NOT NULL REFERENCES transcript.glossaries (id),
    source_term    varchar(255) NOT NULL,
    target_term    varchar(255) NOT NULL,
    context        text,
    domain         varchar(50),
    priority       integer NOT NULL DEFAULT 5,
    definition     text,
    usage_note     text,
    part_of_speech varchar(50),
    is_active      boolean NOT NULL DEFAULT true,
    case_sensitive boolean NOT NULL DEFAULT false,
    created_at     timestamptz NOT NULL,
    created_by     uuid,
    updated_at     timestamptz,
    updated_by     uuid,
    deleted_at     timestamptz,
    deleted_by     uuid
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_glossary_terms_source
    ON transcript.glossary_terms (glossary_id, lower(source_term))
    WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS transcript.global_glossary_terms (
    id                    uuid PRIMARY KEY,
    term                  varchar(255) NOT NULL,
    preferred_translation varchar(255) NOT NULL,
    source_language       varchar(15),
    target_language       varchar(15),
    business_domain       varchar(100),
    definition            text,
    usage_note            text,
    priority              integer NOT NULL DEFAULT 5,
    status                varchar(30) NOT NULL DEFAULT 'draft',
    version               integer NOT NULL DEFAULT 1,
    created_at            timestamptz NOT NULL,
    created_by            uuid,
    updated_at            timestamptz,
    updated_by            uuid,
    deleted_at            timestamptz,
    deleted_by            uuid
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_global_glossary_terms_key
    ON transcript.global_glossary_terms (
        lower(term),
        coalesce(source_language, ''),
        coalesce(target_language, ''),
        coalesce(business_domain, '')
    )
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS ix_global_glossary_terms_status
    ON transcript.global_glossary_terms (status)
    WHERE deleted_at IS NULL;

-- term_id carries no foreign key: the audit outlives a hard-deleted term.
CREATE TABLE IF NOT EXISTS transcript.global_glossary_audits (
    id            uuid PRIMARY KEY,
    term_id       uuid NOT NULL,
    action        varchar(30) NOT NULL,
    before_json   jsonb,
    after_json    jsonb,
    actor_user_id uuid NOT NULL,
    created_at    timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS ix_global_glossary_audits_term
    ON transcript.global_glossary_audits (term_id, created_at);
