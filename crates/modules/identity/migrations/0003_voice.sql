-- Voice profiles, samples and consent (05a schema `voice`; spec 13 sections 7, 8, 11).
-- No cross-schema foreign keys: user_id / workspace_id are plain uuid columns.
CREATE SCHEMA IF NOT EXISTS voice;

CREATE TABLE voice.voice_profiles (
    id            uuid PRIMARY KEY,
    user_id       uuid        NOT NULL,
    workspace_id  uuid,
    display_name  varchar(100),
    provider      varchar(50),
    -- the provider voice id (not a biometric embedding held by WarpTalk)
    embedding_ref varchar(500),
    language      varchar(15),
    status        varchar(20) NOT NULL DEFAULT 'active',
    is_active     boolean     NOT NULL DEFAULT true,
    created_at    timestamptz NOT NULL,
    created_by    uuid,
    updated_at    timestamptz,
    updated_by    uuid,
    deleted_at    timestamptz,
    deleted_by    uuid,
    -- additions from spec 13 section 7 (absent from 05a)
    source        varchar(20) NOT NULL DEFAULT 'upload',
    quality_score integer,
    clone_error_code varchar(50)
);
CREATE INDEX voice_profiles_user_idx ON voice.voice_profiles (user_id) WHERE deleted_at IS NULL;

CREATE TABLE voice.voice_samples (
    id                 uuid PRIMARY KEY,
    voice_profile_id   uuid        NOT NULL,
    sample_type        varchar(30) NOT NULL,
    file_url           varchar(500),
    duration_seconds   integer,
    language           varchar(15),
    contains_raw_audio boolean     NOT NULL DEFAULT true,
    retention_until    timestamptz,
    created_at         timestamptz NOT NULL,
    deleted_at         timestamptz,
    deleted_by         uuid
);
CREATE INDEX voice_samples_profile_idx ON voice.voice_samples (voice_profile_id);

-- Spec 13 section 7 `VoiceConsent`. Rows are never deleted: the table is the audit trail of
-- who consented to what, when, from where (BR-94).
CREATE TABLE voice.voice_consents (
    id                    uuid PRIMARY KEY,
    user_id               uuid        NOT NULL,
    consent_type          varchar(20) NOT NULL DEFAULT 'VOICE_CLONE',
    consent_status        varchar(10) NOT NULL,
    consent_text_version  varchar(40) NOT NULL,
    consent_text_hash     varchar(64) NOT NULL,
    granted_at            timestamptz NOT NULL,
    revoked_at            timestamptz,
    ip_address            varchar(45),
    user_agent            varchar(255),
    created_at            timestamptz NOT NULL
);
CREATE UNIQUE INDEX voice_consents_one_active_idx
    ON voice.voice_consents (user_id, consent_type) WHERE consent_status = 'GRANTED';

-- Provider deletions requested by revocation / replacement. Pending until the provider confirms.
CREATE TABLE voice.provider_voice_deletions (
    provider_voice_id varchar(255) PRIMARY KEY,
    user_id           uuid        NOT NULL,
    reason            varchar(40) NOT NULL,
    requested_at      timestamptz NOT NULL,
    completed_at      timestamptz
);
