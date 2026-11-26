-- Post-session summary / minutes / action items (ADR 0001 item 7; spec 10 sections 3.9-3.11).
-- Not in 05a (the data dictionary has no assistant tables for this); schema `assistant` owns them.
-- Ids from other modules (room, workspace, user, transcript segment) are plain uuid columns.

CREATE TABLE IF NOT EXISTS assistant.meeting_summaries (
    id                 uuid        PRIMARY KEY,
    workspace_id       uuid        NOT NULL,
    room_id            uuid        NOT NULL,
    -- Per-room summary version, 1-based; a regenerate creates version + 1.
    version            integer     NOT NULL,
    -- Version of the transcript the job read (idempotency of the automatic job).
    transcript_version integer     NOT NULL DEFAULT 1,
    trigger            text        NOT NULL CHECK (trigger IN ('auto', 'manual')),
    status             text        NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'completed', 'failed')),
    model              text,
    prompt_version     text,
    -- {summaryShort, summaryDetailed, decisions[], actionItems[], openQuestions[], topics[], language, meta}
    content            jsonb,
    error_code         text,
    error              text,
    attempts           integer     NOT NULL DEFAULT 0,
    requested_by       uuid,
    input_segments     integer,
    tokens_in          bigint      NOT NULL DEFAULT 0,
    tokens_out         bigint      NOT NULL DEFAULT 0,
    created_at         timestamptz NOT NULL DEFAULT now(),
    started_at         timestamptz,
    completed_at       timestamptz,
    updated_at         timestamptz NOT NULL DEFAULT now(),
    UNIQUE (room_id, version)
);

-- At most one automatic job per room and transcript version.
CREATE UNIQUE INDEX IF NOT EXISTS ux_meeting_summaries_auto
    ON assistant.meeting_summaries (room_id, transcript_version)
    WHERE trigger = 'auto';
CREATE INDEX IF NOT EXISTS ix_meeting_summaries_open
    ON assistant.meeting_summaries (status, updated_at)
    WHERE status IN ('pending', 'running');

CREATE TABLE IF NOT EXISTS assistant.action_items (
    id                 uuid        PRIMARY KEY,
    workspace_id       uuid        NOT NULL,
    room_id            uuid        NOT NULL,
    summary_id         uuid        NOT NULL,
    task               text        NOT NULL,
    -- Only when literally present in the transcript (validated), else NULL.
    owner              text,
    -- Deadline as spoken (verbatim text), never parsed.
    due                text,
    status             text        NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'done')),
    source_segment_ids uuid[]      NOT NULL DEFAULT '{}',
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_action_items_room ON assistant.action_items (room_id, summary_id);
CREATE INDEX IF NOT EXISTS ix_action_items_ws_status
    ON assistant.action_items (workspace_id, status, created_at DESC);
