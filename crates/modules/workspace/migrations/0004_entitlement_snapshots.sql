-- Read model of the workspace's resolved entitlements (05a workspace.workspace_entitlement_snapshots).
-- Projected from billing events; never written by hand. `entitlements` is an object keyed by
-- entitlement key: { "<key>": { "kind", "value", "source", "ceiling", "ceilingSource" } }.
-- No row = billing has not published a snapshot for the workspace yet.

CREATE TABLE IF NOT EXISTS workspace.workspace_entitlement_snapshots (
    workspace_id             uuid PRIMARY KEY,
    entitlements             jsonb NOT NULL DEFAULT '{}',
    plan_slug                varchar(80),
    has_active_subscription  boolean NOT NULL DEFAULT false,
    resolved_at              timestamptz NOT NULL,
    last_event_id            uuid NOT NULL,
    updated_at               timestamptz NOT NULL
);
