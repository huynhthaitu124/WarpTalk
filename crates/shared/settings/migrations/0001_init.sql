CREATE SCHEMA IF NOT EXISTS public;

-- One JSON document per (scope, namespace). Platform rows use the nil uuid as scope_id
-- so a plain primary key works.
CREATE TABLE IF NOT EXISTS public.settings_entries (
    scope_kind text        NOT NULL CHECK (scope_kind IN ('platform', 'workspace', 'room')),
    scope_id   uuid        NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000',
    namespace  text        NOT NULL,
    value      jsonb       NOT NULL CHECK (jsonb_typeof(value) = 'object'),
    version    bigint      NOT NULL DEFAULT 1,
    updated_by uuid        NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (scope_kind, scope_id, namespace),
    CHECK ((scope_kind = 'platform') = (scope_id = '00000000-0000-0000-0000-000000000000'))
);

CREATE INDEX IF NOT EXISTS settings_entries_namespace_idx
    ON public.settings_entries (namespace);

-- Append-only change log.
CREATE TABLE IF NOT EXISTS public.settings_audit (
    id         uuid        PRIMARY KEY,
    scope_kind text        NOT NULL,
    scope_id   uuid        NOT NULL,
    namespace  text        NOT NULL,
    old_value  jsonb       NULL,
    new_value  jsonb       NULL,
    version    bigint      NOT NULL,
    actor      uuid        NULL,
    changed_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS settings_audit_lookup_idx
    ON public.settings_audit (namespace, scope_kind, scope_id, changed_at DESC);

CREATE OR REPLACE FUNCTION public.settings_audit_immutable() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'public.settings_audit is append-only';
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS settings_audit_no_change ON public.settings_audit;
CREATE TRIGGER settings_audit_no_change
    BEFORE UPDATE OR DELETE ON public.settings_audit
    FOR EACH ROW EXECUTE FUNCTION public.settings_audit_immutable();
