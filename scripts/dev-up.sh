#!/usr/bin/env bash
# Dung moi truong DEV WarpTalk: ha tang docker + migrate + (tuy chon) chay api/worker/pipeline/web.
# Chay duoc tren macOS (bash 3.2) va Linux. Khong chua bi mat: key doc tu deploy/docker/.env.
#
# Mac dinh (WarpTalk = meeting assistant copilot, ADR 0001): postgres + redis + qdrant, api + worker.
# KHONG co LiveKit va KHONG co pipeline dich/dub; flag translationMeetings/dubbing/... tat.
#
#   scripts/dev-up.sh                 # ha tang + build + migrate, IN RA lenh de ban tu chay tung cai
#   scripts/dev-up.sh --start         # nhu tren, roi chay api+worker nen (log: ./.logs/)
#   scripts/dev-up.sh --start --web   # them Next.js dev server (npm run dev, cong 3000)
#   scripts/dev-up.sh --start --translation   # them LiveKit + pipeline dich/dub (an mac dinh)
#   scripts/dev-up.sh --stop          # dung cac tien trinh nen (khong dung docker)
#   scripts/dev-up.sh --down          # --stop + docker compose down (giu du lieu)
#
# Tuy chon: --translation  them docker profile "translation" (LiveKit) + build/chay pipeline. Muon
#                          thay giao dien dich hop thi con phai bat flag translationMeetings (docs/RUNBOOK.md)
#           --fake (mac dinh khi co --translation: pipeline MT/TTS gia, MEDIA_TRANSPORT=simulated, khong can key)
#           --real (pipeline dung OPENAI_API_KEY/CARTESIA_API_KEY trong .env; transport theo MEDIA_TRANSPORT)
#           --no-docker  bo qua docker (da co Postgres/Redis/...), --no-build  bo qua cargo build
# Bien moi truong: PROFILE=release (dung target/release), API_PORT khong dung: doi HTTP_BIND trong .env.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
COMPOSE_FILE="deploy/docker/docker-compose.yml"
ENV_FILE="deploy/docker/.env"
LOG_DIR="$ROOT/.logs"
PROFILE="${PROFILE:-debug}"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}/$PROFILE"

START=0; WEB=0; STOP=0; DOWN=0; MODE=fake; DOCKER=1; BUILD=1; TRANSLATION=0
for a in "$@"; do
  case "$a" in
    --start) START=1 ;;
    --web) WEB=1; START=1 ;;
    --stop) STOP=1 ;;
    --down) DOWN=1; STOP=1 ;;
    --translation) TRANSLATION=1 ;;
    --fake) MODE=fake ;;
    --real) MODE=real ;;
    --no-docker) DOCKER=0 ;;
    --no-build) BUILD=0 ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) echo "tuy chon khong biet: $a (xem --help)" >&2; exit 2 ;;
  esac
