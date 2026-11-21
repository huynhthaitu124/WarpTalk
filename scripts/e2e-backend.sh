#!/usr/bin/env bash
# Real backend for the Playwright E2E suite (apps/web/e2e).
#   1. scripts/dev-services.sh  -> local Postgres + Redis
#   2. build + run warptalk-migrator
#   3. exec warptalk-api on 127.0.0.1:18080 with a random JWT_SECRET, LiveKit deliberately unset
#      (meeting join answers 503 LIVEKIT_NOT_CONFIGURED -> the "not configured" UI state is tested).
#
# Email verification: the API only logs the token length, so the Playwright fixture marks users
# verified directly in Postgres via psql (TEST-ONLY, see apps/web/e2e/fixtures/db.ts).
#
# Env overrides: E2E_API_BIND (default 127.0.0.1:18080), E2E_SKIP_BUILD=1 (use existing binaries).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

"$ROOT/scripts/dev-services.sh" >&2

export DATABASE_URL="${DATABASE_URL:-postgres://postgres@localhost:5432/warptalk}"
export REDIS_URL="${REDIS_URL:-redis://localhost:6379}"
export HTTP_BIND="${E2E_API_BIND:-127.0.0.1:18080}"
export JWT_SECRET="$(head -c 48 /dev/urandom | base64 | tr -d '\n=+/' | head -c 64)"
export CORS_ORIGINS="${CORS_ORIGINS:-http://127.0.0.1:5199,http://localhost:5199}"
export RUST_LOG="${RUST_LOG:-warn}"
# TEST BACKEND ONLY: raw verification and invitation tokens go to Redis (dev:mail:*) so E2E can read them.
export DEV_MAIL_CAPTURE=1
unset LIVEKIT_URL LIVEKIT_API_KEY LIVEKIT_API_SECRET METRICS_BIND

if [ "${E2E_SKIP_BUILD:-0}" != "1" ]; then
  cargo build -p warptalk-migrator -p warptalk-api >&2
fi
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}/debug"

"$TARGET/warptalk-migrator" >&2
echo "e2e backend: starting warptalk-api on $HTTP_BIND" >&2
exec "$TARGET/warptalk-api"
