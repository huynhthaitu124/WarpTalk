# WarpTalk (Rust) – Kiến trúc backend

Trạng thái: **bản nháp v0.1** (2026-11-20). Phần realtime pipeline (mục 5) sẽ được chốt lại sau khi có thêm tài liệu về luồng dịch và tối ưu latency.

Phạm vi: chỉ backend (business services + AI pipeline realtime). Web (Next.js) và Desktop (Electron) giữ nguyên, nằm ngoài repo này.

Nguồn: `docs/spec/`. Khi các file spec mâu thuẫn nhau, dùng thứ tự ưu tiên sau:

1. Spec suy ra từ code: `08`–`12` (đọc từ code thật của warptalk-ai / warptalk-backend)
2. `05a-data-dictionary.md` (sinh từ ERD DBML)
3. Spec suy ra từ Report 7: `00`–`07` (tài liệu thiết kế, có chỗ đã lỗi thời so với code)

---

## 1. Review spec – những điểm ảnh hưởng tới bản Rust

### 1.1 Mâu thuẫn giữa Report 7 và code (code thắng)

| # | Report 7 nói | Code thực tế | Quyết định cho bản Rust |
|---|---|---|---|
| R1 | Domain events đi qua MassTransit/RabbitMQ (`04` §4) | Translation Room, Meeting, Transcript **không dùng RabbitMQ**, chỉ dùng Redis (`10` §1, `11` §6.3, `12` §6.3) | Bỏ RabbitMQ. Redis Streams là bus duy nhất (ADR-3) |
| R2 | Transcript nhận segment qua gRPC `SaveSegment` (`06` §3.5, §5.2) | Không có RPC `SaveSegment`, segment được ghi bằng cách consume Redis stream (`12` §1) | Ghi transcript bằng stream consumer |
| R3 | Client gửi audio qua SignalR `SendAudioChunk` (`06` §3.5) | Audio đi qua LiveKit, do `livekit_ingress_worker` lấy (`08` §1) | Audio chỉ đi qua LiveKit. Không nhận audio qua WebSocket |
| R4 | Meeting service có cung cấp gRPC | Meeting không có gRPC server (`11` §2) | Không cần gRPC nội bộ (ADR-1) |
| R5 | Audio route có 3 trạng thái READY/BROADCASTING/COMPLETED (`06` §3.10) | Code có 12 trạng thái, cộng thêm thứ tự ưu tiên degrade (`10` §3.7) | Theo `10` §3.7 |
| R6 | Meeting kết thúc với `FINISHED`, room kết thúc với `ENDED` (OI-08) | Chưa rõ cách map giữa hai cái | Một state machine room duy nhất trong module `room`. Meeting chỉ giữ trạng thái runtime của LiveKit |
| R7 | Export ra PDF/DOCX/VTT/SRT | Code chỉ xuất `csv`/`txt` (`12` §5.6) | Giai đoạn 1 làm giống code. PDF/DOCX/SRT ghi vào backlog |

### 1.2 Những chỗ thiết kế cũ nên sửa khi viết lại

