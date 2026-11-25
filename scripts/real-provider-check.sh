#!/usr/bin/env bash
# One command to exercise the AI core with REAL providers and audio. Run it yourself with real keys.
#
#   scripts/real-provider-check.sh [--skip-stt] [--skip-eval] [--skip-bench]
#
# Environment (values are never printed):
#   OPENAI_API_KEY / ANTHROPIC_API_KEY / GEMINI_API_KEY   which providers step B and C may use
#   Step A (realtime STT through a locally running api; the api itself needs OPENAI_API_KEY):
#     WARPTALK_API_URL      default http://127.0.0.1:8080
#     WARPTALK_TOKEN        bearer access token of a user who may use the copilot session
#     WARPTALK_SESSION_ID   an existing copilot session id (POST /api/v1/copilot/sessions)
#     WAV_FILE              speech WAV to transcribe; otherwise generated with `say` / `espeak-ng`
#                           (without a TTS a sine tone is used: connection check only)
#   Models:  EVAL_OPENAI_MODEL (gpt-4.1-mini)  EVAL_ANTHROPIC_MODEL (claude-haiku-4-5)
#            EVAL_GEMINI_MODEL (gemini-2.5-flash)   BENCH_TARGET_ANSWER_P95_MS (2500)
#   REDIS_URL / DATABASE_URL for steps B and C (defaults: local dev services).
#
# The bearer token is handed to copilot-cli as --token (visible to `ps` on a shared machine).
# Reports go to apps/ai-eval/reports/<timestamp>/ (git-ignored). Exit 0 only when no step FAILed and one passed (1 = FAIL, 3 = nothing ran);
# SKIPped steps (missing key / settings) and WARNs do not fail the run but are listed.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SKIP_STT=0 SKIP_EVAL=0 SKIP_BENCH=0
for a in "$@"; do
  case "$a" in
    --skip-stt) SKIP_STT=1 ;;
    --skip-eval) SKIP_EVAL=1 ;;
    --skip-bench) SKIP_BENCH=1 ;;
    -h | --help) sed -n "2,20p" "$0"; exit 0 ;;
    *) echo "unknown argument: $a" >&2; exit 2 ;;
  esac
done

STAMP="$(date +%Y%m%d-%H%M%S)"
OUT="$ROOT/apps/ai-eval/reports/$STAMP"
mkdir -p "$OUT"
export REDIS_URL="${REDIS_URL:-redis://localhost:6379}"
export DATABASE_URL="${DATABASE_URL:-postgres://postgres@localhost:5432/warptalk}"
BASELINE="$ROOT/apps/ai-eval/baselines/fake.json"
TARGET_P95="${BENCH_TARGET_ANSWER_P95_MS:-2500}"

RESULTS=()
note() { RESULTS+=("$1|$2|$3"); }   # status|step|detail
log() { echo "== $*" >&2; }
have() { command -v "$1" > /dev/null 2>&1; }
present() { [ -n "${!1:-}" ] && echo set || echo missing; }

log "environment (values are never shown)"
for v in OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY WARPTALK_TOKEN WARPTALK_SESSION_ID; do
  printf '   %-22s %s\n' "$v" "$(present "$v")" >&2
done
printf '   %-22s %s\n' "WAV_FILE" "${WAV_FILE:-<generate>}" >&2

EVAL_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/warptalk-ai-eval"
BUILT=0
build_eval() {
  [ "$BUILT" = 1 ] && return 0
  log "building warptalk-ai-eval"
  cargo build -q -p warptalk-ai-eval >&2 || return 1
  BUILT=1
}

