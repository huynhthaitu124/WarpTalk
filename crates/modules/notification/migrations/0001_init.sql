CREATE SCHEMA IF NOT EXISTS notification;

CREATE TABLE notification.notification_preferences (
    id                uuid         PRIMARY KEY,
    user_id           uuid         NOT NULL,
    notification_type varchar(50)  NOT NULL,
    email_enabled     boolean      NOT NULL DEFAULT true,
    push_enabled      boolean      NOT NULL DEFAULT true,
    in_app_enabled    boolean      NOT NULL DEFAULT true,
    updated_at        timestamptz  NOT NULL,
    CONSTRAINT notification_preferences_user_type_key UNIQUE (user_id, notification_type)
);
