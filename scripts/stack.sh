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
trap 'printf "\033[1;31mLOI:\033[0m stack.sh dung o dong %s (lenh: %s)\n" "$LINENO" "$BASH_COMMAND" >&2' ERR

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
# Prints the non-Docker processes listening on TCP port $1 (empty = free or held by our own stack).
port_holder() {
  # lsof exits 1 when nothing listens (the normal case on a clean machine); with pipefail that
  # would abort the whole script silently, so the pipeline must not fail.
  local who
  who="$(lsof -nP -iTCP:"$1" -sTCP:LISTEN 2>/dev/null | awk 'NR>1{print $1}' | sort -u | tr '\n' ' ' || true)"
  case "$who" in ""|*com.docke*|*docker*|*vpnkit*) ;; *) printf '%s' "$who" ;; esac
}
# Next port from $1 that nothing outside Docker holds and that is not in $TAKEN (already given out).
free_port() {
  local p="$1"
  while [ -n "$(port_holder "$p")" ] || [[ " $TAKEN " == *" $p "* ]]; do p=$((p + 1)); done
  printf '%s' "$p"
}

check_ports() {
  command -v lsof >/dev/null || return 0
  local p who var new bad=0
  TAKEN="$(for var in POSTGRES_PORT REDIS_PORT QDRANT_HTTP_PORT QDRANT_GRPC_PORT API_PORT WEB_PORT; do env_get "$var"; done | tr '\n' ' ') 7880 7881"
  # Infra ports are only host conveniences (containers talk by service name), so a local
  # Postgres/Redis/Qdrant from Homebrew just moves ours to the next free port.
  for var in POSTGRES_PORT REDIS_PORT QDRANT_HTTP_PORT QDRANT_GRPC_PORT; do
    p="$(env_get "$var")"; [ -n "$p" ] || continue
    who="$(port_holder "$p")"; [ -n "$who" ] || continue
    new="$(free_port "$((p + 1))")"
    env_set "$var" "$new"; TAKEN="$TAKEN $new"; warn "cong $p ($var) dang bi $who giu -> dung $new"
  done
  # These the app and the browser connect to directly: moving them silently would break things.
  for p in "$(env_get API_PORT)" "$(env_get WEB_PORT)" 7880 7881; do
    [ -n "$p" ] || continue
    who="$(port_holder "$p")"
    [ -z "$who" ] || { warn "cong $p dang bi chiem boi: $who"; bad=1; }
  done
  [ "$bad" = 0 ] || die "tat chuong trinh tren hoac doi cong trong $ENV_FILE (API_PORT, WEB_PORT; LiveKit can 7880/7881)"
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
