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

## Backlog (theo thứ tự)

1. Wire `room` vào `api` (đang làm)
2. `meeting`: LiveKit token, webhook (cần `providers/livekit`)
3. **Pipeline vertical slice** (chờ tài liệu luồng dịch + latency): ingress VAD → STT → MT, caption only
4. `transcript` consumer trong `apps/worker`
5. TTS + egress, degradation ladder
6. `billing` metering idempotent
7. invitations, series, artifacts, notification, assistant

## Nợ kỹ thuật đã biết

- `IdentityUserDirectory` gọi từng user một (cần batch query)
- `workspace` dùng role uuid cố định; BR-31/32 (một membership Internal) chưa enforce
- `room`: chưa có capacity limit, credit check khi resume, voice-clone toggle endpoint, các trạng thái route degrade
- Outbox chưa có lease khi claim (consumer phải idempotent)
- Voice consent đang là Noop
