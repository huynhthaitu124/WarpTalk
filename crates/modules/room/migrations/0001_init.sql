CREATE SCHEMA IF NOT EXISTS translation_room;

CREATE TABLE IF NOT EXISTS translation_room.translation_rooms (
    id                            uuid PRIMARY KEY,
    workspace_id                  uuid NOT NULL,
    host_id                       uuid NOT NULL,
    active_host_id                uuid,
    title                         varchar(255) NOT NULL,
    description                   text,
    translation_room_code         varchar(12) NOT NULL,
    status                        varchar(255) NOT NULL DEFAULT 'SCHEDULED',
    translation_room_type         varchar(20) NOT NULL DEFAULT 'GROUP',
    max_participants              integer NOT NULL DEFAULT 10,
    source_language               varchar(15) NOT NULL,
    target_languages              jsonb NOT NULL DEFAULT '[]'::jsonb,
    settings                      jsonb NOT NULL DEFAULT '{}'::jsonb,
    scheduled_at                  timestamptz,
    started_at                    timestamptz,
    ended_at                      timestamptz,
    duration_seconds              integer,
    reminder_30min_sent_at        timestamptz,
    reminder_10min_sent_at        timestamptz,
    reminder_1min_sent_at         timestamptz,
    series_id                     uuid,
    series_occurrence_local_date  date,
    is_active                     boolean NOT NULL DEFAULT true,
    created_at                    timestamptz NOT NULL,
    created_by                    uuid,
    updated_at                    timestamptz,
    updated_by                    uuid,
    deleted_at                    timestamptz,
    deleted_by                    uuid
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_translation_rooms_code
    ON translation_room.translation_rooms (translation_room_code)
    WHERE series_id IS NULL;
CREATE INDEX IF NOT EXISTS ix_translation_rooms_workspace
    ON translation_room.translation_rooms (workspace_id, created_at DESC);

CREATE TABLE IF NOT EXISTS translation_room.translation_room_participants (
    id                           uuid PRIMARY KEY,
    translation_room_id          uuid NOT NULL,
    user_id                      uuid,
    display_name                 varchar(100) NOT NULL,
    role                         varchar(20) NOT NULL DEFAULT 'participant',
    listen_language              varchar(15) NOT NULL,
    speak_language               varchar(15) NOT NULL,
    status                       varchar(255) NOT NULL DEFAULT 'INVITED',
    connection_type              varchar(20) NOT NULL DEFAULT 'webrtc',
    is_translation_audio_enabled boolean NOT NULL DEFAULT true,
    is_using_voice_clone         boolean NOT NULL DEFAULT false,
    joined_at                    timestamptz,
    left_at                      timestamptz,
    created_at                   timestamptz NOT NULL,
    updated_at                   timestamptz NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_translation_room_participants_user
    ON translation_room.translation_room_participants (translation_room_id, user_id)
    WHERE user_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_translation_room_participants_user
    ON translation_room.translation_room_participants (user_id);

CREATE TABLE IF NOT EXISTS translation_room.translation_room_sessions (
    id                   uuid PRIMARY KEY,
    translation_room_id  uuid NOT NULL,
    main_language        varchar(15) NOT NULL,
    audio_url            varchar(500),
    status               varchar(20) NOT NULL DEFAULT 'ACTIVE',
    started_at           timestamptz,
    ended_at             timestamptz,
    created_at           timestamptz NOT NULL,
    updated_at           timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS ix_translation_room_sessions_room
    ON translation_room.translation_room_sessions (translation_room_id, created_at);

CREATE TABLE IF NOT EXISTS translation_room.translation_room_audio_routes (
    id                    uuid PRIMARY KEY,
    translation_room_id   uuid NOT NULL,
    source_participant_id uuid NOT NULL,
    target_participant_id uuid NOT NULL,
    source_language       varchar(15) NOT NULL,
    target_language       varchar(15) NOT NULL,
    voice_clone_enabled   boolean NOT NULL DEFAULT false,
    stream_id             varchar(100),
    status                varchar(20) NOT NULL DEFAULT 'active',
    is_current            boolean NOT NULL DEFAULT true,
    started_at            timestamptz,
    ended_at              timestamptz
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_translation_room_audio_routes_pair
    ON translation_room.translation_room_audio_routes (source_participant_id, target_participant_id)
    WHERE is_current;
CREATE INDEX IF NOT EXISTS ix_translation_room_audio_routes_room
    ON translation_room.translation_room_audio_routes (translation_room_id)
    WHERE is_current;
