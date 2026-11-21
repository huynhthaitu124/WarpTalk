CREATE TABLE IF NOT EXISTS workspace.workspace_invitations (
    id                  uuid PRIMARY KEY,
    workspace_id        uuid NOT NULL,
    email               varchar(320) NOT NULL,
    role_id             uuid NOT NULL,
    membership_type     varchar(20) NOT NULL DEFAULT 'internal',
    matched_domain_id   uuid,
    invited_by          uuid NOT NULL,
    token_hash          varchar(255),
    status              varchar(20) NOT NULL DEFAULT 'pending',
    expires_at          timestamptz NOT NULL,
    accepted_at         timestamptz,
    delivery_status     varchar(50) NOT NULL DEFAULT 'NotSent',
    provider_message_id varchar(255),
    last_sent_at        timestamptz,
    sent_count          integer NOT NULL DEFAULT 0,
    requested_by        uuid,
    reviewed_by         uuid,
    reviewed_at         timestamptz,
    created_at          timestamptz NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_workspace_invitations_token_hash
    ON workspace.workspace_invitations (token_hash);
-- At most one pending invitation per workspace and (case-insensitive) email.
CREATE UNIQUE INDEX IF NOT EXISTS ux_workspace_invitations_pending
    ON workspace.workspace_invitations (workspace_id, lower(email)) WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS ix_workspace_invitations_email
    ON workspace.workspace_invitations (lower(email)) WHERE status = 'pending';
