CREATE SCHEMA IF NOT EXISTS meeting;

CREATE TABLE IF NOT EXISTS meeting.meeting_rooms (
    id                  uuid PRIMARY KEY,
    translation_room_id uuid NOT NULL,
    provider_room_name  varchar(255) NOT NULL,
    active_host_id      uuid,
    status              varchar(255) NOT NULL DEFAULT 'CREATED',
    is_locked           boolean NOT NULL DEFAULT false,
    mute_on_entry       boolean NOT NULL DEFAULT false,
    active_egress_id    text,
    is_current          boolean NOT NULL DEFAULT true,
    is_active           boolean NOT NULL DEFAULT true,
    ended_at            timestamptz,
    created_at          timestamptz NOT NULL,
    created_by          uuid,
    updated_at          timestamptz,
    updated_by          uuid,
    deleted_at          timestamptz,
    deleted_by          uuid
);

-- Many rows per translation room are allowed, one live (current) session.
CREATE UNIQUE INDEX IF NOT EXISTS ux_meeting_rooms_current
    ON meeting.meeting_rooms (translation_room_id)
    WHERE is_current;
CREATE INDEX IF NOT EXISTS ix_meeting_rooms_provider_name
    ON meeting.meeting_rooms (provider_room_name);

CREATE TABLE IF NOT EXISTS meeting.rtc_stream_participants (
    id                              uuid PRIMARY KEY,
    meeting_room_id                 uuid NOT NULL,
    user_id                         uuid,
    provider_identity               varchar(255) NOT NULL,
    display_name                    varchar(255),
    translation_room_participant_id uuid,
    device_info                     varchar(255),
    is_active                       boolean NOT NULL DEFAULT true,
    joined_at                       timestamptz,
    left_at                         timestamptz,
    created_at                      timestamptz NOT NULL,
    created_by                      uuid,
    updated_at                      timestamptz,
    updated_by                      uuid,
    deleted_at                      timestamptz,
    deleted_by                      uuid
);

-- One RTC seat per identity per session (needed for idempotent upserts).
CREATE UNIQUE INDEX IF NOT EXISTS ux_rtc_participants_room_identity
    ON meeting.rtc_stream_participants (meeting_room_id, provider_identity);

CREATE TABLE IF NOT EXISTS meeting.meeting_tracks (
    id                     uuid PRIMARY KEY,
    meeting_participant_id uuid NOT NULL,
    provider_track_id      varchar(255) NOT NULL,
    media_type             varchar(255) NOT NULL DEFAULT 'VIDEO',
    is_muted               boolean NOT NULL DEFAULT false,
    is_active              boolean NOT NULL DEFAULT true,
    published_at           timestamptz,
    unpublished_at         timestamptz,
    created_at             timestamptz NOT NULL,
    created_by             uuid,
    updated_at             timestamptz,
    updated_by             uuid,
    deleted_at             timestamptz,
    deleted_by             uuid
);

-- Webhooks upsert by provider track id (spec: 11 §4).
CREATE UNIQUE INDEX IF NOT EXISTS ux_meeting_tracks_provider_track
    ON meeting.meeting_tracks (provider_track_id);
CREATE INDEX IF NOT EXISTS ix_meeting_tracks_participant
    ON meeting.meeting_tracks (meeting_participant_id);