| # | Vấn đề trong bản gốc | Ảnh hưởng | Hướng xử lý trong bản Rust |
|---|---|---|---|
| D1 | Mỗi chặng STT → MT → TTS là một process riêng, nối nhau bằng Redis Streams (`08` §1) | Mỗi bước tốn thêm một lần serialize, một round-trip Redis và một vòng `XREADGROUP` | Chạy pipeline trong cùng một process, các chặng nối bằng tokio channel. Redis chỉ nhận output cần lưu bền (ADR-2) |
| D2 | `tts:results` chứa WAV khoảng 113 KB mỗi entry (`08` §6) | Tốn bộ nhớ Redis, phải giới hạn `stream_maxlen` 200 | TTS stream thẳng vào LiveKit track. Stream chỉ chứa metadata (segment, lang, duration, voice) |
| D3 | Mỗi lần publish ghi vào cả `<stream>:{meetingId}` lẫn `<stream>` global (`08` §1) | Số lần ghi tăng gấp đôi | Chỉ giữ stream global, filter theo `meeting_id`. Thêm stream theo room khi thật sự có consumer cần |
| D4 | Hàng đợi finalize artifacts nằm trong `Channel` ở RAM, vì vậy phải viết thêm worker reconcile và recovery (`10` §6) | Restart process là mất việc. Logic phức tạp | Dùng transactional outbox trong Postgres (`warptalk-db`), mỗi job có trạng thái và số lần retry |
| D5 | Audio route được sinh cho mọi cặp (speaker, listener), tức N² route (`10` §3.7) | Nếu TTS cũng chạy theo từng listener thì chi phí tăng theo N² | Chỉ synth một lần cho mỗi bộ (speaker, target_lang, voice_mode), rồi fan-out cho listener. Route vẫn giữ để điều khiển quyền và UI. **Cần xác nhận bản gốc xử lý thế nào** |
| D6 | Không rõ service nào ghi cờ khoá billing `ai_service_suspended`. Khi Redis lỗi thì mặc định cho qua (fail-open) (`10` §5.4) | Lỗ hổng chính sách | Module `billing` sở hữu và ghi cờ này. Fail-open phải là quyết định có ghi rõ, không để mặc định |
| D7 | Không có SLO end-to-end nào được enforce (`08` §1) | Không đo được mức cải thiện | Định nghĩa SLO theo NFR-PF-04/05/06 và đo bằng histogram từng chặng ngay từ đầu |

### 1.3 Thiếu thông tin (cần bổ sung trước khi làm module tương ứng)

- **Auth, Workspace, Billing, Notification, Assistant** mới chỉ có spec từ Report 7 (`06`), chưa có spec đọc từ code như `10`–`12`. Viết lại từ tài liệu này sẽ dễ lệch hành vi thật.
- **Pipeline**: chưa rõ pipeline lấy danh sách ngôn ngữ đích và glossary từ đâu, cũng như TTS publish theo track của từng ngôn ngữ hay của từng listener. Đợt tài liệu sắp tới nên trả lời các câu này.
- **Client protocol**: Web/Desktop hiện nói chuyện qua SignalR. Đây là quyết định lớn nhất còn mở (Q1, mục 8).
- `05a` dài 1447 dòng và đủ để viết migration, nhưng các open issue OI-01 (hash mật khẩu), OI-03 (đơn vị billing là phút hay credit) và RV-01 (`usage_records.segment_id`) cần chốt trước khi viết migration cho `identity` và `billing`.

---

## 2. Tổng quan kiến trúc

```
                 Web / Desktop (giữ nguyên)
          REST + WebSocket |              | WebRTC (audio/video)
                           v              v
  +------------------------------+    +-------------------+
  |  warptalk-api                |    |  LiveKit (SFU)    |
  |  (modular monolith, axum)    |    +-------------------+
  |  identity  workspace  room   |      ^ subscribe     | publish
  |  meeting   transcript billing|      | speaker audio | dubbed audio
  |  assistant notification      |      |               v
  |  + WS hub + webhooks + cron  |    +-------------------------------+
  +------------------------------+    |  warptalk-pipeline (N replica) |
      |  ^                            |  room actor / mỗi room:        |
      |  | commands, routes,          |   ingress(VAD) -> STT -> MT    |
      |  | glossary (Redis key+pubsub)|   -> TTS -> LiveKit track      |
      v  |                            +-------------------------------+
  +-----------------------------------------------+   | stt/translate/tts results
  |  Redis: Streams (durable) + Pub/Sub + keys    |<--+ (metadata, không chứa audio)
  +-----------------------------------------------+
      |  consumer groups
      v
  +------------------------------+     +-------------+  +--------+  +-------+
  |  warptalk-worker             |---->| PostgreSQL  |  | Qdrant |  | S3/R2 |
  |  transcript persist, billing |     | 1 DB, mỗi   |  +--------+  +-------+
  |  metering, embedding, summary|     | module 1    |
  |  suggestion, chat MT, notify |     | schema      |
  +------------------------------+     +-------------+
```

Ba "plane" tách theo **yêu cầu latency và cách scale**, không tách theo nghiệp vụ:

