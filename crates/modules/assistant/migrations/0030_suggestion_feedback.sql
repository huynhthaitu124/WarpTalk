-- Suggestion quality loop: a durable snapshot of every published live suggestion plus the
-- feedback participants give it. Not in 05a; schema `assistant` owns them. Ids of other modules
-- (workspace, room, user) are plain uuid columns.

CREATE TABLE IF NOT EXISTS assistant.suggestions (
    id             uuid        PRIMARY KEY,   -- the suggestion_id on the wire
    workspace_id   uuid        NOT NULL,
    room_id        uuid        NOT NULL,
    segment_id     text        NOT NULL,
    category       text        NOT NULL,
    severity       text        NOT NULL DEFAULT 'info',
    content        text        NOT NULL,
    detail         text        NOT NULL DEFAULT '',
    language       text        NOT NULL DEFAULT '',
    model          text        NOT NULL DEFAULT '',
    prompt_version text        NOT NULL DEFAULT '',
    confidence     real        NOT NULL DEFAULT 0,
    sources        jsonb       NOT NULL DEFAULT '[]'::jsonb,
    -- Transcript window the model saw (PII-scrubbed as sent): [{t_ms, channel, speaker, text}]
    context        jsonb       NOT NULL DEFAULT '[]'::jsonb,
    created_at     timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS suggestions_ws_created_idx
    ON assistant.suggestions (workspace_id, created_at);
CREATE INDEX IF NOT EXISTS suggestions_room_idx
    ON assistant.suggestions (room_id);

CREATE TABLE IF NOT EXISTS assistant.suggestion_feedback (
    id            uuid        PRIMARY KEY,
    suggestion_id uuid        NOT NULL REFERENCES assistant.suggestions (id) ON DELETE CASCADE,
    workspace_id  uuid        NOT NULL,
    room_id       uuid        NOT NULL,
    user_id       uuid        NOT NULL,
    action        text        NOT NULL
        CHECK (action IN ('used', 'dismissed', 'helpful', 'not_helpful', 'wrong')),
    comment       text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    -- One verdict per user and action; repeating it updates the comment.
    UNIQUE (suggestion_id, user_id, action)
);

CREATE INDEX IF NOT EXISTS suggestion_feedback_ws_created_idx
    ON assistant.suggestion_feedback (workspace_id, created_at);
CREATE INDEX IF NOT EXISTS suggestion_feedback_action_idx
    ON assistant.suggestion_feedback (action, created_at);
