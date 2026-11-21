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

- [~] W6a Research BYO LLM (OpenAI/Claude/Gemini) → `docs/architecture/byo-llm.md`
- [~] W6b pipeline `audio` + `ingress` (endpointing, knob cấu hình runtime)
- [~] W6c `providers/livekit` + `meeting` slice 1 (join token, bridge, end, webhook)
- [ ] W7a `providers/llm` (trait + adapters openai/anthropic/gemini theo byo-llm.md) + workspace AI config (key mã hoá, hot reload)
- [ ] W7b pipeline `runtime` (room actor, per-speaker task, bounded channel, latency stamps) + `stt` (OpenAI Realtime WS, warm pool, flash mode, early sentences)
- [ ] W7c wire `meeting` vào api; platform settings store (runtime config, Redis pub/sub invalidation)
- [ ] W8a pipeline `mt` (realtime first sentence + batch, glossary, speculative, ordering) + `tts` (Cartesia warm pool, cache, isochrony, catch-up, clone ladder, degradation)
- [ ] W8b `apps/pipeline` binary: LiveKit RTC ingress/egress, room lease, caption stream
- [ ] W8c `apps/worker` + `transcript` consumer (stt/translate/tts results, pause gate, backfill)
- [ ] W9a suggestion engine (spec 09, mọi ngưỡng/category/prompt cấu hình runtime theo workspace/room) + follow-up/action items
- [ ] W9b knowledge/RAG (spec 14 + proposals): structure-aware chunking + citation page/timestamp, score filter + re-rank, purge vectors khi archived/AI-disallowed, embedding retry/DLQ, fact extraction, PII scan transcript, OCR hook, search-quality metrics
- [ ] W10 assistant (WarpBot) chat + tools + confirmation gate; billing metering; notification; invitations/series/artifacts
- [ ] Voice clone (spec 13): consent gate runtime, progressive ladder, revocation/deletion

## Nợ kỹ thuật đã biết

- `IdentityUserDirectory` gọi từng user một (cần batch query)
- `workspace` dùng role uuid cố định; BR-31/32 (một membership Internal) chưa enforce
- `room`: chưa có capacity limit, credit check khi resume, voice-clone toggle endpoint, các trạng thái route degrade
- Outbox chưa có lease khi claim (consumer phải idempotent)
- Voice consent đang là Noop