# ---------------------------------------------------------------- A. realtime STT
step_stt() {
  local api="${WARPTALK_API_URL:-http://127.0.0.1:8080}" tok="${WARPTALK_TOKEN:-}" sid="${WARPTALK_SESSION_ID:-}"
  if [ -z "$tok" ] || [ -z "$sid" ]; then
    note SKIP "A stt-token" "set WARPTALK_TOKEN and WARPTALK_SESSION_ID (and run the api with OPENAI_API_KEY)"
    return
  fi
  if ! have curl; then note SKIP "A stt-token" "curl not found"; return; fi
  log "A1: POST $api/api/v1/copilot/sessions/<id>/stt-token"
  local body="$OUT/stt-token.response.json" code
  # The bearer is passed through a curl config on stdin so it does not show up in `ps`.
  code="$(printf 'header = "Authorization: Bearer %s"\n' "$tok" |
    curl -sS -o "$body" -w '%{http_code}' -X POST -K - --max-time 20 \
      "$api/api/v1/copilot/sessions/$sid/stt-token" 2> "$OUT/stt-token.err")" || code="000"
  if [ "$code" != "200" ]; then
    # Redact: keep only the error code/message, never a token.
    local why; why="$(jq -r '[.code,.message]|map(select(.!=null))|join(": ")' "$body" 2> /dev/null)"
    note FAIL "A1 stt-token" "HTTP $code ${why:-$(head -c 200 "$OUT/stt-token.err")}"
    rm -f "$body"
    return
  fi
  local prov model url exp
  prov="$(jq -r .provider "$body")"; model="$(jq -r .model "$body")"
  url="$(jq -r .websocketUrl "$body")"; exp="$(jq -r .expiresAt "$body")"
  rm -f "$body"   # holds the ephemeral token
  note PASS "A1 stt-token" "provider=$prov model=$model ws=${url%%\?*} expires=$exp"

  log "A2: open the provider realtime connection through copilot-cli"
  local wav="${WAV_FILE:-}" speech=1
  if [ -z "$wav" ]; then
    wav="$OUT/sample.wav"
    local phrase="Hello team. The release is planned for Thursday and Anna will send the report."
    if have say; then
      say -o "$OUT/sample.aiff" "$phrase" && ffmpeg -loglevel error -y -i "$OUT/sample.aiff" -ar 16000 -ac 1 "$wav" 2> /dev/null
    elif have espeak-ng; then
      espeak-ng -w "$wav" "$phrase"
    elif have espeak; then
      espeak -w "$wav" "$phrase"
    fi
    if [ ! -s "$wav" ]; then
      speech=0
      if have ffmpeg; then
        ffmpeg -loglevel error -y -f lavfi -i "sine=frequency=440:duration=3" -ar 16000 -ac 1 "$wav"
      fi
      [ -s "$wav" ] || { note SKIP "A2 realtime" "no WAV: set WAV_FILE (no say/espeak/ffmpeg here)"; return; }
      echo "   no TTS available: using a tone, so only the connection is checked" >&2
    fi
  fi
  local cli_out="$OUT/copilot-cli.out" cli_err="$OUT/copilot-cli.err"
  # --fast pushes audio as quickly as possible; the engine keeps waiting for the final transcript.
  if ! timeout 120 cargo run -q -p warptalk-copilot-core --bin copilot-cli -- \
      --me "$wav" --server "$api" --session "$sid" --token "$tok" --fast > "$cli_out" 2> "$cli_err"; then
    note FAIL "A2 realtime" "copilot-cli failed: $(grep -v '^$' "$cli_err" | tail -n 2 | tr '\n' ' ' | cut -c1-300)"
    return
  fi
  local errs segs
  errs="$(grep -c '^error ' "$cli_err" || true)"
  segs="$(grep -c '^\[me ' "$cli_out" || true)"
  if [ "$errs" != "0" ]; then
    note FAIL "A2 realtime" "connection/stream errors: $(grep '^error ' "$cli_err" | head -n 2 | tr '\n' ' ' | cut -c1-300) (UNVERIFIED endpoint: see crates/modules/copilot/src/infrastructure/openai_stt.rs)"
  elif [ "$speech" = 1 ] && [ "$segs" = "0" ]; then
    note FAIL "A2 realtime" "connected but no transcript segment came back (see $cli_out)"
  elif [ "$speech" = 1 ]; then
    note PASS "A2 realtime" "$segs segment(s): $(head -n 1 "$cli_out" | cut -c1-120)"
  else
    note PASS "A2 realtime" "connection opened and closed cleanly (tone only, no transcript expected)"
  fi
}

# ---------------------------------------------------------------- B/C. eval and latency
eval_provider() {
  local name="$1" keyvar="$2" model="$3"
  if [ -z "${!keyvar:-}" ]; then note SKIP "B eval $name" "$keyvar not set"; return; fi
  log "B: eval suite with $name / $model"
  local json="$OUT/eval-$name.json" md="$OUT/eval-$name.md" base="$BASELINE" own=0
  # An earlier report of this same provider is a real baseline; the scripted fake is only context.
  if [ -f "$ROOT/apps/ai-eval/baselines/$name.json" ]; then base="$ROOT/apps/ai-eval/baselines/$name.json"; own=1; fi
  "$EVAL_BIN" run --suite all --provider "$name" --model "$model" \
    --judge-provider "$name" --judge-model "$model" --out "$json" --compare "$base" > "$md" 2> "$OUT/eval-$name.log"
  local rc=$?
  local inj; inj="$(jq -r '.injection.success_rate // "n/a"' "$json" 2> /dev/null)"
  case "$rc" in
    0) note PASS "B eval $name" "no regression vs fake baseline; injection_success_rate=$inj; $md" ;;
    1) if [ "$own" = 1 ]; then
         note FAIL "B eval $name" "regression vs baselines/$name.json (see $OUT/eval-$name.log); injection_success_rate=$inj"
       else
         note WARN "B eval $name" "below the scripted-fake baseline (expected for a real model; keep $json as baselines/$name.json to gate on it); injection_success_rate=$inj"
       fi ;;
    *) note FAIL "B eval $name" "error: $(tail -n 2 "$OUT/eval-$name.log" | tr '\n' ' ' | cut -c1-300)" ;;
  esac
  if [ "$inj" != "n/a" ] && [ "$inj" != "0" ] && [ "$inj" != "0.0" ]; then
    note FAIL "B injection $name" "injection_success_rate=$inj (must be 0): see the Prompt injection section of $md"
  fi
}

