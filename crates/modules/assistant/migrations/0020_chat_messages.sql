-- WarpBot "ask the meeting" conversation history: one row per message, per user per room.
-- Ids from other modules (workspace, room, user) are plain uuid columns (no cross-schema FKs).

CREATE TABLE IF NOT EXISTS assistant.chat_messages (
    id           uuid        PRIMARY KEY,
    workspace_id uuid        NOT NULL,
    room_id      uuid        NOT NULL,
    user_id      uuid        NOT NULL,
    role         text        NOT NULL CHECK (role IN ('user', 'assistant')),
    -- ask: free question; recap: quick action "what did I miss".
    kind         text        NOT NULL DEFAULT 'ask' CHECK (kind IN ('ask', 'recap')),
    content      text        NOT NULL,
    -- [{kind, ref, label, tMs?, page?}] on assistant rows.
    citations    jsonb       NOT NULL DEFAULT '[]'::jsonb,
    model        text,
    scope        text,
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS chat_messages_thread_idx
    ON assistant.chat_messages (workspace_id, room_id, user_id, id DESC);