| Binary | Nhiệm vụ | Scale theo | Latency |
|---|---|---|---|
| `warptalk-api` | REST, WebSocket hub, webhook (LiveKit, Stripe), job định kỳ | Số request | P95 < 500 ms (NFR-RL-06) |
| `warptalk-pipeline` | Speech → translation → dub realtime | Số room và speaker đang hoạt động | Speech-to-speech < 2–2.5 s (NFR-PF-05) |
| `warptalk-worker` | Consumer bất đồng bộ: lưu transcript, trừ credit, embedding, summary, suggestion, dịch chat, gửi email/push | Consumer lag | Vài giây; post-meeting < 30–45 s (NFR-PF-07) |
| `warptalk-migrator` | Chạy migration rồi thoát | – | – |

---

## 3. Các quyết định kiến trúc (ADR rút gọn)

### ADR-1: Modular monolith cho business plane

**Quyết định.** Gộp 10 service .NET (gateway, auth, workspace, translation-room, meeting, transcript, assistant, billing, notification, payment) thành **một binary `warptalk-api`**, chia thành 8 module crate. Mỗi module sở hữu một schema Postgres riêng.

**Lý do.**
- Team nhỏ. Bản gốc tốn nhiều công sức cho gRPC, secret nội bộ, YARP và cache trạng thái giữa các service.
- Các lời gọi chéo như "kiểm tra host", "policy workspace" hay "consent voice clone" thành lời gọi hàm, không còn đi qua mạng.
- Giữ được khả năng tách service về sau. Module không phụ thuộc trực tiếp vào nhau (xem §4.3). Khi tách, chỉ cần thay adapter bằng client gRPC/HTTP.

**Đánh đổi.** Deploy chung một lần và chung failure domain. Chấp nhận được, vì business plane không phải đường nóng về latency.

### ADR-2: Pipeline realtime chạy trong một process, mỗi room là một actor

**Quyết định.** Mỗi room đang dịch do **một replica `warptalk-pipeline`** sở hữu (lease trên Redis `pipeline:room:{id}:owner`). Trong replica đó:

```
RoomActor
 ├─ SpeakerTask (mỗi speaker)   LiveKit audio -> VAD/endpoint -> STT (warm WS)
 │                              -> mpsc -> MT (mỗi target lang) -> mpsc -> TTS
 └─ Egress                      TTS chunk -> LiveKit track (theo lang/voice)
                                caption/translation -> Redis Stream (lưu bền)
```

**Lý do.**
- Bỏ được 3 hop Redis và việc serialize audio giữa các chặng (D1, D2).
- Thứ tự trong cùng một speaker đến tự nhiên từ channel, không cần cơ chế `_PublishTurn` chờ tối đa 10 s như bản gốc.
- Pool WebSocket warm (OpenAI, Cartesia) nằm ngay cạnh nơi dùng.

**Đánh đổi.**
- Scale theo room chứ không theo từng chặng. Muốn scale riêng TTS thì không làm được. Tuy vậy, giới hạn thực sự là quota của vendor (Cartesia concurrency 3), không phải CPU.
- Khi replica chết, room phải được replica khác nhận lại qua lease. Câu nói đang dở bị mất, transcript đã lưu thì không.
- **Sẽ đánh giá lại** khi có tài liệu tối ưu latency. Các trait của từng chặng (`warptalk-pipeline-runtime`) được thiết kế để có thể đặt một transport Redis vào giữa nếu cần.

### ADR-3: Redis là message bus duy nhất, Postgres outbox cho event cần đảm bảo

- **Streams** + consumer group cho kết quả pipeline và domain event: XAUTOCLAIM, dead letter sau 5 lần, giống `08` §6 và `12` §6.1.
- **Pub/Sub** cho lệnh tạm thời: room commands, `AUDIO_ROUTES_UPDATED`, presence.
- **Keys** cho cache, rate limit, lease và idempotency key của billing (NFR-RL-04).
- Event sinh ra cùng một transaction DB được ghi vào bảng outbox, sau đó một relay đẩy lên Stream. Không mất event khi crash.
- Tên stream, channel và key giữ **giống bản gốc** (`translate:results`, `translationRoom:{id}:audio_routes`, ...) và định nghĩa tập trung trong `warptalk-contracts`. Nhờ vậy giai đoạn chuyển đổi có thể chạy song song với worker Python cũ.

