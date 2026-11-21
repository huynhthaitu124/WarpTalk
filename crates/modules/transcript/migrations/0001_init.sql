CREATE SCHEMA IF NOT EXISTS transcript;

-- Column names/types follow docs/spec/05a-data-dictionary.md "Transcript".
-- Additions beyond 05a (spec 12): transcript_corrections.reason, segment_translation_links.is_stale (WT-704).

CREATE TABLE IF NOT EXISTS transcript.transcripts (
    id                          uuid PRIMARY KEY,
    workspace_id                uuid NOT NULL,
    translation_room_id         uuid NOT NULL,
    translation_room_session_id uuid,
    version                     integer NOT NULL DEFAULT 1,
    status                      varchar(255) NOT NULL DEFAULT 'RECORDING',
    source_language             varchar(15) NOT NULL,
    total_segments              integer NOT NULL DEFAULT 0,
    total_duration_ms           integer NOT NULL DEFAULT 0,
    is_current                  boolean NOT NULL DEFAULT true,
    previous_transcript_id      uuid,
    engine_version              varchar(50),
    last_sequence_order         integer NOT NULL DEFAULT 0,
    is_active                   boolean NOT NULL DEFAULT true,
    finalized_at                timestamptz,
    created_at                  timestamptz NOT NULL,
    created_by                  uuid,
    updated_at                  timestamptz,
    updated_by                  uuid,
    deleted_at                  timestamptz,
    deleted_by                  uuid
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_transcripts_room_version
    ON transcript.transcripts (translation_room_id, version);
CREATE UNIQUE INDEX IF NOT EXISTS ux_transcripts_room_current
    ON transcript.transcripts (translation_room_id)
    WHERE is_current;

CREATE TABLE IF NOT EXISTS transcript.transcript_segments (
    id                     uuid PRIMARY KEY,
    transcript_id          uuid NOT NULL,
    speaker_participant_id uuid,
    speaker_name           varchar(100) NOT NULL,
    original_text          text NOT NULL,
    original_language      varchar(15) NOT NULL,
    start_time_ms          integer NOT NULL,
    end_time_ms            integer NOT NULL,
    confidence             numeric(5,4),
    sequence_order         integer NOT NULL,
    is_corrected           boolean NOT NULL DEFAULT false,
    is_final               boolean NOT NULL DEFAULT true,
    matched_segment_id     uuid,
    clean_text             text,
    clean_flags            text[],
    created_at             timestamptz NOT NULL,
    updated_at             timestamptz NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_transcript_segments_order
    ON transcript.transcript_segments (transcript_id, sequence_order);

CREATE TABLE IF NOT EXISTS transcript.transcript_corrections (
    id                             uuid PRIMARY KEY,
    segment_id                     uuid NOT NULL,
    user_id                        uuid NOT NULL,
    original_text                  text NOT NULL,
    corrected_text                 text NOT NULL,
    correction_type                varchar(255) NOT NULL,
    status                         varchar(255) NOT NULL DEFAULT 'PENDING',
    triggered_retranslation        boolean NOT NULL DEFAULT false,
    translation_content_id         uuid,
    reversal_credit_transaction_id uuid,
    reviewed_by                    uuid,
    reviewed_at                    timestamptz,
    created_at                     timestamptz NOT NULL,
    reason                         text
);

CREATE INDEX IF NOT EXISTS ix_transcript_corrections_segment
    ON transcript.transcript_corrections (segment_id, created_at);

CREATE TABLE IF NOT EXISTS transcript.translation_contents (
    id                              uuid PRIMARY KEY,
    workspace_id                    uuid NOT NULL,
    text_hash                       varchar(64) NOT NULL,
    target_language                 varchar(15) NOT NULL,
    translated_text                 text NOT NULL,
    translator_model                varchar(100) NOT NULL,
    confidence                      numeric(5,4),
    source_stt_confidence           numeric(5,4),
    is_retranslated                 boolean NOT NULL DEFAULT false,
    previous_translation_content_id uuid,
    latency_ms                      integer,
    status                          varchar(20) NOT NULL DEFAULT 'pending',
    created_at                      timestamptz NOT NULL,
    updated_at                      timestamptz NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_translation_contents_dedup
    ON transcript.translation_contents (workspace_id, text_hash, target_language);

CREATE TABLE IF NOT EXISTS transcript.segment_translation_links (
    segment_id             uuid NOT NULL,
    translation_content_id uuid NOT NULL,
    target_language        varchar(15) NOT NULL,
    is_current             boolean NOT NULL DEFAULT true,
    delivered_at           timestamptz,
    created_at             timestamptz NOT NULL,
    is_stale               boolean NOT NULL DEFAULT false,
    PRIMARY KEY (segment_id, translation_content_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_segment_translation_links_current
    ON transcript.segment_translation_links (segment_id, target_language)
    WHERE is_current;

CREATE TABLE IF NOT EXISTS transcript.audio_dubbings (
    id                        uuid PRIMARY KEY,
    workspace_id              uuid NOT NULL,
    translation_content_id    uuid NOT NULL,
    text_hash                 varchar(64) NOT NULL,
    voice_type                varchar(20) NOT NULL,
    provider                  varchar(50) NOT NULL DEFAULT 'cartesia',
    provider_voice_id         varchar(255) NOT NULL,
    previous_audio_dubbing_id uuid,
    audio_url                 varchar(500),
    duration_ms               integer,
    status                    varchar(20) NOT NULL DEFAULT 'pending',
    created_at                timestamptz NOT NULL,
    updated_at                timestamptz NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_audio_dubbings_dedup
    ON transcript.audio_dubbings (workspace_id, text_hash, provider_voice_id);