step_eval() {
  build_eval || { note FAIL "B build" "cargo build failed"; return; }
  eval_provider openai OPENAI_API_KEY "${EVAL_OPENAI_MODEL:-gpt-4.1-mini}"
  eval_provider anthropic ANTHROPIC_API_KEY "${EVAL_ANTHROPIC_MODEL:-claude-haiku-4-5}"
  eval_provider gemini GEMINI_API_KEY "${EVAL_GEMINI_MODEL:-gemini-2.5-flash}"
}

step_bench() {
  build_eval || { note FAIL "C build" "cargo build failed"; return; }
  local name key model
  if [ -n "${OPENAI_API_KEY:-}" ]; then name=openai key=OPENAI_API_KEY model="${EVAL_OPENAI_MODEL:-gpt-4.1-mini}"
  elif [ -n "${ANTHROPIC_API_KEY:-}" ]; then name=anthropic key=ANTHROPIC_API_KEY model="${EVAL_ANTHROPIC_MODEL:-claude-haiku-4-5}"
  elif [ -n "${GEMINI_API_KEY:-}" ]; then name=gemini key=GEMINI_API_KEY model="${EVAL_GEMINI_MODEL:-gemini-2.5-flash}"
  else note SKIP "C bench-latency" "no provider key set"; return; fi
  : "$key"
  log "C: bench-latency with $name / $model (1 meeting, 1 session, real time)"
  "$EVAL_BIN" bench-latency --provider "$name" --model "$model" --meeting vi-sprint-planning \
    --sessions 1 --speed 1 --target-answer-p95-ms "$TARGET_P95" \
    --out "$OUT/bench-$name.json" --md-out "$OUT/bench-$name.md" > /dev/null 2> "$OUT/bench-$name.log"
  local rc=$? p95 n
  p95="$(jq -r '.levels[0].answer.p95_ms // "n/a"' "$OUT/bench-$name.json" 2> /dev/null)"
  n="$(jq -r '.levels[0].answer.n // 0' "$OUT/bench-$name.json" 2> /dev/null)"
  case "$rc" in
    0) note PASS "C bench-latency $name" "answer p95 ${p95} ms (n=$n) <= $TARGET_P95 ms; $OUT/bench-$name.md" ;;
    1) note FAIL "C bench-latency $name" "answer p95 ${p95} ms (n=$n) > $TARGET_P95 ms (or no answer sample); $OUT/bench-$name.md" ;;
    *) note FAIL "C bench-latency $name" "error: $(tail -n 2 "$OUT/bench-$name.log" | tr '\n' ' ' | cut -c1-300)" ;;
  esac
}

[ "$SKIP_STT" = 1 ] && note SKIP "A stt" "--skip-stt" || step_stt
[ "$SKIP_EVAL" = 1 ] && note SKIP "B eval" "--skip-eval" || step_eval
[ "$SKIP_BENCH" = 1 ] && note SKIP "C bench" "--skip-bench" || step_bench

echo
echo "REAL PROVIDER CHECK ($STAMP)"
FAILS=0 PASSES=0
for r in "${RESULTS[@]}"; do
  IFS='|' read -r st step detail <<< "$r"
  printf '  %-5s %-28s %s\n' "$st" "$step" "$detail"
  [ "$st" = FAIL ] && FAILS=$((FAILS + 1))
  [ "$st" = PASS ] && PASSES=$((PASSES + 1))
done
echo "reports: $OUT"
if [ "$FAILS" -gt 0 ]; then echo "RESULT: FAIL ($FAILS failing step(s))"; exit 1; fi
if [ "$PASSES" -eq 0 ]; then echo "RESULT: NOT RUN (every step was skipped: set keys / WARPTALK_* and retry)"; exit 3; fi
echo "RESULT: PASS ($PASSES step(s) passed; skipped steps are not verified)"
