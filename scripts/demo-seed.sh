#!/usr/bin/env bash
# Tao du lieu demo qua API that: 2 user (da xac minh email), 1 workspace, moi + chap nhan user thu hai,
# 1 phong dich vi -> en, ja. In tai khoan dang nhap va ma phong.
#
# Yeu cau: API dang chay voi DEV_MAIL_CAPTURE=1 (scripts/dev-up.sh bat san), curl, jq,
#          redis-cli (hoac docker compose cho Redis cua deploy/docker).
# Bien moi truong (deu tuy chon):
#   API_URL        mac dinh http://127.0.0.1:8080   (co hay khong "/api/v1" deu duoc)
#   REDIS_URL      mac dinh redis://localhost:6379
#   DEMO_PASSWORD  mat khau cho ca hai user (mac dinh Demo!Pass123)
#   DEMO_HOST_EMAIL / DEMO_GUEST_EMAIL   co dinh email (mac dinh: ngau nhien moi lan chay)
#   DEMO_WORKSPACE ten workspace, DEMO_ROOM ten phong
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
API_URL="${API_URL:-http://127.0.0.1:8080}"
API_URL="${API_URL%/}"; API_URL="${API_URL%/api/v1}"
API="$API_URL/api/v1"
REDIS_URL="${REDIS_URL:-redis://localhost:6379}"
PASSWORD="${DEMO_PASSWORD:-Demo!Pass123}"
SUFFIX="$(LC_ALL=C tr -dc 'a-z0-9' </dev/urandom | head -c 6 || true)"
HOST_EMAIL="${DEMO_HOST_EMAIL:-demo-host-$SUFFIX@example.com}"
GUEST_EMAIL="${DEMO_GUEST_EMAIL:-demo-guest-$SUFFIX@example.com}"
WS_NAME="${DEMO_WORKSPACE:-Demo Co $SUFFIX}"
ROOM_TITLE="${DEMO_ROOM:-Demo vi-en-ja $SUFFIX}"

die() { printf 'LOI: %s\n' "$*" >&2; exit 1; }
for c in curl jq; do command -v "$c" >/dev/null 2>&1 || die "thieu '$c'"; done

# api METHOD PATH [TOKEN] [JSON]  -> in body, thoat !=0 neu HTTP >= 300
api() {
  local method="$1" path="$2" token="${3:-}" body="${4:-}" out code
  local args=(-sS -m 20 -X "$method" -H 'content-type: application/json' -w '\n%{http_code}')
  [ -n "$token" ] && args+=(-H "authorization: Bearer $token")
  [ -n "$body" ] && args+=(--data "$body")
  out="$(curl "${args[@]}" "$API$path")" || die "khong goi duoc $API$path (API co dang chay khong?)"
  code="${out##*$'\n'}"; out="${out%$'\n'*}"
  if [ "$code" -ge 300 ]; then
    printf '%s' "$out"
    echo >&2; echo "LOI: $method $path -> HTTP $code: $out" >&2
    return 1
  fi
  printf '%s' "$out"
}

redis_get() {
  local key="$1" v=""
  if command -v redis-cli >/dev/null 2>&1; then
    v="$(redis-cli -u "$REDIS_URL" --raw GET "$key" 2>/dev/null || true)"
  elif command -v docker >/dev/null 2>&1; then
    v="$(docker compose -f "$ROOT/deploy/docker/docker-compose.yml" exec -T redis redis-cli --raw GET "$key" 2>/dev/null || true)"
  fi
  printf '%s' "$v" | tr -d '\r\n'
}

# Doi token "gui email" xuat hien trong Redis (DEV_MAIL_CAPTURE=1), toi da ~6s.
wait_mail() {
  local key="$1" v="" i
  for i in $(seq 1 30); do
    v="$(redis_get "$key")"
    [ -n "$v" ] && { printf '%s' "$v"; return 0; }
    sleep 0.2
  done
  return 1
}

curl -sf -m 5 "$API_URL/health" >/dev/null || die "API khong tra loi o $API_URL/health"

