CREATE SCHEMA IF NOT EXISTS workspace;

CREATE TABLE IF NOT EXISTS workspace.workspaces (
    id                                   uuid PRIMARY KEY,
    name                                 varchar(150) NOT NULL,
    slug                                 varchar(100) NOT NULL,
    owner_id                             uuid NOT NULL,
    logo_url                             varchar(500),
    allow_external_collaboration         boolean NOT NULL DEFAULT false,
    require_verified_domain_for_internal boolean NOT NULL DEFAULT true,
    allow_subdomains                     boolean NOT NULL DEFAULT false,
    settings                             jsonb NOT NULL DEFAULT '{}'::jsonb,
    is_active                            boolean NOT NULL DEFAULT true,
    created_at                           timestamptz NOT NULL,
    created_by                           uuid,
    updated_at                           timestamptz,
    updated_by                           uuid,
    deleted_at                           timestamptz,
    deleted_by                           uuid
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_workspaces_slug ON workspace.workspaces (slug);

CREATE TABLE IF NOT EXISTS workspace.workspace_members (
    id                  uuid PRIMARY KEY,
    workspace_id        uuid NOT NULL,
    user_id             uuid NOT NULL,
    role_id             uuid NOT NULL,
    membership_type     varchar(20) NOT NULL DEFAULT 'internal',
    status              varchar(20) NOT NULL DEFAULT 'active',
    can_create_meetings boolean NOT NULL DEFAULT true,
    joined_at           timestamptz NOT NULL,
    removed_at          timestamptz,
    removed_by          uuid
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_workspace_members_ws_user
    ON workspace.workspace_members (workspace_id, user_id);
CREATE INDEX IF NOT EXISTS ix_workspace_members_user_active
    ON workspace.workspace_members (user_id) WHERE status = 'active';
