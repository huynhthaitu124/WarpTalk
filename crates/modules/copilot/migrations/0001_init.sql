-- Copilot sessions (ADR 0001). A session is a room of type COPILOT_SESSION (schema
-- translation_room) plus the facts below; `room_id` is a plain uuid (no cross-schema FK).
CREATE SCHEMA IF NOT EXISTS copilot;

CREATE TABLE IF NOT EXISTS copilot.sessions (
    room_id              uuid PRIMARY KEY,
    workspace_id         uuid NOT NULL,
    host_id              uuid NOT NULL,
    host_participant_id  uuid NOT NULL,
    others_speaker_id    uuid NOT NULL,
    title                varchar(255) NOT NULL,
    platform             varchar(20) NOT NULL,
    meeting_url          varchar(1000),
    language_me          varchar(15) NOT NULL,
    language_others      varchar(15) NOT NULL,
    name_me              varchar(100) NOT NULL,
    name_others          varchar(100) NOT NULL,
    created_at           timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS ix_copilot_sessions_workspace
    ON copilot.sessions (workspace_id, created_at DESC);
CREATE INDEX IF NOT EXISTS ix_copilot_sessions_host
    ON copilot.sessions (workspace_id, host_id, created_at DESC);
