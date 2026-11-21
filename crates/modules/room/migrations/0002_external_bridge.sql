-- External bridge rooms (docs/architecture/external-meeting.md D2; spec 10 section 3.8).
-- `translation_room_type` already exists (05a): 'EXTERNAL_BRIDGE' marks a bridge room, every other
-- value is a native room. The bridge columns below are not in 05a yet (flagged in the task report).
ALTER TABLE translation_room.translation_rooms
    ADD COLUMN IF NOT EXISTS bridge_provider            varchar(20),
    ADD COLUMN IF NOT EXISTS bridge_meeting_code        varchar(255),
    ADD COLUMN IF NOT EXISTS bridge_join_url            varchar(1000),
    ADD COLUMN IF NOT EXISTS bridge_capturer_user_id    uuid,
    ADD COLUMN IF NOT EXISTS bridge_capturer_since      timestamptz,
    ADD COLUMN IF NOT EXISTS bridge_capturer_lease_at   timestamptz,
    ADD COLUMN IF NOT EXISTS bridge_audio_flowing       boolean,
    ADD COLUMN IF NOT EXISTS bridge_last_audio_at       timestamptz,
    ADD COLUMN IF NOT EXISTS bridge_audio_mode          varchar(10) NOT NULL DEFAULT 'voice',
    ADD COLUMN IF NOT EXISTS bridge_epoch               integer NOT NULL DEFAULT 0;

-- One open bridge room per workspace + provider + meeting code.
CREATE UNIQUE INDEX IF NOT EXISTS ux_translation_rooms_open_bridge_meeting
    ON translation_room.translation_rooms (workspace_id, bridge_provider, bridge_meeting_code)
    WHERE translation_room_type = 'EXTERNAL_BRIDGE'
      AND deleted_at IS NULL
      AND status NOT IN ('ENDED', 'CANCELLED', 'EXPIRED', 'FAILED');
