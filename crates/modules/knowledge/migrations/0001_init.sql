CREATE SCHEMA IF NOT EXISTS knowledge;

-- One row per (workspace, embedding profile). A collection never mixes profiles; switching the
-- profile creates a `building` row (new collection) until it is activated after a reindex.
CREATE TABLE knowledge.embedding_profiles (
    workspace_id  uuid        NOT NULL,
    profile_hash  text        NOT NULL,
    provider      text        NOT NULL,
    model         text        NOT NULL,
    dims          integer     NOT NULL CHECK (dims > 0),
    distance      text        NOT NULL,
    collection    text        NOT NULL,
    status        text        NOT NULL CHECK (status IN ('active', 'building', 'retired')),
    created_at    timestamptz NOT NULL DEFAULT now(),
    activated_at  timestamptz,
    PRIMARY KEY (workspace_id, profile_hash)
);
CREATE UNIQUE INDEX embedding_profiles_one_active
    ON knowledge.embedding_profiles (workspace_id) WHERE status = 'active';

CREATE TABLE knowledge.index_jobs (
    job_id           text        NOT NULL,
    workspace_id     uuid        NOT NULL,
    source_type      text        NOT NULL,
    source_id        text        NOT NULL,
    status           text        NOT NULL
        CHECK (status IN ('pending', 'running', 'retrying', 'indexed', 'blocked', 'deleted', 'dead')),
    attempts         integer     NOT NULL DEFAULT 0,
    last_error       text,
    chunks_indexed   integer     NOT NULL DEFAULT 0,
    last_message_id  text,
    next_attempt_at  timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    completed_at     timestamptz,
    PRIMARY KEY (workspace_id, job_id)
);
CREATE INDEX index_jobs_source ON knowledge.index_jobs (workspace_id, source_type, source_id);
CREATE INDEX index_jobs_dead ON knowledge.index_jobs (status) WHERE status = 'dead';

-- Mirror of what is in the vector store (payload = ChunkPayload JSON).
CREATE TABLE knowledge.chunks (
    id            uuid        NOT NULL,
    workspace_id  uuid        NOT NULL,
    source_type   text        NOT NULL,
    source_id     text        NOT NULL,
    kind          text        NOT NULL CHECK (kind IN ('content', 'fact')),
    ordinal       integer     NOT NULL,
    text_hash     text        NOT NULL,
    collection    text        NOT NULL,
    payload       jsonb       NOT NULL,
    indexed_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, id)
);
CREATE INDEX chunks_source ON knowledge.chunks (workspace_id, source_type, source_id, kind);

CREATE TABLE knowledge.facts (
    id                uuid        PRIMARY KEY,
    workspace_id      uuid        NOT NULL,
    source_type       text        NOT NULL,
    source_id         text        NOT NULL,
    category          text        NOT NULL
        CHECK (category IN ('decision', 'requirement', 'definition', 'commitment', 'risk', 'reference')),
    fact              text        NOT NULL,
    quote             text,
    confidence        real        NOT NULL CHECK (confidence >= 0 AND confidence <= 1),
    source_chunk_ids  uuid[]      NOT NULL DEFAULT '{}',
    chunk_id          uuid,
    created_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX facts_source ON knowledge.facts (workspace_id, source_type, source_id);
CREATE INDEX facts_category ON knowledge.facts (workspace_id, category);
