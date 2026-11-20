CREATE TABLE IF NOT EXISTS public.outbox (
    id           uuid PRIMARY KEY,
    stream       text NOT NULL,
    payload      jsonb NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    published_at timestamptz NULL,
    attempts     int NOT NULL DEFAULT 0,
    last_error   text NULL
);

CREATE INDEX IF NOT EXISTS outbox_unpublished_created_at_idx
    ON public.outbox (created_at)
    WHERE published_at IS NULL;