### ADR-4: PostgreSQL + sqlx, giữ nguyên schema của `05a`

- Một database, mỗi module một schema: `auth`, `voice`, `workspace`, `translation_room`, `meeting`, `transcript`, `subscription`, `assistant`, `notification`. Không join chéo schema, giống quy tắc gốc.
- Migration forward-only nằm trong từng module (`crates/modules/<m>/migrations/`). `warptalk-migrator` gom và chạy theo thứ tự.
- Dùng `sqlx` với query kiểm tra lúc compile, không dùng ORM. Với dev quen EF thì tương đương viết SQL tay, nhưng sai tên cột là compiler báo lỗi.

### ADR-5: WebSocket hub trong `warptalk-api`

Thay SignalR Gateway bằng một WebSocket endpoint (axum). Hub dùng Redis Pub/Sub làm backplane để nhiều replica `api` cùng phát được. Giao thức phụ thuộc Q1 (mục 8).

### ADR-6: Clean Architecture trong mỗi module (giống bản .NET)

| .NET (bản gốc) | Rust (bản này) |
|---|---|
| `X.Domain` project | `src/domain/` – entity, value object, state machine, lỗi domain |
| `X.Application` (services, interfaces) | `src/application/` – use case và **port** (trait) |
| `X.Infrastructure` (EF, Redis, clients) | `src/infrastructure/` – adapter cài đặt port |
| `X.API` (controllers) | `src/http/` – axum handler + DTO (serde) |
| `Program.cs` / DI container | `apps/api/src/main.rs` – composition root, ráp mọi thứ bằng tay |
| `IRepository<T>` interface | `trait RoomRepository` trong `application` |

---

## 4. Cấu trúc thư mục

```
WarpTalk/
├── Cargo.toml                 # workspace: members, version dependency dùng chung, lints
├── rust-toolchain.toml
├── apps/                      # binary: chỉ chứa wiring, không chứa logic nghiệp vụ
│   ├── api/                   # REST + WS hub + webhooks + cron jobs
│   ├── pipeline/              # realtime speech -> translation -> dub
│   ├── worker/                # Redis Streams consumers
│   └── migrator/              # chạy migration
├── crates/
│   ├── shared/                # dùng chung, không biết gì về nghiệp vụ cụ thể
│   │   ├── kernel/            # ID có kiểu (RoomId, WorkspaceId…), AppError, clock, paging
│   │   ├── config/            # đọc env thành struct có kiểu
│   │   ├── telemetry/         # tracing, OTel, Prometheus, histogram latency theo stage
│   │   ├── contracts/         # tên stream/key/channel, envelope và payload event
│   │   ├── bus/               # Redis Streams consumer group, DLQ, lease, idempotency
│   │   ├── db/                # pool Postgres, transaction, outbox
│   │   └── storage/           # S3 / R2
│   ├── modules/               # bounded context nghiệp vụ (api + worker dùng)
│   │   ├── identity/          # auth + voice
│   │   ├── workspace/
│   │   ├── room/              # translation_room
│   │   ├── meeting/
│   │   ├── transcript/
│   │   ├── assistant/
│   │   ├── billing/
│   │   └── notification/
│   ├── pipeline/              # realtime (chỉ pipeline dùng)
│   │   ├── runtime/           # room actor, speaker task, channel, ordering, degrade, lease
│   │   ├── audio/             # PCM, resample, ring buffer, RMS
│   │   ├── ingress/           # VAD, hangover ladder, chunk cap, gate
│   │   ├── stt/
│   │   ├── mt/
│   │   └── tts/
│   └── providers/             # adapter cho dịch vụ bên ngoài, không biết về nghiệp vụ
│       ├── livekit/  openai/  cartesia/  stripe/
│       └── email/    google/  qdrant/
├── deploy/docker/             # Dockerfile, compose
├── docs/
│   ├── spec/                  # spec suy ra từ bản .NET (đã có)
│   └── architecture/          # tài liệu này + ADR
└── tests/                     # test end-to-end chạy trên nhiều binary
```

