-- B-05: a segment's translation arrives one sentence (chunk) at a time. Chunks are kept here;
-- the segment's current translation link points at their concatenation.
-- B-06: offsets that do not fit the integer columns are stored as NULL (and logged), never
-- clamped to a wrong value.

CREATE TABLE IF NOT EXISTS transcript.translation_chunks (
    segment_id             uuid NOT NULL,
    target_language        varchar(15) NOT NULL,
    chunk_index            integer NOT NULL,
    workspace_id           uuid NOT NULL,
    translated_text        text NOT NULL,
    translator_model       varchar(100) NOT NULL,
    latency_ms             integer,
    source_stt_confidence  numeric(5,4),
    created_at             timestamptz NOT NULL,
    updated_at             timestamptz NOT NULL,
    PRIMARY KEY (segment_id, target_language, chunk_index)
);

ALTER TABLE transcript.transcript_segments ALTER COLUMN start_time_ms DROP NOT NULL;
ALTER TABLE transcript.transcript_segments ALTER COLUMN end_time_ms DROP NOT NULL;
