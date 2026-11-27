-- Meeting minutes (biên bản họp), drafted from the room's AI summary and edited by the host.
-- One row per version; exactly one current row per room. `content` is the web's JSON document,
-- stored verbatim as text so fields this server does not know survive a round trip.
CREATE SCHEMA IF NOT EXISTS assistant;

CREATE TABLE IF NOT EXISTS assistant.meeting_minutes (
    id                       uuid         PRIMARY KEY,
    workspace_id             uuid         NOT NULL,
    room_id                  uuid         NOT NULL,
    minutes_no               varchar(40)  NOT NULL,
    meeting_title            varchar(300),
    status                   varchar(20)  NOT NULL DEFAULT 'DRAFT',
    version                  integer      NOT NULL DEFAULT 1,
    is_current               boolean      NOT NULL DEFAULT true,
    previous_minutes_id      uuid,
    based_on_summary_id      uuid,
    drafted_by_engine        varchar(100),
    drafted_at               timestamptz,
    secretary_participant_id uuid,
    secretary_signed_at      timestamptz,
    chair_user_id            uuid,
    chair_approved_at        timestamptz,
    edit_count_vs_draft      integer      NOT NULL DEFAULT 0,
    content                  text         NOT NULL,
    created_by               uuid         NOT NULL,
    created_at               timestamptz  NOT NULL,
    updated_at               timestamptz  NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_meeting_minutes_current
    ON assistant.meeting_minutes (room_id) WHERE is_current;
CREATE INDEX IF NOT EXISTS ix_meeting_minutes_workspace
    ON assistant.meeting_minutes (workspace_id, created_at DESC);