### 4.1 Bên trong một module

```
crates/modules/room/
├── Cargo.toml
├── migrations/                # 0001_init.sql, 0002_... (schema translation_room)
└── src/
    ├── lib.rs                 # pub fn routes(), pub fn consumers(), struct RoomModule
    ├── domain/                # Room, Participant, AudioRoute, RoomStatus (state machine)
    ├── application/           # create_room, join_room, start_room… + ports (traits)
    ├── infrastructure/        # PgRoomRepository, RedisRouteCache, ...
    └── http/                  # handlers + DTO
```

### 4.2 Quy tắc phụ thuộc giữa crate (bắt buộc)

```
apps/*  ──>  modules/*, pipeline/*, providers/*, shared/*
modules/* ──> shared/*, providers/*        (KHÔNG module -> module)
pipeline/* ──> shared/*, providers/*, pipeline/audio
providers/* ──> shared/kernel             (không biết về module)
shared/* ──> shared/kernel                (không biết về ai khác)
```

- `pipeline` **không** phụ thuộc `modules` và **không** kết nối Postgres. Cấu hình room (routes, ngôn ngữ, glossary, consent) đến qua Redis key + pub/sub do `api` publish, đúng như bản gốc (`10` §3.7). Nhờ vậy pipeline nhẹ và khởi động nhanh.
- `domain/` không import tokio, sqlx hay redis. Test domain chỉ là `cargo test`, không cần DB.

### 4.3 Module cần dữ liệu của module khác thì làm sao?

Bên cần định nghĩa port. Composition root ráp adapter vào. Ví dụ: `transcript` cần kiểm tra host (bản gốc gọi gRPC sang Translation Room):

```rust
// crates/modules/transcript/src/application/ports.rs
pub trait RoomDirectory: Send + Sync {
    async fn effective_host(&self, room: RoomId) -> Result<UserId, AppError>;
}

// apps/api/src/wiring.rs  (ở đây được phép biết cả hai module)
struct RoomDirectoryAdapter(Arc<room::application::RoomQueries>);
impl transcript::application::RoomDirectory for RoomDirectoryAdapter { ... }
```

Khi tách service sau này, chỉ cần thay `RoomDirectoryAdapter` bằng client HTTP/gRPC, module `transcript` không phải sửa.

---

## 5. Pipeline realtime (khung, sẽ chi tiết hoá sau)

### 5.1 Ngân sách latency từ số đo của bản gốc (`08` §1)

| Chặng | Số đo (bản Python) | Ghi chú |
|---|---|---|
| Endpointing (hangover) | 250 / 576 / 864 ms | Tính vào "từ lúc hết câu" (NFR-PF-04). Thường là khoản lớn nhất có thể tinh chỉnh |
| STT commit → completed | p50 774 ms | Flash mode + model streaming |
| MT câu đầu (realtime) | 655–919 ms | Fallback sang chat model, timeout 2 s |
| TTS first audio | 180 ms (warm) / 669 ms (cold) | Phải giữ warm pool |
| **Tổng ước tính** | khoảng 1.9–2.7 s | Mục tiêu 2–2.5 s (NFR-PF-05) |

Rust không làm nhanh các API bên ngoài. Phần thắng nằm ở: bỏ hop Redis và serialize audio, không có GIL / event-loop bị CPU chặn khi chạy VAD, `tokio::select!` để huỷ và timeout chính xác, và warm pool ổn định.

### 5.2 Hợp đồng giữa các chặng (dự kiến)

```rust
// warptalk-pipeline-runtime
pub struct Utterance { room, speaker, seq, lang, pcm_or_stream, started_at }
pub struct SttSegment { segment_id, seq, text, lang, confidence, is_early, t_commit }
pub struct Translation { segment_id, target_lang, text, model, t_first_token }
pub struct DubChunk { segment_id, target_lang, voice, pcm, is_last }
```

