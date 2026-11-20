CREATE SCHEMA IF NOT EXISTS auth;

CREATE TABLE auth.users (
    id                                  uuid PRIMARY KEY,
    email                               varchar(320) NOT NULL UNIQUE,
    password_hash                       varchar(255),
    full_name                           varchar(150) NOT NULL,
    avatar_url                          varchar(500),
    phone                               varchar(20),
    preferred_language                  varchar(15) NOT NULL DEFAULT 'vi-VN',
    timezone                            varchar(50) NOT NULL DEFAULT 'Asia/Ho_Chi_Minh',
    is_active                           boolean NOT NULL DEFAULT true,
    is_locked                           boolean NOT NULL DEFAULT false,
    failed_login_attempts               integer NOT NULL DEFAULT 0,
    locked_until                        timestamptz,
    email_verified                      boolean NOT NULL DEFAULT false,
    email_verified_at                   timestamptz,
    google_id                           varchar(255) UNIQUE,
    last_login_at                       timestamptz,
    last_login_ip                       varchar(45),
    email_verification_token_hash       varchar(64),
    email_verification_token_expires_at timestamptz,
    password_reset_token_hash           varchar(64),
    password_reset_token_expires_at     timestamptz,
    created_at                          timestamptz NOT NULL,
    created_by                          uuid,
    updated_at                          timestamptz,
    updated_by                          uuid,
    deleted_at                          timestamptz,
    deleted_by                          uuid
);
CREATE INDEX users_email_verification_token_hash_idx
    ON auth.users (email_verification_token_hash) WHERE email_verification_token_hash IS NOT NULL;
CREATE INDEX users_password_reset_token_hash_idx
    ON auth.users (password_reset_token_hash) WHERE password_reset_token_hash IS NOT NULL;

CREATE TABLE auth.user_settings (
    id                            uuid PRIMARY KEY,
    user_id                       uuid NOT NULL UNIQUE,
    default_speak_language        varchar(15) NOT NULL DEFAULT 'vi-VN',
    default_listen_language       varchar(15) NOT NULL DEFAULT 'en-US',
    voice_clone_enabled           boolean NOT NULL DEFAULT false,
    mic_noise_suppression         boolean NOT NULL DEFAULT true,
    default_translation_room_type varchar(20) NOT NULL DEFAULT 'group',
    auto_record_translation_rooms boolean NOT NULL DEFAULT false,
    auto_generate_summary         boolean NOT NULL DEFAULT true,
    default_max_participants      integer NOT NULL DEFAULT 10,
    theme                         varchar(10) NOT NULL DEFAULT 'system',
    transcript_font_size          integer NOT NULL DEFAULT 14,
    show_original_transcript      boolean NOT NULL DEFAULT true,
    show_translated_transcript    boolean NOT NULL DEFAULT true,
    high_contrast                 boolean NOT NULL DEFAULT false,
    screen_reader_mode            boolean NOT NULL DEFAULT false,
    updated_at                    timestamptz NOT NULL,
    updated_by                    uuid
);

CREATE TABLE auth.roles (
    id          uuid PRIMARY KEY,
    name        varchar(50) NOT NULL UNIQUE,
    description varchar(255),
    is_system   boolean NOT NULL DEFAULT false,
    is_active   boolean NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL,
    created_by  uuid,
    updated_at  timestamptz,
    updated_by  uuid,
    deleted_at  timestamptz,
    deleted_by  uuid
);

CREATE TABLE auth.permissions (
    id          uuid PRIMARY KEY,
    code        varchar(100) NOT NULL UNIQUE,
    description varchar(255),
    group_name  varchar(50) NOT NULL,
    is_active   boolean NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL,
    created_by  uuid,
    updated_at  timestamptz,
    updated_by  uuid,
    deleted_at  timestamptz,
    deleted_by  uuid
);

CREATE TABLE auth.user_roles (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL,
    role_id     uuid NOT NULL,
    assigned_at timestamptz NOT NULL,
    assigned_by uuid,
    revoked_at  timestamptz,
    revoked_by  uuid
);
CREATE UNIQUE INDEX user_roles_user_role_uq ON auth.user_roles (user_id, role_id);

CREATE TABLE auth.role_permissions (
    role_id       uuid NOT NULL,
    permission_id uuid NOT NULL,
    created_at    timestamptz NOT NULL,
    created_by    uuid,
    PRIMARY KEY (role_id, permission_id)
);

CREATE TABLE auth.refresh_tokens (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL,
    token_hash  varchar(255) NOT NULL UNIQUE,
    family_id   uuid NOT NULL DEFAULT gen_random_uuid(),
    device_info varchar(255),
    ip_address  varchar(45),
    expires_at  timestamptz NOT NULL,
    revoked_at  timestamptz,
    created_at  timestamptz NOT NULL
);
CREATE INDEX refresh_tokens_family_idx ON auth.refresh_tokens (family_id);
CREATE INDEX refresh_tokens_user_idx ON auth.refresh_tokens (user_id);
