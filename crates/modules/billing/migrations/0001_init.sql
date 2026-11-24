-- Billing (spec 05a "Billing"): plans, one live subscription per workspace, credit ledger,
-- usage records, rate card, entitlement overrides. No cross-schema foreign keys.
-- Additive columns beyond 05a are marked (+).
CREATE SCHEMA IF NOT EXISTS subscription;

CREATE TABLE IF NOT EXISTS subscription.plans (
    id                            uuid PRIMARY KEY,
    name                          varchar(100) NOT NULL,
    slug                          varchar(50)  NOT NULL UNIQUE,
    tier                          varchar(20)  NOT NULL,
    price                         numeric(12,2) NOT NULL,
    currency                      char(3) NOT NULL DEFAULT 'USD',
    billing_cycle                 varchar(20) NOT NULL DEFAULT 'monthly',
    credits_per_cycle             integer NOT NULL,
    max_participants              integer NOT NULL DEFAULT 2,
    max_languages                 integer NOT NULL DEFAULT 2,
    max_active_rooms              integer NOT NULL DEFAULT 5,
    voice_clone_enabled           boolean NOT NULL DEFAULT false,
    ai_assistant_enabled          boolean NOT NULL DEFAULT false,
    glossary_enabled              boolean NOT NULL DEFAULT false,
    dedicated_gpu                 boolean NOT NULL DEFAULT false,
    overage_cap_credits           integer NOT NULL DEFAULT 0,
    overage_price_per_credit      numeric(18,10) NOT NULL DEFAULT 0.0001520913,
    low_balance_threshold_credits integer NOT NULL DEFAULT 0,
    rollover_cap_credits          integer NOT NULL DEFAULT 0,
    invoice_terms_days            integer NOT NULL DEFAULT 15,
    invoice_grace_hours           integer NOT NULL DEFAULT 360,
    features                      jsonb NOT NULL DEFAULT '{}',
    sort_order                    integer NOT NULL DEFAULT 0,
    is_active                     boolean NOT NULL DEFAULT true,
    created_at                    timestamptz NOT NULL DEFAULT now(),
    created_by                    uuid,
    updated_at                    timestamptz,
    updated_by                    uuid,
    deleted_at                    timestamptz,
    deleted_by                    uuid,
    stripe_price_id               varchar(255)            -- (+) fallback when no STRIPE_PRICE_<SLUG> env
);

CREATE TABLE IF NOT EXISTS subscription.subscriptions (
    id                                 uuid PRIMARY KEY,
    user_id                            uuid NOT NULL,
    workspace_id                       uuid,
    plan_id                            uuid NOT NULL,
    status                             varchar(20) NOT NULL DEFAULT 'active',
    credits_remaining                  integer NOT NULL DEFAULT 0,
    credits_used_this_cycle            integer NOT NULL DEFAULT 0,
    current_period_start               timestamptz NOT NULL,
    current_period_end                 timestamptz NOT NULL,
    auto_renew                         boolean NOT NULL DEFAULT true,
    cancellation_reason                text,
    cancelled_at                       timestamptz,
    trial_ends_at                      timestamptz,
    entitlement_overrides              jsonb,
    credits_per_cycle_override         integer,
    contract_price_usd                 numeric(14,2),
    overage_cap_credits_override       integer,
    overage_price_per_credit_override  numeric(18,10),
    low_balance_threshold_credits_override integer,
    invoice_terms_days_override        integer,
    billing_contact_email              varchar(255),
    overage_credits_this_cycle         integer NOT NULL DEFAULT 0,
    overage_started_at                 timestamptz,
    service_state                      varchar(20) NOT NULL DEFAULT 'healthy',
    suspended_reason                   varchar(30),
    owner_email_domain                 varchar(255),
    is_active                          boolean NOT NULL DEFAULT true,
    created_at                         timestamptz NOT NULL DEFAULT now(),
    created_by                         uuid,
    updated_at                         timestamptz,
    updated_by                         uuid,
    deleted_at                         timestamptz,
    deleted_by                         uuid,
    stripe_subscription_id             varchar(255)         -- (+)
);

