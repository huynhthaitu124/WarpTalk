-- Settings fields required by the Auth API surface (06-service-design 1.2) but absent from 05a.
ALTER TABLE auth.user_settings
    ADD COLUMN default_audio_input    varchar(255),
    ADD COLUMN default_audio_output   varchar(255),
    ADD COLUMN auto_translate_enabled boolean NOT NULL DEFAULT true;

-- Role required for the `sa` JWT claim.
INSERT INTO auth.roles (id, name, description, is_system, is_active, created_at)
VALUES (gen_random_uuid(), 'SYSTEM_ADMIN', 'Platform administrator', true, true, now())
ON CONFLICT (name) DO NOTHING;