done

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mLOI:\033[0m %s\n' "$*" >&2; exit 1; }

# Profile "translation" (LiveKit) only when asked; --down always includes it so a previous --translation run is stopped too.
compose() {
  local prof=""
  if [ "$TRANSLATION" = 1 ] || [ "$DOWN" = 1 ]; then prof="--profile translation"; fi
  if [ -f "$ENV_FILE" ]; then
    # shellcheck disable=SC2086
    docker compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" $prof "$@"
  else
    # shellcheck disable=SC2086
    docker compose -f "$COMPOSE_FILE" $prof "$@"
  fi
}

stop_bg() {
  [ -d "$LOG_DIR" ] || return 0
  for pidfile in "$LOG_DIR"/*.pid; do
    [ -f "$pidfile" ] || continue
    pid="$(cat "$pidfile")"
    if kill -0 "$pid" 2>/dev/null; then
      say "dung $(basename "$pidfile" .pid) (pid $pid)"
      kill "$pid" 2>/dev/null || true
    fi
    rm -f "$pidfile"
  done
}

if [ "$STOP" = 1 ]; then
  stop_bg
  if [ "$DOWN" = 1 ]; then compose down; fi
  exit 0
fi

# ---------- kiem tra cong cu ----------
need() { command -v "$1" >/dev/null 2>&1 || die "thieu '$1' (xem docs/RUNBOOK.md muc 1)"; }
need cargo; need curl
[ "$DOCKER" = 1 ] && need docker
[ "$WEB" = 1 ] && need npm

# ---------- .env ----------
if [ ! -f "$ENV_FILE" ]; then
  cp deploy/docker/.env.example "$ENV_FILE"
  say "tao $ENV_FILE tu .env.example"
fi

# Dien gia tri sinh ngau nhien (neu de trong) vao .env de giu on dinh giua cac lan chay.
ensure_secret() { # $1 ten bien, $2 lenh sinh
  local name="$1" gen="$2" cur
  cur="$(grep -E "^${name}=" "$ENV_FILE" | head -n1 | cut -d= -f2- || true)"
  if [ -z "$cur" ]; then
    local val tmp
    val="$(eval "$gen")"
    tmp="$(mktemp)"
    grep -v -E "^${name}=" "$ENV_FILE" >"$tmp" || true
    printf '%s=%s\n' "$name" "$val" >>"$tmp"
    cat "$tmp" >"$ENV_FILE"; rm -f "$tmp"
    say "da sinh $name moi trong $ENV_FILE"
  fi
}
need openssl
ensure_secret JWT_SECRET "openssl rand -base64 48 | tr -d '\n=+/'"
ensure_secret AI_KEYS_KEK "openssl rand -base64 32"

set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

API_BIND="${HTTP_BIND:-127.0.0.1:${API_PORT:-8090}}"
API_URL="http://${API_BIND}"
WEB_PORT_V="${WEB_PORT:-3000}"
# next.config.ts rewrites /api/** to this backend (dev only).
export API_PROXY_TARGET="${API_PROXY_TARGET:-$API_URL}"

# ---------- ha tang ----------
if [ "$DOCKER" = 1 ]; then
  docker info >/dev/null 2>&1 || die "Docker daemon chua chay (mo Docker Desktop / OrbStack roi chay lai)"
  if [ "$TRANSLATION" = 1 ]; then say "docker compose up (postgres, redis, qdrant, livekit)"; else say "docker compose up (postgres, redis, qdrant)"; fi
  compose up -d --wait --wait-timeout 120
  say "cho Qdrant tai http://localhost:${QDRANT_HTTP_PORT:-6333}/readyz"
  ok=0
  for _ in $(seq 1 30); do
    if curl -sf -m 2 "http://localhost:${QDRANT_HTTP_PORT:-6333}/readyz" >/dev/null 2>&1; then ok=1; break; fi
    sleep 1
  done
  [ "$ok" = 1 ] || die "Qdrant khong san sang. Xem: docker compose -f $COMPOSE_FILE logs qdrant"
  if [ "$TRANSLATION" = 1 ]; then
    say "cho LiveKit tai http://localhost:7880"
    ok=0
    for _ in $(seq 1 30); do
      if [ "$(curl -s -m 2 http://localhost:7880 || true)" = "OK" ]; then ok=1; break; fi
      sleep 1
    done
    [ "$ok" = 1 ] || die "LiveKit khong tra 'OK' o :7880. Xem: docker compose -f $COMPOSE_FILE --profile translation logs livekit"
  fi
fi

# ---------- build + migrate ----------
BUILD_FLAGS=""
BINS="warptalk-migrator warptalk-api warptalk-worker"
[ "$TRANSLATION" = 1 ] && BINS="$BINS warptalk-pipeline"
[ "$PROFILE" = release ] && BUILD_FLAGS="--release"
if [ "$BUILD" = 1 ]; then
  say "cargo build ($BINS)"
  PKGS=""; for b in $BINS; do PKGS="$PKGS -p $b"; done
  # shellcheck disable=SC2086
  cargo build $BUILD_FLAGS $PKGS
fi
for b in $BINS; do
  [ -x "$TARGET/$b" ] || die "khong thay $TARGET/$b (chay lai khong co --no-build)"
done
say "migrate"
"$TARGET/warptalk-migrator"

# ---------- lenh chay ----------
ENVLOAD="set -a; . deploy/docker/.env; set +a"
if [ "$MODE" = fake ]; then
  PIPE_CMD="env -u OPENAI_API_KEY -u CARTESIA_API_KEY MEDIA_TRANSPORT=simulated PIPELINE_FAKE_PROVIDERS=1 SIM_PARTICIPANTS=\"\${SIM_PARTICIPANTS:-}\" $TARGET/warptalk-pipeline"
else
  PIPE_CMD="env -u PIPELINE_FAKE_PROVIDERS $TARGET/warptalk-pipeline"
  [ "$TRANSLATION" != 1 ] || [ -n "${OPENAI_API_KEY:-}" ] || echo "CANH BAO: --real nhung OPENAI_API_KEY trong $ENV_FILE dang trong" >&2
fi

run_bg() { # $1 ten, phan con lai: lenh (chuoi shell)
  local name="$1"; shift
  mkdir -p "$LOG_DIR"
  if [ -f "$LOG_DIR/$name.pid" ] && kill -0 "$(cat "$LOG_DIR/$name.pid")" 2>/dev/null; then
    say "$name da chay (pid $(cat "$LOG_DIR/$name.pid"))"; return
  fi
  nohup bash -c "$*" >"$LOG_DIR/$name.log" 2>&1 &
  echo $! >"$LOG_DIR/$name.pid"
  say "$name da chay nen (pid $!, log $LOG_DIR/$name.log)"
}

if [ "$START" = 1 ]; then
  run_bg api "exec $TARGET/warptalk-api"
  say "cho API $API_URL/health"
  ok=0
  for _ in $(seq 1 60); do
    if curl -sf -m 2 "$API_URL/health" >/dev/null 2>&1; then ok=1; break; fi
    sleep 1
  done
  [ "$ok" = 1 ] || die "API khong len. Xem $LOG_DIR/api.log"
  run_bg worker "exec $TARGET/warptalk-worker"
  if [ "$TRANSLATION" = 1 ]; then run_bg pipeline "exec $PIPE_CMD"; fi
  if [ "$WEB" = 1 ]; then
    [ -d apps/web/node_modules ] || (cd apps/web && npm ci)
    run_bg web "cd apps/web && exec npm run dev -- -p ${WEB_PORT_V}"
  fi
  sleep 2
  for n in worker $([ "$TRANSLATION" = 1 ] && echo pipeline); do
    if ! kill -0 "$(cat "$LOG_DIR/$n.pid")" 2>/dev/null; then
      echo "CANH BAO: $n da thoat. Xem $LOG_DIR/$n.log" >&2
    fi
  done
fi

cat <<EOT

================ SAN SANG ================
API        : $API_URL        (health: $API_URL/health)
$([ "$TRANSLATION" = 1 ] && echo "LiveKit    : ${LIVEKIT_URL:-ws://localhost:7880}  (devkey/secret)" || echo "LiveKit    : tat (copilot; them --translation de bat)")
Web        : http://localhost:${WEB_PORT_V}   (Next.js, sau khi chay 'npm run dev')
Nap env    : $ENVLOAD

Lenh chay tung thanh phan (moi cai mot terminal, tai thu muc goc repo):
  1) API      : $ENVLOAD; $TARGET/warptalk-api
  2) Worker   : $ENVLOAD; $TARGET/warptalk-worker
$([ "$TRANSLATION" = 1 ] && echo "  3) Pipeline : $ENVLOAD; $PIPE_CMD")
  $([ "$TRANSLATION" = 1 ] && echo 4 || echo 3)) Web      : $ENVLOAD; cd apps/web && npm install && npm run dev -- -p ${WEB_PORT_V}
EOT
if [ "$TRANSLATION" != 1 ]; then
  :
elif [ "$MODE" = fake ]; then
  cat <<'EOT'
     (pipeline --fake: MT/TTS gia, transport mo phong. Muon "ai do noi" trong phong hay dat
      SIM_PARTICIPANTS=<participant id,...> truoc lenh; demo-seed.sh in ra id.)
EOT
else
  echo "     (pipeline --real: dung key that trong $ENV_FILE; MEDIA_TRANSPORT=${MEDIA_TRANSPORT:-simulated})"
fi
cat <<EOT

Du lieu demo : scripts/demo-seed.sh        (API_URL=$API_URL)
Dung nen     : scripts/dev-up.sh --stop    (log: $LOG_DIR/)
Dung het     : scripts/dev-up.sh --down
Huong dan    : docs/RUNBOOK.md
EOT