Mỗi chặng là một task với `mpsc` có giới hạn (backpressure). Mỗi message mang `Instant` gốc để đo latency từng chặng (`warptalk_stage_latency_ms`, giữ tên metric của bản gốc).

### 5.3 Degradation ladder (NFR-RL-03)

`voice clone lỗi → TTS chuẩn → chỉ phụ đề`. Mỗi lần chuyển bậc phát event `AudioRoutingEventType` (`10` §3.7) để `room` cập nhật trạng thái route.

---

## 6. Stack đề xuất (thêm vào `[workspace.dependencies]` khi bắt đầu code)

| Nhu cầu | Crate |
|---|---|
| Async runtime | `tokio` |
| HTTP + WebSocket | `axum`, `tower`, `tower-http` |
| Postgres | `sqlx` (postgres, runtime-tokio, macros, uuid, chrono/time) |
| Redis | `redis` (tokio-comp, streams) hoặc `fred` |
| Serialize | `serde`, `serde_json` |
| Lỗi | `thiserror` (trong lib), `anyhow` (trong `apps/*`) |
| Log/trace/metric | `tracing`, `tracing-subscriber`, `opentelemetry`, `metrics` + `metrics-exporter-prometheus` |
| JWT, hash mật khẩu | `jsonwebtoken`, `pbkdf2` + `sha2` (theo NFR-SC-02, chờ OI-01) |
| HTTP client | `reqwest` (rustls) |
| WebSocket client (OpenAI, Cartesia) | `tokio-tungstenite` |
| LiveKit | `livekit` (RTC SDK) + `livekit-api` (token, room service, webhook) |
| VAD (Silero ONNX) | `ort` |
| Resample | `rubato` |
| S3 | `aws-sdk-s3` hoặc `object_store` |
| Qdrant | `qdrant-client` |
| Test | `tokio::test`, `testcontainers` (Postgres, Redis), `insta` cho snapshot |

---

## 7. Lộ trình gợi ý

| Bước | Nội dung | Kết quả |
|---|---|---|
| 0 | Khung workspace (xong), CI `fmt` + `clippy` + `test` | `cargo check` xanh |
| 1 | `shared/*`: config, telemetry, kernel, db, bus; migrator; `identity` (register, login, JWT) | Login được |
| 2 | `workspace` + `room` (tạo, join, lobby, start/end, audio routes publish lên Redis) | Có room sống |
| 3 | `meeting` (LiveKit token + webhook) | Vào được LiveKit room từ web |
| 4 | **Pipeline vertical slice**: 1 speaker, 1 ngôn ngữ đích, caption only | Đo được STT + MT |
| 5 | TTS + egress LiveKit, warm pools, degradation | Speech-to-speech |
| 6 | `transcript` consumer (worker), `billing` metering idempotent | Dữ liệu bền + trừ credit |
| 7 | Phần còn lại: assistant/RAG, notification, chat, bridge, suggestion | Đủ tính năng |

Có thể làm song song bước 4–5 với 2–3 nếu dùng room giả (đọc route từ Redis key do script ghi tay).

---

## 8. Câu hỏi mở cần chốt

| ID | Câu hỏi | Ảnh hưởng |
|---|---|---|
| Q1 | Client Web/Desktop hiện tại có giữ nguyên không? Nếu có, WS hub phải nói được **giao thức SignalR** (JSON hub protocol), hoặc client phải sửa | ADR-5, toàn bộ realtime UI |
| Q2 | Có cần chạy song song với hệ thống .NET/Python cũ (dùng chung Redis/Postgres) trong giai đoạn chuyển đổi không? | Mức độ giữ nguyên tên stream/key/payload |
| Q3 | Bản gốc TTS synth theo listener hay theo (speaker, lang)? (D5) | Chi phí Cartesia, thiết kế egress |
| Q4 | OI-01 hash mật khẩu, OI-03 đơn vị billing, RV-01 `segment_id` trong `usage_records` | Migration `identity`, `billing` |
| Q5 | Tài liệu đọc từ code cho Auth, Workspace, Billing, Notification, Assistant? | Độ chính xác khi viết lại các module này |