register_verify_login() { # $1 email $2 ten -> in "userId accessToken"
  local email="$1" name="$2" reg uid tok
  if reg="$(api POST /auth/register '' "$(jq -nc --arg e "$email" --arg p "$PASSWORD" --arg n "$name" '{email:$e,password:$p,fullName:$n}')" 2>/dev/null)"; then
    uid="$(jq -r '.userId' <<<"$reg")"
    tok="$(wait_mail "dev:mail:verify:$(printf '%s' "$email" | tr 'A-Z' 'a-z')")" \
      || die "khong thay token xac minh trong Redis cho $email. API co DEV_MAIL_CAPTURE=1 va cung REDIS_URL ($REDIS_URL) khong?"
    api POST /auth/verify-email '' "$(jq -nc --arg t "$tok" '{token:$t}')" >/dev/null
  else
    echo "  ($email da ton tai, bo qua dang ky; dung mat khau dang dung)" >&2
    uid=""
  fi
  local login
  login="$(api POST /auth/login '' "$(jq -nc --arg e "$email" --arg p "$PASSWORD" '{email:$e,password:$p}')")" \
    || die "dang nhap $email that bai (sai mat khau? Dat DEMO_PASSWORD)"
  tok="$(jq -r '.accessToken' <<<"$login")"
  [ -n "$uid" ] || uid="$(api GET /me "$tok" | jq -r '.id // .userId')"
  printf '%s %s\n' "$uid" "$tok"
}

echo "1/6 Dang ky + xac minh + dang nhap 2 user" >&2
read -r HOST_ID HOST_TOKEN < <(register_verify_login "$HOST_EMAIL" "Demo Host")
read -r GUEST_ID GUEST_TOKEN < <(register_verify_login "$GUEST_EMAIL" "Demo Guest")

echo "2/6 Tao workspace '$WS_NAME'" >&2
WS="$(api POST /workspaces "$HOST_TOKEN" "$(jq -nc --arg n "$WS_NAME" '{name:$n}')")"
WS_ID="$(jq -r '.id' <<<"$WS")"
WS_SLUG="$(jq -r '.slug // empty' <<<"$WS")"

echo "3/6 Moi $GUEST_EMAIL" >&2
api POST "/workspaces/$WS_ID/invitations" "$HOST_TOKEN" "$(jq -nc --arg e "$GUEST_EMAIL" '{email:$e,role:"MEMBER"}')" >/dev/null
INV_TOKEN="$(wait_mail "dev:mail:workspace-invite:$(printf '%s' "$GUEST_EMAIL" | tr 'A-Z' 'a-z')")" \
  || die "khong thay token loi moi trong Redis cho $GUEST_EMAIL"

echo "4/6 $GUEST_EMAIL chap nhan loi moi" >&2
api POST /invitations/accept "$GUEST_TOKEN" "$(jq -nc --arg t "$INV_TOKEN" '{token:$t}')" >/dev/null

echo "5/6 Tao phong dich vi -> en, ja" >&2
ROOM="$(api POST /translation-rooms "$HOST_TOKEN" "$(jq -nc --arg w "$WS_ID" --arg t "$ROOM_TITLE" \
  '{workspaceId:$w,title:$t,sourceLanguage:"vi",targetLanguages:["en","ja"],requiresApproval:false,displayName:"Demo Host"}')")"
ROOM_ID="$(jq -r '.id' <<<"$ROOM")"
ROOM_CODE="$(jq -r '.roomCode' <<<"$ROOM")"

echo "6/6 Guest vao phong bang ma (nghe vi -> noi en)" >&2
api POST /translation-rooms/join "$GUEST_TOKEN" "$(jq -nc --arg c "$ROOM_CODE" \
  '{roomCode:$c,displayName:"Demo Guest",speakLanguage:"en",listenLanguage:"vi"}')" >/dev/null || \
  echo "  (guest join that bai, bo qua)" >&2

PARTS="$(api GET "/translation-rooms/$ROOM_ID/participants" "$HOST_TOKEN" 2>/dev/null || echo '[]')"
SIM_IDS="$(jq -r 'if type=="array" then . else (.items // .participants // []) end | map(.id) | join(",")' <<<"$PARTS" 2>/dev/null || true)"

cat <<EOT

================ DEMO SAN SANG ================
API         : $API_URL
Workspace   : $WS_NAME   (id $WS_ID${WS_SLUG:+, slug $WS_SLUG})
Phong       : $ROOM_TITLE
  roomId    : $ROOM_ID
  ma phong  : $ROOM_CODE        (nguon vi -> dich en, ja)

Tai khoan (mat khau chung): $PASSWORD
  Host  (OWNER)  : $HOST_EMAIL   userId $HOST_ID
  Guest (MEMBER) : $GUEST_EMAIL  userId $GUEST_ID

Dang nhap web: http://localhost:${WEB_PORT:-5173}/ -> dung email/mat khau tren.
Vao phong    : mo phong "$ROOM_TITLE" (Host), hoac nguoi khac nhap ma $ROOM_CODE.
EOT
if [ -n "$SIM_IDS" ]; then
  cat <<EOT

Pipeline mo phong (--fake) can biet ai "noi". Khoi dong lai pipeline voi:
  SIM_PARTICIPANTS=$SIM_IDS
EOT
fi
