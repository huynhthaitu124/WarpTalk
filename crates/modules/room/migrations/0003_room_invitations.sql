-- Invitations to a translation room by email (spec 05a: translation_room_invitations).
CREATE TABLE IF NOT EXISTS translation_room.translation_room_invitations (
    id uuid PRIMARY KEY,
    translation_room_id uuid NOT NULL,
    email varchar(255) NOT NULL,
    status varchar(50) NOT NULL DEFAULT 'PENDING',
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS translation_room_invitations_room_email_uq
    ON translation_room.translation_room_invitations (translation_room_id, email);
