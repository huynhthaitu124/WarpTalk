# Tiến độ (cập nhật bởi orchestrator)

Quy trình: orchestrator giao từng crate cho agent model nhỏ (haiku: việc cơ học, sonnet: logic khó) với brief hẹp + `CLAUDE.md`; agent tự chạy fmt/clippy/test; orchestrator chạy lại clippy + test toàn workspace rồi commit.

## Đã xong

| Crate | Nội dung | Test |
|---|---|---|
| shared/kernel | typed IDs, AppError (+Coded), paging camelCase, clock, roles | unit |
| shared/config | env config có redact secret | unit |
| shared/telemetry | tracing, Prometheus, `warptalk_stage_latency_ms` | unit |
| shared/db | pool, migration runner theo module (advisory lock, checksum), outbox | Postgres |
| shared/bus | Streams consumer group, XAUTOCLAIM, DLQ, idempotency, lease, pub/sub | Redis |
| shared/contracts | tên stream/channel/key + payload tương thích bản gốc | unit |
| shared/web | JWT, `AuthUser`/`SystemAdmin`, layers | unit + router |
| modules/identity | register, verify email, login lockout, refresh rotation + reuse detection, profile, settings | Postgres |
| modules/workspace | CRUD, settings, members, role, leave, transfer ownership | Postgres |
| modules/room | state machine, join/lobby, sessions, audio routes → Redis snapshot | Postgres + Redis |
| apps/migrator | chạy migration mọi module | chạy thật |
| apps/api | server chạy được, e2e qua HTTP | e2e |

## Backlog (loop, theo thứ tự ưu tiên)

Nguồn: spec 08–14 + repo gốc (clone tại scratchpad `upstream/`). Đánh dấu `[~]` đang làm, `[x]` xong.

- [x] W6a Research BYO LLM (OpenAI/Claude/Gemini) → `docs/architecture/byo-llm.md`
- [x] W6b pipeline `audio` + `ingress` (endpointing, knob cấu hình runtime)
- [x] W6c `providers/livekit` + `meeting` slice 1 (join token, bridge, end, webhook)
- [x] W7a `providers/llm` (trait + adapters openai/anthropic/gemini theo byo-llm.md) + workspace AI config (key mã hoá, hot reload)
- [x] W7b pipeline `runtime` (room actor, per-speaker task, bounded channel, latency stamps) + `stt` (OpenAI Realtime WS, warm pool, flash mode, early sentences)
- [x] W7c wire `meeting` vào api; platform settings store (runtime config, Redis pub/sub invalidation)
- [x] W8a pipeline `mt` (realtime first sentence + batch, glossary, speculative, ordering) + `tts` (Cartesia warm pool, cache, isochrony, catch-up, clone ladder, degradation)
- [x] W8b `apps/pipeline` binary: LiveKit RTC ingress/egress, room lease, caption stream
- [x] W8c `apps/worker` + `transcript` consumer (stt/translate/tts results, pause gate, backfill)
- [x] W9a suggestion engine (spec 09, mọi ngưỡng/category/prompt cấu hình runtime theo workspace/room) + follow-up/action items
- [x] W9b knowledge/RAG (spec 14 + proposals): structure-aware chunking + citation page/timestamp, score filter + re-rank, purge vectors khi archived/AI-disallowed, embedding retry/DLQ, fact extraction, PII scan transcript, OCR hook, search-quality metrics
- [ ] W10 assistant (WarpBot) chat + tools + confirmation gate; billing metering; notification; invitations/series/artifacts
- [ ] Voice clone (spec 13)
- [x] W11a External meeting analysis xong; [x] W11b bridge room API + bridge_token fix; [x] far-side dedupe;
- [x] W11 External meeting (Meet/Teams/Zoom) phân tích → `docs/architecture/external-meeting.md`, rồi sửa lỗi + phát triển
- [x] W12 Test: functional e2e, failure injection, performance/latency (pipeline bench với fake provider, load test N room, đo mouth-to-ear)
- [x] W14 `apps/web` (React/Vite/Tailwind 4) theo design system Linear + WarpTalk legacy, Playwright E2E trên backend thật (test plan + visual regression + a11y)
- [x] W13a App macOS SwiftUI (chưa compile; xem apps/macos/README.md, VERIFY list) → sau W14 chuyển sang WebView shell + native bridge
- [ ] W13 App macOS chạy được meeting (deadline: thứ 2 2026-11-23): consent gate runtime, progressive ladder, revocation/deletion


## Cập nhật 2026-11-22

- Backend: 591+ test pass toàn workspace; thêm invitations, GET /me, dev mail capture, BYO LLM (ai-plane), bridge viewer/lease/release, chat meeting + dịch chat, đổi ngôn ngữ giữa meeting, glossary (workspace + global) feed vào pipeline, lỗi request JSON thống nhất.
- Pipeline: chuỗi STT→MT→TTS→dub; bench release 2026-11-22: speech_end→first dub p50 1021 ms / p95 1645 ms (1 câu), baseline 2275/3114 (docs/perf/run-2026-11-22.md).
- Web: audit 56 lỗi → đã sửa gần hết DS/IA/LY/FN/A11Y + meeting UX; Playwright 208/208; ảnh review docs/review/.
- Demo: docs/RUNBOOK.md, deploy/docker/docker-compose.yml, scripts/dev-up.sh, scripts/demo-seed.sh; livekit optional dep (`--features livekit-rtc`).
- [~] Web: chat API, đổi ngôn ngữ, Glossary UI, members role/remove, dọn psql/lucide (đang làm)
- [ ] macOS app: build thật trên Mac + sửa VERIFY; pipeline `livekit-rtc` build thật (cần clang ≥ 21)
- [ ] Còn thiếu backend: documents upload, voice profile/consent, billing, notification, WarpBot chat, forgot-password, calendar

## Quyết định 2026-11-22 (user)

- `apps/web` chuyển từ Vite sang **Next.js (App Router, cùng major với legacy: Next 16)**. Thứ tự: hoàn tất restyle theo `apps/web/LEGACY_AUDIT.md` (đang chạy) → migrate sang Next.js giữ nguyên components/tests → Playwright + so sánh legacy.

## Bị chặn bởi môi trường (cần user quyết)

- Không build được LiveKit Rust RTC (`webrtc-sys` cần clang ≥ 21; máy có clang 18). Tải LLVM 21 bị chặn bởi permission.
- Không tải được `livekit-server` binary để test meeting thật bằng Playwright (permission). Meeting E2E dùng trạng thái "LiveKit not configured"/mock.
- `openai.com`, `ai.google.dev`, `developers.google.com`, `developers.zoom.us`, `learn.microsoft.com` bị chặn → các claim liên quan đánh dấu UNVERIFIED.

## Nợ kỹ thuật đã biết

- `apps/api` dùng `#[path]` để include `apps/worker/src/{ai,knowledge}.rs` → tách thành crate `crates/shared/ai-plane`
- Knowledge: member chưa có allowlist meeting/document (fail closed); workspace documents upload chưa làm
- Pipeline: chưa phát `__MEETING_END__`; isochrony cần `source_ms` trên SttSegment

- `IdentityUserDirectory` gọi từng user một (cần batch query)
- `workspace` dùng role uuid cố định; BR-31/32 (một membership Internal) chưa enforce
- `room`: chưa có capacity limit, credit check khi resume, voice-clone toggle endpoint, các trạng thái route degrade
- Outbox chưa có lease khi claim (consumer phải idempotent)
- Voice consent đang là Noop
