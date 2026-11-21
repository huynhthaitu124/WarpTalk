CREATE SCHEMA IF NOT EXISTS assistant;

-- Per-workspace LLM override, one row per (workspace, purpose). Absent row = platform default.
CREATE TABLE assistant.workspace_llm_configs (
    id               uuid        PRIMARY KEY,
    workspace_id     uuid        NOT NULL,
    purpose          text        NOT NULL
        CHECK (purpose IN ('assistant', 'suggestion_decide', 'suggestion_generate', 'summary', 'embedding')),
    provider         text        NOT NULL
        CHECK (provider IN ('openai', 'anthropic', 'gemini', 'openai_compatible')),
    model            text        NOT NULL,
    base_url         text,
    reasoning        text        CHECK (reasoning IN ('off', 'low', 'medium', 'high')),
    max_output_tokens integer    CHECK (max_output_tokens > 0),
    timeout_ms       integer     CHECK (timeout_ms > 0),
    data_boundary    text        NOT NULL DEFAULT 'external'
        CHECK (data_boundary IN ('external', 'workspace_controlled')),
    fallback_policy  text        NOT NULL DEFAULT 'none'
        CHECK (fallback_policy IN ('none', 'platform_if_allowed')),
    enabled          boolean     NOT NULL DEFAULT true,
    status           text        NOT NULL DEFAULT 'invalid' CHECK (status IN ('ok', 'degraded', 'invalid')),
    last_validated_at timestamptz,
    last_error_code  text,
    -- Bumped on every change; resolvers cache built providers by (workspace, version).
    version          bigint      NOT NULL DEFAULT 1,
    created_by       uuid,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, purpose)
);

-- One API key per (workspace, provider), envelope-encrypted: AES-256-GCM(key) under a random
-- DEK; the DEK is wrapped (AES-256-GCM, nonce prepended) by the platform KEK. AAD of both is
-- workspace_id || provider. `last4` is the only fragment kept in clear, for display.
CREATE TABLE assistant.workspace_llm_credentials (
    id            uuid        PRIMARY KEY,
    workspace_id  uuid        NOT NULL,
    provider      text        NOT NULL
        CHECK (provider IN ('openai', 'anthropic', 'gemini', 'openai_compatible')),
    ciphertext    bytea       NOT NULL,
    nonce         bytea       NOT NULL,
    wrapped_dek   bytea       NOT NULL,
    key_version   integer     NOT NULL,
    last4         text        NOT NULL,
    created_by    uuid        NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    rotated_at    timestamptz,
    validated_at  timestamptz,
    UNIQUE (workspace_id, provider)
);

CREATE TABLE assistant.workspace_llm_budgets (
    workspace_id          uuid        NOT NULL,
    period                text        NOT NULL CHECK (period IN ('day', 'month')),
    max_input_tokens      bigint      CHECK (max_input_tokens >= 0),
    max_output_tokens     bigint      CHECK (max_output_tokens >= 0),
    max_est_cost_micro_usd bigint     CHECK (max_est_cost_micro_usd >= 0),
    hard_stop             boolean     NOT NULL DEFAULT false,
    alert_threshold_pct   integer     NOT NULL DEFAULT 80 CHECK (alert_threshold_pct BETWEEN 1 AND 100),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, period)
);
