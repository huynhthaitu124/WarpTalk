-- In-meeting chat (spec: 11 §5.4; columns per 05a meeting_chat_*).
CREATE TABLE IF NOT EXISTS meeting.meeting_chat_messages (
    id                  uuid PRIMARY KEY,
    meeting_room_id     uuid NOT NULL,
    workspace_id        uuid NOT NULL,
    sender_user_id      uuid,
    participant_id      uuid,
    sender_display_name varchar(255) NOT NULL,
    sender_type         varchar(50) NOT NULL DEFAULT 'user',
    message_type        varchar(50) NOT NULL DEFAULT 'text',
    original_language   varchar(50) NOT NULL,
    original_text       text NOT NULL,
    translation_enabled boolean NOT NULL DEFAULT false,
    mentions            jsonb NOT NULL DEFAULT '[]',
    is_hidden           boolean NOT NULL DEFAULT false,
    file_url            text,
    file_name           varchar(255),
    file_size_bytes     bigint,
    content_type        varchar(255),
    -- (proposal) client-generated id: makes POST idempotent per sender.
    client_message_id   varchar(100),
    created_at          timestamptz NOT NULL,
    updated_at          timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS ix_chat_messages_room_created
    ON meeting.meeting_chat_messages (meeting_room_id, created_at, id);
CREATE UNIQUE INDEX IF NOT EXISTS ux_chat_messages_client_id
    ON meeting.meeting_chat_messages (meeting_room_id, sender_user_id, client_message_id)
    WHERE client_message_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS meeting.meeting_chat_translations (
    id              uuid PRIMARY KEY,
    message_id      uuid NOT NULL,
    meeting_room_id uuid NOT NULL,
    source_language varchar(50) NOT NULL,
    target_language varchar(50) NOT NULL,
    translated_text text NOT NULL,
    model_used      varchar(100),
    confidence      numeric,
    prompt_version  integer NOT NULL DEFAULT 1,
    created_at      timestamptz NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_chat_translations_key
    ON meeting.meeting_chat_translations (message_id, target_language, prompt_version);
CREATE INDEX IF NOT EXISTS ix_chat_translations_room
    ON meeting.meeting_chat_translations (meeting_room_id);

-- Moderation audit trail (spec: 11 §4.1 `meeting_chat_moderation_events`, action `hidden`).
CREATE TABLE IF NOT EXISTS meeting.meeting_chat_moderation_events (
    id                   uuid PRIMARY KEY,
    message_id           uuid NOT NULL,
    meeting_room_id      uuid NOT NULL,
    moderated_by_user_id uuid NOT NULL,
    action               varchar(50) NOT NULL DEFAULT 'hidden',
    reason               text,
    created_at           timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_chat_moderation_message
    ON meeting.meeting_chat_moderation_events (message_id);
