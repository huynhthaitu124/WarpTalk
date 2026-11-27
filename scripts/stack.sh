#!/bin/bash
# One command for testers: the whole WarpTalk backend in Docker (postgres, redis, qdrant, livekit,
# pipeline, migrator, api, worker, web). No Rust/Node toolchain needed on the host.
#
#   scripts/stack.sh up       # build + start everything, wait until the API answers
#   scripts/stack.sh down     # stop (data volumes are kept)
#   scripts/stack.sh logs [service...]
#   scripts/stack.sh status
#
# Ports on the host (change in deploy/docker/.env): API 8090, web 3000, LiveKit 7880/7881 + UDP 50000-50100,
# Postgres 5432, Redis 6379, Qdrant 6333/6334. The macOS app talks to http://localhost:8090 by default.
set -euo pipefail

cd "$(dirname "$0")/.."
COMPOSE_FILE=deploy/docker/docker-compose.yml
ENV_FILE=deploy/docker/.env
CMD="${1:-up}"; shift || true

say()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!! \033[0m %s\n' "$*"; }
die()  { printf '\033[1;31mLOI:\033[0m %s\n' "$*" >&2; exit 1; }

command -v docker >/dev/null || die "can Docker Desktop (https://docker.com)"
docker info >/dev/null 2>&1 || die "Docker chua chay. Mo Docker Desktop roi chay lai."

compose() { docker compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" --profile apps "$@"; }

env_get() { grep -E "^$1=" "$ENV_FILE" | tail -n1 | cut -d= -f2- || true; }
env_set() { # $1 name, $2 value (replaces or appends)
  local tmp; tmp="$(mktemp)"
  grep -v -E "^$1=" "$ENV_FILE" >"$tmp" || true
  printf '%s=%s\n' "$1" "$2" >>"$tmp"
  cat "$tmp" >"$ENV_FILE"; rm -f "$tmp"
}

prepare_env() {
  if [ ! -f "$ENV_FILE" ]; then
    cp deploy/docker/.env.example "$ENV_FILE"
    say "tao $ENV_FILE tu .env.example"
  fi
  # Values older .env.example files shipped that now break the stack (Vite-era web port, API on :8080).
  if [ "$(env_get WEB_PORT)" = "5173" ]; then env_set WEB_PORT 3000; say "WEB_PORT 5173 -> 3000 (gia tri cu)"; fi
  [ -n "$(env_get API_PORT)" ] || env_set API_PORT 8090
  [ -n "$(env_get JWT_SECRET)" ] || { env_set JWT_SECRET "$(openssl rand -base64 48 | tr -d '\n=+/')"; say "da sinh JWT_SECRET"; }
  [ -n "$(env_get AI_KEYS_KEK)" ] || { env_set AI_KEYS_KEK "$(openssl rand -base64 32)"; say "da sinh AI_KEYS_KEK"; }
  if [ -z "$(env_get OPENAI_API_KEY)" ]; then
    warn "OPENAI_API_KEY trong $ENV_FILE dang trong: WarpBot/pill se KHONG co transcript va goi y."
    warn "Dien key vao $ENV_FILE roi chay lai: scripts/stack.sh up"
  fi
}

# Something else on a host port = the container fails to start, or worse, the app talks to the wrong server.
check_ports() {
  command -v lsof >/dev/null || return 0
  local p who bad=0
  for p in "$(env_get API_PORT)" "$(env_get WEB_PORT)" 7880 "$(env_get POSTGRES_PORT)" "$(env_get REDIS_PORT)"; do
    [ -n "$p" ] || continue
    who="$(lsof -nP -iTCP:"$p" -sTCP:LISTEN 2>/dev/null | awk 'NR>1{print $1}' | sort -u | tr '\n' ' ')"
    case "$who" in ""|*com.docke*|*docker*|*vpnkit*) ;; *) warn "cong $p dang bi chiem boi: $who"; bad=1 ;; esac
  done
  [ "$bad" = 0 ] || die "tat chuong trinh tren hoac doi cong trong $ENV_FILE (API_PORT, WEB_PORT, POSTGRES_PORT, REDIS_PORT)"
}

wait_api() {
  local port url i; port="$(env_get API_PORT)"; url="http://localhost:${port:-8090}/health"
  say "doi API tai $url"
  for i in $(seq 1 90); do
    curl -fsS "$url" >/dev/null 2>&1 && return 0
    sleep 2
  done
  die "API khong len sau 3 phut. Xem: scripts/stack.sh logs api migrator"
}

case "$CMD" in
  up)
    prepare_env
    # Ports held by our own running stack are fine; only check on a cold start.
    [ -n "$(compose ps --status running -q api 2>/dev/null || true)" ] || check_ports
    say "build + chay stack (lan dau mat vai phut: build Rust + pipeline LiveKit)"
    compose up -d --build "$@"
    wait_api
    cat <<EOF

WarpTalk dang chay:
  API     : http://localhost:$(env_get API_PORT)   (app macOS: Settings > Backend URL = gia tri nay)
  Web     : http://localhost:$(env_get WEB_PORT)
  LiveKit : ws://localhost:7880
Dung lai : scripts/stack.sh down     Log: scripts/stack.sh logs api pipeline
EOF
    ;;
  down)   compose down "$@" ;;
  logs)   compose logs -f --tail=200 "$@" ;;
  status) compose ps ;;
  *) sed -n '2,11p' "$0"; exit 2 ;;
esac