-- BR-65: one live subscription per workspace.
CREATE UNIQUE INDEX IF NOT EXISTS ux_subscriptions_workspace_live
    ON subscription.subscriptions (workspace_id) WHERE is_active AND workspace_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_subscriptions_stripe_sub
    ON subscription.subscriptions (stripe_subscription_id) WHERE stripe_subscription_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS subscription.payments (
    id                      uuid PRIMARY KEY,
    subscription_id         uuid NOT NULL,
    user_id                 uuid NOT NULL,
    amount                  numeric(12,2) NOT NULL,
    tax_amount              numeric(12,2) NOT NULL DEFAULT 0,
    total_amount            numeric(12,2) NOT NULL,
    currency                char(3) NOT NULL DEFAULT 'USD',
    payment_method          varchar(30) NOT NULL,
    provider                varchar(30) NOT NULL DEFAULT 'stripe',
    provider_transaction_id varchar(1024) UNIQUE,
    provider_order_id       varchar(1024),
    status                  varchar(20) NOT NULL DEFAULT 'pending',
    failure_reason          varchar(500),
    provider_metadata       jsonb,
    paid_at                 timestamptz,
    refunded_at             timestamptz,
    created_at              timestamptz NOT NULL DEFAULT now(),
    updated_at              timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS subscription.invoices (
    id                  uuid PRIMARY KEY,
    payment_id          uuid NOT NULL,
    user_id             uuid,
    workspace_id        uuid,
    invoice_number      varchar(30) NOT NULL UNIQUE,
    stripe_invoice_id   varchar(255),
    amount              numeric(12,2),
    subtotal            numeric(12,2) NOT NULL,
    tax                 numeric(12,2) NOT NULL DEFAULT 0,
    total               numeric(12,2) NOT NULL,
    currency            char(3) NOT NULL DEFAULT 'USD',
    status              varchar(20) NOT NULL DEFAULT 'issued',
    pdf_url             varchar(500),
    invoice_pdf_url     varchar(500),
    hosted_invoice_url  varchar(500),
    line_items          jsonb NOT NULL DEFAULT '[]',
    issued_at           timestamptz NOT NULL,
    due_at              timestamptz NOT NULL,
    paid_at             timestamptz,
    created_at          timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_invoices_stripe
    ON subscription.invoices (stripe_invoice_id) WHERE stripe_invoice_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS subscription.credit_transactions (
    id                          uuid PRIMARY KEY,
    subscription_id             uuid NOT NULL,
    user_id                     uuid NOT NULL,
    workspace_id                uuid,
    amount                      integer NOT NULL,
    type                        varchar(20) NOT NULL,
    charge_type                 varchar(30),
    description                 varchar(255),
    reference_id                uuid,
    reference_type              varchar(30),
    balance_after               integer NOT NULL,
    pricing_rate_card_id        uuid,
    usage_record_id             uuid,
    invoice_id                  uuid,
    reversal_of_transaction_id  uuid UNIQUE,
    unit_price_snapshot         numeric(18,6),
    currency                    char(3),
    idempotency_key             varchar(255),
    correlation_id              varchar(100),
    status                      varchar(20) NOT NULL DEFAULT 'committed',
    triggered_by_participant_id uuid,
    transcript_segment_id       uuid,
    created_at                  timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_credit_tx_correlation
    ON subscription.credit_transactions (correlation_id, type) WHERE correlation_id IS NOT NULL;
-- NFR-RL-04: a charge/grant with the same idempotency key is applied once.
CREATE UNIQUE INDEX IF NOT EXISTS ux_credit_tx_idempotency
    ON subscription.credit_transactions (idempotency_key) WHERE idempotency_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_credit_tx_workspace
    ON subscription.credit_transactions (workspace_id, created_at DESC);

CREATE TABLE IF NOT EXISTS subscription.credit_balance_snapshots (
    id                      uuid PRIMARY KEY,
    subscription_id         uuid NOT NULL,
    credits_remaining       integer NOT NULL,
    credits_used_this_cycle integer NOT NULL,
    snapshot_at             timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_balance_snapshots_sub
    ON subscription.credit_balance_snapshots (subscription_id, snapshot_at DESC);

CREATE TABLE IF NOT EXISTS subscription.usage_records (
    id                  uuid PRIMARY KEY,
    subscription_id     uuid NOT NULL,
    user_id             uuid,
    workspace_id        uuid NOT NULL,
    translation_room_id uuid,
    segment_id          uuid,                                   -- RV-01: per-segment traceability
    usage_type          varchar(30) NOT NULL,
    unit                varchar(20) NOT NULL DEFAULT 'credit',
    quantity            numeric(12,4) NOT NULL DEFAULT 1,
    credits_consumed    integer NOT NULL,
    duration_seconds    integer,
    details             jsonb,
    recorded_at         timestamptz NOT NULL,
    event_id            varchar(255),                           -- (+) producer idempotency key
    model               varchar(100),                           -- (+)
    cost_source         varchar(10) NOT NULL DEFAULT 'platform' -- (+) platform | byo (byo-llm.md section 7)
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_usage_records_event
    ON subscription.usage_records (event_id) WHERE event_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_usage_records_workspace
    ON subscription.usage_records (workspace_id, recorded_at DESC);

CREATE TABLE IF NOT EXISTS subscription.usage_rate_card (
    id                   uuid PRIMARY KEY,
    charge_type          varchar(30) NOT NULL,   -- stt | llm_suggestion | llm_summary | llm_assistant | embedding
    unit                 varchar(30),            -- audio_second | 1k_tokens_in | 1k_tokens_out
    provider             varchar(50),
    model                varchar(100),           -- NULL = default for the charge type
    source_language_code varchar(15),
    target_language_code varchar(15),
    unit_price           numeric(18,6) NOT NULL, -- credits per unit
    provider_unit_cost   numeric(18,10),
    markup_multiplier    numeric(10,4),
    currency             char(3) NOT NULL,       -- 'CRD' = credits
    effective_from       timestamptz NOT NULL,
    effective_to         timestamptz,
    is_active            boolean NOT NULL DEFAULT true,
    notes                text
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_rate_card_live
    ON subscription.usage_rate_card (charge_type, COALESCE(unit, ''), currency, COALESCE(provider, ''),
        COALESCE(model, ''), COALESCE(source_language_code, ''), COALESCE(target_language_code, ''))
    WHERE is_active AND effective_to IS NULL;

CREATE TABLE IF NOT EXISTS subscription.workspace_entitlement_overrides (
    workspace_id    uuid NOT NULL,
    entitlement_key varchar(60) NOT NULL,
    value           varchar(40) NOT NULL,
    set_by          uuid,
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, entitlement_key)
);

-- (+) Stripe customer per workspace; lets webhooks resolve the workspace from the customer id.
CREATE TABLE IF NOT EXISTS subscription.customers (
    workspace_id       uuid PRIMARY KEY,
    stripe_customer_id varchar(255) NOT NULL UNIQUE,
    created_at         timestamptz NOT NULL DEFAULT now()
);

-- (+) Webhook idempotency (NFR-IF-07 / NFR-RL-04): one row per processed Stripe event id.
CREATE TABLE IF NOT EXISTS subscription.webhook_events (
    event_id     varchar(255) PRIMARY KEY,
    event_type   varchar(100) NOT NULL,
    received_at  timestamptz NOT NULL DEFAULT now()
);

-- (+) Audit trail of admin actions (rate card edits, manual credit adjustments).
CREATE TABLE IF NOT EXISTS subscription.admin_audit (
    id           uuid PRIMARY KEY,
    actor_id     uuid NOT NULL,
    action       varchar(40) NOT NULL,
    workspace_id uuid,
    details      jsonb NOT NULL DEFAULT '{}',
    created_at   timestamptz NOT NULL DEFAULT now()
);

-- Seed: plans (data, not logic). Prices/credits/limits/flags are editable rows.
INSERT INTO subscription.plans
    (id, name, slug, tier, price, credits_per_cycle, max_participants, max_languages, max_active_rooms,
     voice_clone_enabled, ai_assistant_enabled, glossary_enabled, overage_cap_credits,
     low_balance_threshold_credits, rollover_cap_credits, features, sort_order)
SELECT gen_random_uuid(), v.name, v.slug, v.tier, v.price, v.credits, v.participants, v.languages, v.rooms,
       v.voice, v.assistant, v.glossary, v.overage, v.low, v.rollover, v.features::jsonb, v.sort_order
FROM (VALUES
    ('Free trial', 'free',     'free',     0.00::numeric,      1800,   2, 2,  5, false, false, false,     0,   180,     0,
     '{"max_concurrent_copilot_sessions":1,"monthly_minutes":30,"byo_llm_enabled":false,"top_up_enabled":false,"trial_days":14}', 0),
    ('Pro',        'pro',      'paid',     29.00::numeric,    36000,  10, 5, 20, false, true,  true,      0,  3600,     0,
     '{"max_concurrent_copilot_sessions":2,"monthly_minutes":600,"byo_llm_enabled":false,"top_up_enabled":true}', 10),
    ('Business',   'business', 'business', 99.00::numeric,   180000, 50, 10, 100, true,  true,  true, 18000, 18000, 36000,
     '{"max_concurrent_copilot_sessions":5,"monthly_minutes":3000,"byo_llm_enabled":true,"top_up_enabled":true}', 20)
) AS v(name, slug, tier, price, credits, participants, languages, rooms, voice, assistant, glossary,
       overage, low, rollover, features, sort_order)
WHERE NOT EXISTS (SELECT 1 FROM subscription.plans p WHERE p.slug = v.slug);

-- Seed: rate card in credits. STT: per audio second (OI-03/RV-01). LLM: per 1K tokens.
INSERT INTO subscription.usage_rate_card (id, charge_type, unit, model, unit_price, currency, effective_from, notes)
SELECT gen_random_uuid(), v.charge_type, v.unit, NULL, v.price, 'CRD', now(), 'seed default (model-agnostic)'
FROM (VALUES
    ('stt',            'audio_second', 1.0::numeric),
    ('llm_suggestion', '1k_tokens_in', 1.0::numeric),
    ('llm_suggestion', '1k_tokens_out', 3.0::numeric),
    ('llm_summary',    '1k_tokens_in', 1.0::numeric),
    ('llm_summary',    '1k_tokens_out', 3.0::numeric),
    ('llm_assistant',  '1k_tokens_in', 1.0::numeric),
    ('llm_assistant',  '1k_tokens_out', 3.0::numeric),
    ('embedding',      '1k_tokens_in', 0.2::numeric)
) AS v(charge_type, unit, price)
WHERE NOT EXISTS (
    SELECT 1 FROM subscription.usage_rate_card r
    WHERE r.charge_type = v.charge_type AND r.unit = v.unit AND r.model IS NULL
      AND r.is_active AND r.effective_to IS NULL);
