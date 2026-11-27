# WarpTalk: Runbook demo (dev, macOS)

**Định vị hiện tại (ADR 0001):** WarpTalk là *meeting assistant copilot* (transcript, gợi ý, follow-up, biên bản). Dịch họp, dub, voice clone và cầu nối họp ngoài (LiveKit) được **tạm ẩn sau feature flag, mặc định tắt**; code vẫn còn. Stack mặc định chỉ gồm Postgres, Redis, Qdrant, api, worker và web: **không LiveKit, không pipeline**. Muốn chạy lại phần dịch: `scripts/dev-up.sh --start --translation` (mục 2) và bật flag (mục 2.1).

Tài liệu này là hướng dẫn để dựng và chạy demo trên một máy Mac. Mọi tên biến môi trường bên dưới đã được đối chiếu với code (mục 3 ghi file nguồn). Mọi đường dẫn tính từ thư mục gốc repo.

Mục lục: 1. Yêu cầu máy · 2. Khởi động nhanh · 3. Biến môi trường · 4. Chạy từng thành phần · 5. Cấu hình key thật · 6. Pipeline với LiveKit thật · 7. App macOS · 8. Kịch bản demo · 9. Đo latency · 10. Chạy test · 11. Troubleshooting · 12. Giới hạn đã biết

---

## 1. Yêu cầu máy

| Cần | Phiên bản | Cài | Dùng cho |
|---|---|---|---|
| Rust toolchain | theo `rust-toolchain.toml` | `curl https://sh.rustup.rs -sSf \| sh` (rustup tự đọc `rust-toolchain.toml`) | api, worker, pipeline, migrator |
| Node.js | 22 | `brew install node@22` | `apps/web` (Next.js 16, Playwright) |
| Docker | Docker Desktop hoặc OrbStack, Compose v2 | `brew install --cask docker` | Postgres, Redis, Qdrant, LiveKit |
| `jq`, `curl`, `openssl` | bất kỳ | `brew install jq` (curl, openssl có sẵn) | `scripts/demo-seed.sh`, `scripts/dev-up.sh` |
| `redis-cli` | bất kỳ | `brew install redis` (chỉ cần client) | đọc token xác minh email; Playwright |
| `psql` | bất kỳ | `brew install libpq && brew link --force libpq` | Playwright fixtures, kiểm tra DB |
| Xcode + xcodegen | Xcode 15.3+, macOS 14.2+ | App Store; `brew install xcodegen` | app macOS (mục 7) |
| BlackHole 2ch | bản đã ký | `brew install --cask blackhole-2ch` | **tuỳ chọn**, chỉ demo voice họp ngoài |
| LLVM/clang 21 | >= 21 | `brew install llvm` | **tuỳ chọn**, chỉ khi build pipeline với LiveKit RTC (mục 6) |

Kiểm tra nhanh: `cargo --version && node --version && docker compose version && jq --version && redis-cli --version`.

Cổng cần trống: 5432 (Postgres), 6379 (Redis), 6333/6334 (Qdrant), 7880/7881/UDP 50000-50100 (LiveKit), 8090 (API, `API_PORT`), 3000 (web). Đổi cổng Postgres/Redis/Qdrant trong `deploy/docker/.env` (`POSTGRES_PORT`, `REDIS_PORT`, `QDRANT_HTTP_PORT`) rồi sửa `DATABASE_URL`/`REDIS_URL`/`QDRANT_URL` cho khớp.

---

## 1.1 Cho tester: cả stack trong Docker (1 lệnh)

Không cần Rust/Node trên máy, chỉ Docker Desktop. Chạy postgres, redis, qdrant, livekit, pipeline, migrator, api, worker, web:

```bash
scripts/stack.sh up      # lần đầu build vài phút; tự tạo deploy/docker/.env, sinh JWT_SECRET/AI_KEYS_KEK, kiểm tra cổng
scripts/stack.sh status  # / logs api pipeline / down
```

Điền `OPENAI_API_KEY` trong `deploy/docker/.env` (thiếu thì WarpBot/pill không có transcript). API ở `http://localhost:8090` (đúng mặc định app macOS), web ở `http://localhost:3000`.

## 2. Khởi động nhanh (5 lệnh)

```bash
# 1. Hạ tầng (Postgres, Redis, Qdrant) + build + migrate + chạy api/worker nền + web (log ở ./.logs/)
#    Mặc định KHÔNG có LiveKit và KHÔNG có pipeline dịch/dub. Thêm --translation nếu cần (xem bên dưới).
scripts/dev-up.sh --start --web
# 2. Dữ liệu demo: 2 user, workspace, phòng vi -> en, ja (chỉ dùng được khi bật dịch họp, mục 2.1)
scripts/demo-seed.sh
# 3. Mở web
open http://localhost:3000
# 4. Đăng nhập; Home -> "Start copilot session" (hoặc /<workspace>/sessions)
# 5. Dừng: scripts/dev-up.sh --stop      (dừng cả docker: scripts/dev-up.sh --down)
```

Lần đầu `dev-up.sh` tự:
- tạo `deploy/docker/.env` từ `deploy/docker/.env.example` (file `.env` bị `.gitignore`, chứa key thật của bạn);
- sinh `JWT_SECRET` và `AI_KEYS_KEK` ngẫu nhiên và ghi vào `.env` (để restart không mất phiên đăng nhập);
- `docker compose up -d --wait` (**chỉ** Postgres, Redis, Qdrant), chờ Qdrant `/readyz`. LiveKit thuộc profile `translation` của compose nên không chạy trừ khi có `--translation`;
- `cargo build` 3 binary (migrator, api, worker; thêm pipeline khi có `--translation`; lần đầu mất vài phút), chạy `warptalk-migrator`.

**Dịch họp (`--translation`)**: thêm docker profile `translation` (LiveKit, chờ `http://localhost:7880` trả `OK`), build và chạy `warptalk-pipeline`. Chế độ pipeline mặc định khi đó là `--fake` (MT/TTS giả, transport mô phỏng, **không cần key**). Muốn dùng key thật: điền key vào `deploy/docker/.env` rồi `scripts/dev-up.sh --stop && scripts/dev-up.sh --start --real` (mục 5). Chạy `scripts/dev-up.sh` không tham số = chỉ chuẩn bị hạ tầng + migrate rồi **in** lệnh để tự chạy từng thành phần (mục 4).

### 2.1 Feature flags (web)

Web hỏi `GET /api/v1/features?workspaceId=` → `{translationMeetings, dubbing, voiceClone, externalBridge, copilot}` (camelCase; mặc định `false,false,false,false,true`; nếu endpoint chưa có/404 web dùng đúng các giá trị mặc định này). Cờ tắt thì web ẩn: tạo/bắt đầu họp dịch (`/meetings`, `/rooms`, `/join`), route/dub/chọn ngôn ngữ nghe, mục Voice clone trong Settings và trang `/external` (mục nav "External meeting" được thay bằng "Copilot"). Truy cập trực tiếp URL của tính năng đang ẩn sẽ chuyển về Home, không báo lỗi. Cách bật cờ ở backend: namespace settings `features` (xem tài liệu của module settings); chỉ bật `translationMeetings` khi đã chạy `--translation`.

Các cờ: `--start` chạy nền; `--web` thêm Next.js dev server; `--translation` thêm LiveKit + pipeline (ẩn mặc định); `--fake|--real` (chỉ có nghĩa với `--translation`); `--no-docker` (đã có Postgres/Redis riêng); `--no-build`; `--stop`; `--down`; `PROFILE=release` dùng `target/release`.

---

## 3. Biến môi trường

Nguồn: `crates/shared/config/src/lib.rs` (DATABASE_*, REDIS_*, JWT_*, HTTP_BIND, CORS_ORIGINS, LOG_FORMAT, RUST_LOG, METRICS_BIND), `apps/api/src/main.rs`, `apps/api/src/wiring.rs` (`LiveKitSettings`), `crates/shared/ai-plane/src/{ai,vault,validator,knowledge}.rs`, `apps/pipeline/src/config.rs`, `apps/pipeline/src/main.rs`, `apps/web/next.config.ts`, `apps/web/playwright.config.ts`, `scripts/e2e-backend.sh`, `apps/web/e2e/fixtures/*.ts`. Biến rỗng được coi như chưa đặt. **BÍ MẬT** = không commit, không dán vào chat/log.

Cột "ai dùng": **A** = `warptalk-api`, **W** = `warptalk-worker`, **P** = `warptalk-pipeline`, **M** = `warptalk-migrator`.

### 3.1 Dùng chung

| Biến | Ai dùng | Bắt buộc | Mặc định | Ví dụ | Ghi chú |
|---|---|---|---|---|---|
| `DATABASE_URL` | A W M (P tuỳ chọn) | **có** (A, W, M) | không | `postgres://postgres@localhost:5432/warptalk` | Có thể chứa mật khẩu: coi là bí mật. Pipeline thiếu biến này vẫn chạy với settings mặc định, không hot-reload |
| `DATABASE_MAX_CONNECTIONS` | A W P M | không | `10` | `20` | Migrator luôn dùng 2 |
| `REDIS_URL` | A W P | **có** | không | `redis://localhost:6379` | Migrator không dùng. Bí mật nếu có mật khẩu |
| `REDIS_STREAM_MAXLEN` | A W P | không | `200` | `500` | Độ dài tối đa stream kết quả (pipeline dùng cho sink kết quả) |
| `REDIS_STREAM_TTL_SECONDS` | A W P | không | `3600` | `7200` | Được đọc vào `RedisConfig` |
| `RUST_LOG` | A W P M | không | `info` | `info,warptalk_pipeline=debug` | Bộ lọc `tracing` |
| `LOG_FORMAT` | A W P M | không | `pretty` | `json` | Chỉ nhận `pretty` hoặc `json`, giá trị khác làm tiến trình thoát |
| `METRICS_BIND` | A P | không | tắt | `127.0.0.1:9090` | Phải là `host:port` hợp lệ. Mỗi tiến trình một cổng riêng. Worker và migrator không mở metrics. Để trống khi chạy dev |

### 3.2 API (`apps/api`)

| Biến | Bắt buộc | Mặc định | Ví dụ | Ghi chú |
|---|---|---|---|---|
| `JWT_SECRET` | **có** | không | chuỗi >= 32 byte | **BÍ MẬT**. Dưới 32 byte thì API không khởi động. Sinh: `openssl rand -base64 48 \| tr -d '\n=+/'` |
| `JWT_ISSUER` | không | `WarpTalk.AuthService` | | |
| `JWT_AUDIENCE` | không | `WarpTalk` | | |
| `JWT_ACCESS_TTL_SECONDS` | không | `900` | | |
| `JWT_REFRESH_TTL_SECONDS` | không | `2592000` (30 ngày) | | |
| `HTTP_BIND` | không | `0.0.0.0:8080` | `127.0.0.1:8090` | `dev-up.sh` dùng `127.0.0.1:8090` (khớp mặc định app macOS) |
| `CORS_ORIGINS` | không | rỗng | `http://localhost:3000,http://127.0.0.1:3000` | Phân tách dấu phẩy. Khi web đi qua rewrite `/api` của Next dev (hoặc bản export chạy qua proxy cùng origin) thì không cần |
| `DEV_MAIL_CAPTURE` | không | tắt | `1` | Phải đúng `1`. **Chỉ dev**: token xác minh email/lời mời lưu thô vào Redis (`dev:mail:verify:{email}`, `dev:mail:workspace-invite:{email}`, TTL 1 giờ). Thiếu biến này và thiếu `RESEND_API_KEY` thì API chỉ log độ dài token, nên không thể xác minh email |
| `LIVEKIT_URL` | không (cả 3 hoặc không) | không | `ws://localhost:7880` | Thiếu một trong ba: `POST /meetings/{id}/join` và `/bridge-token` trả `503 LIVEKIT_NOT_CONFIGURED`. URL này **được trả cho trình duyệt**, phải truy cập được từ máy người dùng |
| `LIVEKIT_API_KEY` | như trên | không | `devkey` | **BÍ MẬT** ở production |
| `LIVEKIT_API_SECRET` | như trên | không | `secret` | **BÍ MẬT** ở production |
| `AI_ALLOW_INSECURE_BASE_URL` | không | strict | `true` | Nhận `1`, `true`, `TRUE`, `yes`. Cho phép `baseUrl` BYO dạng http/loopback (ví dụ Ollama). Chỉ dev |
| `RESEND_API_KEY` | không (cần để gửi email thật) | không | `re_...` | **BÍ MẬT**. Có biến này (và không bật `DEV_MAIL_CAPTURE`) thì email xác minh, đặt lại mật khẩu và lời mời workspace được gửi qua Resend (`POST https://api.resend.com/emails`). Thiếu cả hai: API chỉ log và in WARN lúc khởi động, người dùng không xác minh/đặt lại được |
| `EMAIL_FROM` | bắt buộc khi có `RESEND_API_KEY` | không | `WarpTalk <no-reply@your-domain>` | Người gửi đã xác minh domain trên Resend. Thiếu thì API không khởi động |
| `APP_BASE_URL` | bắt buộc khi có `RESEND_API_KEY` | không | `https://app.your-domain` | Gốc URL web, dùng cho link trong email: `/verify-email?token=`, `/reset-password?token=`, `/invitations/accept?token=`. Thiếu thì API không khởi động |
| `EMAIL_BRAND_NAME` | không | `WarpTalk` | | Tên sản phẩm trong tiêu đề/nội dung email |
| `EMAIL_DEFAULT_LANG` | không | `vi` | `en` | Ngôn ngữ mẫu email (`vi` hoặc `en`) |
| `GOOGLE_CLIENT_IDS` | không | không | `123-web.apps.googleusercontent.com,123-mac.apps.googleusercontent.com` | Danh sách OAuth client id (phân tách dấu phẩy) được chấp nhận làm `aud` của Google ID token (web + macOS). Thiếu thì `POST /api/v1/auth/google` trả `503 GOOGLE_NOT_CONFIGURED` |
| `DEV_MAIL_CAPTURE` (bổ sung) | | | `1` | Ngoài `dev:mail:verify:{email}`, token đặt lại mật khẩu lưu ở `dev:mail:reset:{email}` (TTL 1 giờ). Ưu tiên cao hơn `RESEND_API_KEY` |
| `STRIPE_SECRET_KEY` | không (cần cho thanh toán) | không | `sk_test_...` | **BÍ MẬT**. Thiếu thì `POST /workspaces/{id}/billing/checkout` và `/portal` trả `503 STRIPE_NOT_CONFIGURED`; các route đọc vẫn chạy |
| `STRIPE_WEBHOOK_SECRET` | không (cần cho webhook) | không | `whsec_...` | **BÍ MẬT**. Thiếu thì `POST /billing/webhooks/stripe` trả 503; chữ ký sai trả 400 |
| `STRIPE_PRICE_PRO`, `STRIPE_PRICE_BUSINESS` | không | `plans.stripe_price_id` | `price_...` | Price id của gói; gói không có price id thì `purchasable=false` |
| `BILLING_SUCCESS_URL`, `BILLING_CANCEL_URL` | không | `{APP_BASE_URL}/billing?checkout=success` / `=cancelled` | | URL quay lại sau Stripe Checkout (mặc định localhost:3000 nếu không có `APP_BASE_URL`) |
| `BILLING_PORTAL_RETURN_URL` | không | `{APP_BASE_URL}/billing` | | URL quay lại từ Stripe Customer Portal |
| `BILLING_TOPUP_USD_PER_CREDIT` | không | `0.0002` | `0.0002` | Giá USD mỗi credit khi nạp thêm |
| `COPILOT_STT_MODEL` | không | `gpt-4o-transcribe` | | Tên model STT ghi vào usage của copilot (worker metering tính giá theo rate card) |
| `COPILOT_SERVER_STT` | không | `openai` nếu có `OPENAI_API_KEY`, ngược lại tắt | `off`/`fake`/`openai` | STT phía server cho WebSocket dự phòng `GET /copilot/sessions/{id}/audio` (app đẩy audio lên khi STT trực tiếp lỗi). `fake` = không cần key (demo/test). Tắt thì socket và `audio-ticket` trả 503 `SERVER_STT_NOT_CONFIGURED` |
| `COPILOT_AUDIO_MAX_SESSION_SECS` | không | `3300` | | Một socket audio bị đóng (`max_session_duration`) sau từng ấy giây (dưới giới hạn phiên 60 phút của provider); client tự nối lại ở utterance kế tiếp |
| `COPILOT_AUDIO_MAX_CONCURRENT_PER_USER` | không | `2` | | Số socket audio mở cùng lúc mỗi user (mỗi instance API) |
| `COPILOT_AUDIO_IDLE_TIMEOUT_SECS` / `_MAX_FRAME_BYTES` / `_MAX_UTTERANCE_MS` / `_TICKET_TTL_SECS` / `_FEED_TIMEOUT_MS` | không | `120` / `64000` / `300000` / `60` / `5000` | | Giới hạn của socket audio (idle, kích thước frame, audio tối đa giữa hai commit, tuổi thọ ticket một lần, ngưỡng provider chậm) |

Ngoài ra A và W còn đọc nhóm AI/Qdrant ở 3.3 và 3.4.

### 3.2.1 Billing enforcement

- Worker (`apps/worker`) đọc stream usage, trừ credit theo rate card và ghi trạng thái dịch vụ AI vào Redis: `workspace:{id}:ai_service_state` (`healthy|low_balance|in_overage|suspended`) và `workspace:{id}:ai_service_suspended` (lý do, ví dụ `insufficient_credits`, `trial_ended`; khoá bị xoá khi hết suspended).
- Khi workspace `suspended`: `POST /copilot/sessions/{id}/stt-token` trả `402 INSUFFICIENT_CREDITS`. Ingest transcript vẫn được nhận (không mất dữ liệu cuộc họp), kèm cờ `insufficientCredits` trên kết quả. Đọc trạng thái lỗi thì fail-open (không chặn họp).
- Webhook Stripe: cấu hình endpoint `POST /api/v1/billing/webhooks/stripe` trên Stripe Dashboard (ký bằng `STRIPE_WEBHOOK_SECRET`; xử lý idempotent theo event id).
- Hiện chỉ copilot bị chặn theo credit; assistant (gợi ý, tóm tắt) chỉ ghi usage chứ chưa bị chặn.
- API yêu cầu migration `subscription` (chạy `warptalk-migrator` trước).

### 3.3 AI nền tảng và BYO (A, W; riêng `OPENAI_API_KEY` còn P)

| Biến | Ai dùng | Bắt buộc | Mặc định | Ví dụ | Ghi chú |
|---|---|---|---|---|---|
| `OPENAI_API_KEY` | A W P | không (nhưng xem ghi chú) | không | `sk-...` | **BÍ MẬT**. P: STT + MT thật. A/W: embedding Knowledge, gợi ý, dịch chat. Thiếu cả `OPENAI_API_KEY` và `GEMINI_API_KEY` thì embedding bị tắt, indexing/search Knowledge lỗi |
| `OPENAI_BASE_URL` | A W | không | endpoint OpenAI | `https://api.openai.com/v1` | Dùng cho proxy/gateway |
| `ANTHROPIC_API_KEY` | A W | không | không | `sk-ant-...` | **BÍ MẬT** |
| `ANTHROPIC_BASE_URL` | A W | không | mặc định của provider | | |
| `GEMINI_API_KEY` | A W | không | không | `AIza...` | **BÍ MẬT** |
| `GEMINI_BASE_URL` | A W | không | mặc định của provider | | |
| `AI_KEYS_KEK` | A W | không (BYO tắt nếu thiếu) | không | base64 của đúng 32 byte | **BÍ MẬT**. Sinh: `openssl rand -base64 32`. Thiếu hoặc sai thì `PUT /ai/workspaces/{id}/config` trả `503 BYO_DISABLED`. Mất KEK = mất toàn bộ key BYO đã lưu |
| `AI_KEYS_KEK_VERSION` | A W | không | `1` | `2` | Số nguyên, dùng khi xoay KEK |
| `QDRANT_URL` | A W | không | in-memory (mất khi restart) | `http://localhost:6333` | Cổng **REST** 6333, không phải gRPC 6334 |
| `QDRANT_API_KEY` | A W | không | không | | **BÍ MẬT** |

### 3.4 Pipeline (`apps/pipeline`)

| Biến | Bắt buộc | Mặc định | Ví dụ | Ghi chú |
|---|---|---|---|---|
| `MEDIA_TRANSPORT` | không | **`livekit`** | `simulated` | `simulated` hoặc `livekit`. Mặc định là `livekit` nhưng binary build thường **không có** transport này và sẽ thoát với lỗi hướng dẫn (mục 6). `dev-up.sh` đặt `simulated` |
| `LIVEKIT_URL` / `LIVEKIT_API_KEY` / `LIVEKIT_API_SECRET` | có khi `MEDIA_TRANSPORT=livekit` | không | như mục 3.2 | Pipeline vào phòng LiveKit dưới tên agent ẩn. **BÍ MẬT** (key/secret) |
| `OPENAI_API_KEY` | có khi `livekit`; với `simulated` có thể thiếu | không | | Thiếu + `simulated` = STT giả. Có key = STT/MT thật (xem 3.3) |
| `OPENAI_STT_MODEL` | không | mặc định của `OpenAiSttConfig` | | |
| `CARTESIA_API_KEY` | không | không | | **BÍ MẬT**. Thiếu: chỉ có phụ đề dịch, không có tiếng dub |
| `PIPELINE_FAKE_PROVIDERS` | không | tắt | `1` | `1/true/yes/on`. MT/TTS giả với profile latency spec 08. **Chỉ có tác dụng khi KHÔNG có `OPENAI_API_KEY`** và bắt buộc `MEDIA_TRANSPORT=simulated` (không thì pipeline thoát). Vì vậy `dev-up.sh --fake` gỡ `OPENAI_API_KEY`/`CARTESIA_API_KEY` khỏi môi trường pipeline |
| `SIM_PARTICIPANTS` | không | rỗng | `id1,id2` | Chỉ `simulated`. Danh sách participant id "nói" trong mọi phòng mô phỏng. Rỗng = không ai nói (có cảnh báo). `demo-seed.sh` in ra giá trị đúng cho phòng vừa tạo |
| `SIM_WAV_DIR` | không | không | `/tmp/wav` | Nếu có `<dir>/<participantId>.wav` thì phát lặp file đó thay cho giọng tổng hợp |
| `PIPELINE_LEASE_TTL_MS` | không | `15000` | | Tối thiểu 1000 |
| `PIPELINE_SCAN_INTERVAL_MS` | không | `5000` | | `0` tắt quét snapshot route (chỉ nhận lệnh) |
| `PIPELINE_HOST_ID` | không | `<HOSTNAME hoặc "pipeline">-<hậu tố ngẫu nhiên>` | `pipe-1` | Phải khác nhau giữa các replica |
| `HOSTNAME` | không | `pipeline` | | Biến hệ thống, chỉ để sinh `PIPELINE_HOST_ID` |

### 3.5 Web (`apps/web`) và test

| Biến | Đọc ở | Mặc định | Ghi chú |
|---|---|---|---|
| `API_PROXY_TARGET` | `next.config.ts` (chỉ `next dev`) | `http://127.0.0.1:8090` | API mà `rewrites` của Next đẩy `/api/**` tới. Bộ E2E đặt `http://127.0.0.1:18080`; `dev-up.sh` đặt theo `HTTP_BIND` |
| `NEXT_PUBLIC_API_BASE_URL` | `src/lib/env.ts` (chỉ bản export `out/`) | rỗng (cùng origin) | Gốc backend (không có `/api/v1`) khi không có `window.warptalkNative.config.apiBaseUrl`. Bản bundle macOS dùng giá trị host tiêm vào |
| `WEB_PORT` | `scripts/dev-up.sh` | `3000` | Truyền cho `next dev -p`. `npm run dev` thuần luôn dùng 3000 |
| `E2E_WEB_PORT` | `playwright.config.ts` | `3199` | `next dev` của bộ E2E (`-H 127.0.0.1`, distDir `.next-e2e`) |
| `E2E_EXPORT_PORT` | `playwright.config.ts` | `3198` | Bản export tĩnh (`next build` + `scripts/serve-out.mjs`) cho spec `export.spec.ts` |
| `E2E_API_PORT` | `playwright.config.ts` | `18080` | |
| `PW_CHROMIUM` | `playwright.config.ts` | bản Chromium của Playwright | Đường dẫn Chromium khác nếu cần |
| `CI` | `playwright.config.ts` | không | Có `CI` thì Playwright KHÔNG tái dùng server đang chạy |
| `E2E_API` | `e2e/fixtures/api.ts` | `http://127.0.0.1:18080/api/v1` | |
| `E2E_REDIS` | `e2e/fixtures/api.ts` | `redis://localhost:6379` | Fixture gọi `redis-cli` |
| `PGHOST`, `PGUSER`, `PGDATABASE` | `e2e/fixtures/db.ts` | `localhost`, `postgres`, `warptalk` | Fixture gọi `psql` |
| `E2E_API_BIND`, `E2E_SKIP_BUILD` | `scripts/e2e-backend.sh` | `127.0.0.1:18080`, `0` | Script này **chỉ chạy được trên Linux** (xem mục 10) |
| `CARGO_TARGET_DIR` | cargo, `dev-up.sh` | `./target` | |

Biến chỉ dành cho Docker Compose (code Rust/JS **không đọc**): `POSTGRES_USER`, `POSTGRES_DB`, `POSTGRES_PORT`, `REDIS_PORT`, `QDRANT_HTTP_PORT`, `QDRANT_GRPC_PORT`, `MINIO_*`.

### 3.6 App macOS (`apps/macos`, UserDefaults, đổi trong WarpTalk > Settings, Cmd+,)

| Khoá | Mặc định | Ghi chú |
|---|---|---|
| `webURL` | `http://localhost:3000` | Origin của web app. Native bridge chỉ hoạt động đúng origin này (scheme + host + port) |
| `backendURL` | `http://localhost:8090` | Không kèm `/api/v1`. Chỉ dùng khi web không truyền `backendUrl` |
| `dubOutputDeviceUID` | rỗng (thiết bị hệ thống) | Chọn BlackHole 2ch để đưa tiếng dub vào Meet |
| `overlayClickThrough`, `overlayHiddenFromScreenShare` | `false` | Overlay phụ đề |

---

## 4. Chạy từng thành phần

Mọi lệnh ở thư mục gốc repo. Nạp env trước mỗi lệnh: `set -a; . deploy/docker/.env; set +a`.

```bash
# Hạ tầng mặc định: Postgres, Redis, Qdrant
docker compose -f deploy/docker/docker-compose.yml --env-file deploy/docker/.env up -d --wait
# Thêm LiveKit (dịch họp, tuỳ chọn): ... --profile translation up -d --wait ; curl http://localhost:7880 -> "OK"
curl http://localhost:6333/readyz               # Qdrant
# docker compose ... --profile minio up -d      # MinIO tuỳ chọn (chưa nối vào code)

# Build + migrate (phải xong trước api/worker)
cargo build -p warptalk-migrator -p warptalk-api -p warptalk-worker      # thêm -p warptalk-pipeline khi chạy dịch họp
target/debug/warptalk-migrator

# Mỗi lệnh một terminal
target/debug/warptalk-api                        # http://127.0.0.1:8090/health
target/debug/warptalk-worker
# Chỉ khi dịch họp (--translation). Pipeline giả (không cần key):
env -u OPENAI_API_KEY -u CARTESIA_API_KEY MEDIA_TRANSPORT=simulated PIPELINE_FAKE_PROVIDERS=1 \
    SIM_PARTICIPANTS="<id1>,<id2>" target/debug/warptalk-pipeline
# Pipeline key thật (transport mô phỏng, STT/MT/TTS thật; mục 5):
env -u PIPELINE_FAKE_PROVIDERS MEDIA_TRANSPORT=simulated target/debug/warptalk-pipeline
cd apps/web && npm install && npm run dev        # http://localhost:3000 (Next.js; API_PROXY_TARGET mặc định http://127.0.0.1:8090)
cd apps/web && npm run build && npm start      # bản export tĩnh apps/web/out (macOS bundle), phục vụ ở :3000 bởi scripts/serve-out.mjs
```

Vai trò: **API** (REST + webhook LiveKit, `/api/v1`), **worker** (consumer Redis: transcript, gợi ý, embedding/Knowledge, dịch chat), **pipeline** (STT, MT, TTS theo phòng), **migrator** (migration mọi module rồi thoát; API đòi module `identity`, `assistant` đã migrate, worker đòi `transcript`, `assistant`, `meeting`).

Hạ tầng: `docker compose ... down` giữ dữ liệu (volume `pgdata`, `redisdata`, `qdrantdata`); xoá sạch: `docker compose ... down -v`. Log nền của `dev-up.sh --start`: `./.logs/{api,worker,pipeline,web}.log` (tiến trình: `./.logs/*.pid`).

Compose chưa có container cho app vì repo chưa có Dockerfile; các khối `api/worker/pipeline` trong `docker-compose.yml` đang comment sẵn để bật khi có.

---

## 5. Cấu hình key thật

Điền vào `deploy/docker/.env` (không commit), rồi khởi động lại tiến trình tương ứng.

| Cần | Biến | Lấy ở đâu | Tác dụng |
|---|---|---|---|
| OpenAI | `OPENAI_API_KEY` | platform.openai.com | STT realtime + MT (pipeline); embedding, gợi ý, tóm tắt, dịch chat (api/worker) |
| Cartesia | `CARTESIA_API_KEY` | play.cartesia.ai | TTS: tiếng dub. Thiếu thì chỉ có phụ đề dịch |
| Anthropic | `ANTHROPIC_API_KEY` | console.anthropic.com | LLM nền tảng cho gợi ý/tóm tắt nếu cấu hình provider `anthropic` |
| Gemini | `GEMINI_API_KEY` | aistudio.google.com | LLM/embedding thay thế (Knowledge chấp nhận OpenAI **hoặc** Gemini) |
| KEK cho BYO | `AI_KEYS_KEK` | tự sinh | Mã hoá key AI của workspace |

Sinh KEK (đúng 32 byte, base64):

```bash
openssl rand -base64 32
```

`dev-up.sh` đã tự sinh sẵn một KEK dev vào `.env` ở lần chạy đầu. Với demo thật hãy giữ nguyên giá trị đó giữa các lần chạy: **đổi KEK sẽ làm key BYO đã lưu không giải mã được**.

Chuyển pipeline sang key thật: `scripts/dev-up.sh --stop && scripts/dev-up.sh --start --real --no-docker --no-build` (hoặc chạy lệnh "Pipeline key thật" ở mục 4). Log đầu tiên của pipeline cho biết chuỗi đang chạy: `chain: STT -> MT -> TTS -> dub` là đủ key; `running STT only` là thiếu `OPENAI_API_KEY`; `CARTESIA_API_KEY not set` là không có dub.

Kiểm tra: log worker không còn dòng `no AI key configured: chat messages are not translated`; log API không còn `knowledge embeddings disabled`.

---

## 6. Build pipeline với LiveKit thật

> Chỉ cần cho dịch họp / họp ngoài qua LiveKit, đang tạm ẩn (ADR 0001). Copilot không cần mục này.

Cần để pipeline **nghe audio thật** trong phòng LiveKit (họp native có dub, họp ngoài). Chi tiết: [`apps/pipeline/LIVEKIT.md`](../apps/pipeline/LIVEKIT.md). Tóm tắt, **chưa từng build được trong môi trường phát triển** (cần clang >= 21, container dev chỉ có clang 18):

1. Manifest đã có sẵn: `livekit` (optional) trong `[workspace.dependencies]` và feature `livekit-rtc = ["dep:livekit"]` trong `apps/pipeline/Cargo.toml`. Build mặc định không compile LiveKit; bật bằng `--features livekit-rtc`. Khi build có feature, `MEDIA_TRANSPORT` mặc định là `livekit`, còn không thì mặc định `simulated`.
2. Toolchain trên Mac:
   ```bash
   brew install llvm
   export CC=$(brew --prefix llvm)/bin/clang CXX=$(brew --prefix llvm)/bin/clang++
   $CC --version        # phải >= 21; Apple clang của Xcode không dùng được
   ```
3. Build (cần mạng: `webrtc-sys` tải libwebrtc dựng sẵn):
   ```bash
   cargo build -p warptalk-pipeline --features livekit-rtc --release
   ```
4. Chạy:
   ```bash
   set -a; . deploy/docker/.env; set +a
   MEDIA_TRANSPORT=livekit PIPELINE_FAKE_PROVIDERS= target/release/warptalk-pipeline
   ```
   Cần `LIVEKIT_*` (đã có trong `.env`), `OPENAI_API_KEY` (bắt buộc với livekit, không có STT giả), tuỳ chọn `CARTESIA_API_KEY`.
5. Checklist xác nhận sau lần build đầu (từ `LIVEKIT.md`): clippy với feature; hai người vào phòng LiveKit dev thì `XLEN stt:results` tăng (`redis-cli XLEN stt:results`) và phụ đề đến qua data channel topic `captions`; các dòng `// UNVERIFIED` trong `apps/pipeline/src/livekit_transport.rs`.

Giả định của transport: tên phòng LiveKit = id translation room; identity người tham gia = participant id; track dub tên `ai-interpreter-{lang}`. Nếu không build được: dùng `MEDIA_TRANSPORT=simulated` (kịch bản A/B ở mục 8 vẫn chạy được, nhưng audio của người thật không vào pipeline).

---

## 7. App macOS

Xem [`apps/macos/README.md`](../apps/macos/README.md) (build, ký, quyền) và [`apps/macos/BRIDGE.md`](../apps/macos/BRIDGE.md) (hợp đồng JS <-> native, mã lỗi, stub cho Playwright).

```bash
brew install xcodegen
cd apps/macos
cp Config/Local.xcconfig.example Config/Local.xcconfig   # điền DEVELOPMENT_TEAM (Apple ID miễn phí được) để macOS không hỏi lại quyền mỗi lần build
make gen && make open        # rồi Cmd+R trong Xcode; hoặc: make build && make run
make test                    # XCTest: bridge, NativeAPI, overlay, AudioActivity, Resampler
```

Điều kiện chạy: web đang chạy ở `http://localhost:3000` và API ở `http://localhost:8090` (đúng mặc định `dev-up.sh`). Trang trắng thì bấm Cmd+R trong app. Debug web: Safari > Develop > (tên máy).

**App chưa từng được biên dịch.** Lần build đầu có thể lỗi tên API. Danh sách `// VERIFY:` cần kiểm tra (chạy `grep -rn VERIFY apps/macos/Sources`):

| File | Điểm cần kiểm tra |
|---|---|
| `Audio/DeviceManager.swift:100` | Ký hiệu `kAudioHardwarePropertyProcessObjectList`, `kAudioProcessProperty*` (macOS 14.2+) |
| `Audio/Resampler.swift:27` | Trộn stereo -> mono của `AVAudioConverter` có phải trung bình |
| `Audio/ProcessTapCapture.swift:58,90` | Kiểu tham số initializer tap; tên khoá `kAudioAggregateDeviceMainSubDeviceKey` (cũ: `...MasterSubDeviceKey`) |
| `Web/NativeBridge.swift:31` | Chữ ký và `@MainActor` của closure reply (macOS 14 SDK) |
| `Web/WebViewModel.swift:53` | Delegate xin quyền media (macOS 12+) |
| `Features/Permissions/PermissionsManager.swift:66` | Pane id `Privacy_AudioCapture` |
| `Features/External/VoiceOutputPlayer.swift:54` | Đặt thiết bị ra trên `AVAudioEngine.outputNode.audioUnit` |
| `Features/External/DubListener.swift:24-73` | LiveKit Swift SDK 2.x: `Room`, `connect(url:token:)`, `remoteParticipants`, `audioTracks`, `set(subscribed:)`, `add(audioRenderer:)`, `volume`, `render(pcmBuffer:)` |
| `Features/External/BridgePublisher.swift:23,27,38` | `RoomOptions`, `AudioManager.shared.mixer` (`micVolume`/`appVolume`), `mixer.capture(appAudio:)`. Nếu SDK pin không có: Plan B trong README (chưa cài đặt) |

Quyền (System Settings > Privacy & Security): Micro; "Ghi âm thanh hệ thống" (Process Tap, hộp thoại hiện lần capture đầu tiên); "Ghi màn hình" chỉ cần khi dùng ScreenCaptureKit dự phòng. Reset: `tccutil reset Microphone com.example.warptalk` (bundle id là placeholder, xem `project.yml`).

---

## 8. Kịch bản demo từng bước

> Các kịch bản dưới đây là **dịch họp / dub / họp ngoài** (đang ẩn). Chạy `scripts/dev-up.sh --start --web --translation` và bật các flag tương ứng (mục 2.1) trước khi thử. Kịch bản copilot: Home -> "Start copilot session"; trên trình duyệt chỉ tạo phiên và xem transcript/biên bản, việc bắt audio cần app macOS (bridge `startCopilotSession`, P1, xem `apps/macos/BRIDGE.md`).

Chuẩn bị chung: `scripts/dev-up.sh --start --web --translation`, `scripts/demo-seed.sh`, ghi lại 2 email, mật khẩu, mã phòng. Dùng 2 cửa sổ trình duyệt: một thường, một ẩn danh (hai người khác nhau).

Phạm vi thật của từng kịch bản (đọc trước khi demo):

| | Cần |
|---|---|
| Họp native hình/tiếng + chat | LiveKit server (docker) |
| Phụ đề/dub tự động từ giọng người thật | Pipeline build với `livekit-rtc` (mục 6) + `OPENAI_API_KEY` (+ `CARTESIA_API_KEY` cho dub) |
| Họp ngoài (Meet) | App macOS + pipeline `livekit-rtc` + `OPENAI_API_KEY` |
| BYO / glossary / runtime settings | Chỉ API (+ `AI_KEYS_KEK` cho BYO) |
| Knowledge search | Qdrant + worker + `OPENAI_API_KEY` hoặc `GEMINI_API_KEY`; nội dung đến từ transcript họp (chưa có upload tài liệu) |

### 8.1 Họp native 2 người

1. Cửa sổ 1: đăng nhập Host. Vào workspace > Meetings > mở phòng "Demo vi-en-ja ...".
2. Bấm vào họp (Join). Cho phép Camera/Micro. API phát token LiveKit. Nếu thấy lỗi `LIVEKIT_NOT_CONFIGURED` thì API chạy thiếu `LIVEKIT_*` (đang dùng backend của bộ E2E?).
3. Cửa sổ 2 (ẩn danh): đăng nhập Guest, vào `http://localhost:3000/join`, nhập mã phòng do `demo-seed.sh` in ra, chọn ngôn ngữ nói/nghe.
4. Kỳ vọng: thấy video hai bên, chat hoạt động, trạng thái phòng ACTIVE. Với pipeline `livekit-rtc` + key: phụ đề đã dịch xuất hiện, và nghe tiếng dub theo ngôn ngữ nghe.
5. Không có pipeline RTC: demo dừng ở video + chat; có thể chạy pipeline `--fake` với `SIM_PARTICIPANTS` do `demo-seed.sh` in để thấy `redis-cli XLEN stt:results` tăng (kết quả mô phỏng chảy qua worker vào transcript). Chưa xác nhận giao diện hiển thị transcript mô phỏng ra sao.
6. Kết thúc: Host bấm End. Worker lưu transcript.

### 8.2 Họp ngoài (Google Meet), chỉ phụ đề (text-only)

Đây là đường demo an toàn nhất (không cần BlackHole). Cần: app macOS chạy (mục 7), pipeline `livekit-rtc` + `OPENAI_API_KEY`.

1. Mở WarpTalk.app (web 3000 + API 8090 đang chạy). Đăng nhập, vào trang External (`/external`).
2. Mở Meet trong Chrome, vào một cuộc họp, bật loa để Chrome phát tiếng (nếu chưa có tiếng sẽ gặp `NO_AUDIO_PROCESS`).
3. Trong web: dán link Meet, chọn ngôn ngữ nguồn và đích, chọn ứng dụng Chrome trong danh sách app có audio, bấm bắt đầu. App xin quyền "Ghi âm thanh hệ thống" lần đầu.
4. Kỳ vọng: overlay phụ đề nổi (Cmd+Shift+O bật/tắt), phụ đề cũng hiện trong web. Tiếng gốc vẫn nghe như cũ.
5. Máy thứ hai/tab khác: dùng cơ chế `takeover` để thấy lease capturer (heartbeat 409 thành `LEASE_LOST`).

### 8.3 Voice qua BlackHole

1. `brew install --cask blackhole-2ch`. Trong WarpTalk > Settings: "Đầu ra tiếng dub" = BlackHole 2ch.
2. Trong Meet: Microphone = BlackHole 2ch, Loa giữ nguyên thiết bị thật (để Process Tap bắt tiếng người kia).
3. Bắt đầu như 8.2 nhưng chọn chế độ voice (audio mode `voice`). Cần `CARTESIA_API_KEY` để có dub.
4. Kỳ vọng: người bên kia nghe tiếng dub qua mic ảo; Process Tap chỉ lấy tiến trình app họp nên tiếng dub không vọng lại. Nếu `mixer.capture(appAudio:)` không có trong SDK pin thì BridgePublisher không chạy (mục 7, VERIFY).

### 8.4 AI settings BYO (key riêng của workspace)

1. Đăng nhập Host (OWNER). Settings > **AI providers** (`/{workspace}/settings/ai`). Chỉ OWNER/ADMIN thấy mục này.
2. Nếu thấy lỗi `BYO_DISABLED`/trang báo BYO tắt: `AI_KEYS_KEK` chưa đặt (mục 5).
3. Chọn provider (OpenAI, Anthropic, Google Gemini, OpenAI-compatible endpoint), dán key, lưu: API gọi thử provider để kiểm tra key rồi mã hoá bằng KEK. Bấm "Test key" để thử lại, "Replace key", "Remove key".
4. Gán provider cho từng mục đích: Assistant, Meeting summaries, Suggestions (decide/write), Embeddings (đổi embedding sẽ re-index).
5. Endpoint tương thích OpenAI chạy local (ví dụ Ollama `http://localhost:11434/v1`): cần `AI_ALLOW_INSECURE_BASE_URL=true` cho API, nếu không bị từ chối `INVALID_BASE_URL`.
6. Kiểm tra bảo mật: key không xuất hiện trong response sau khi lưu và không có trong log.

### 8.5 Glossary

1. Menu Glossary (`/{workspace}/glossary`). Tạo glossary, thêm thuật ngữ nguồn -> đích (ví dụ `WarpTalk` -> `WarpTalk`, `chốt deal` -> `close the deal`).
2. Import CSV (cột `source,target`), export CSV.
3. Thuật ngữ được pipeline nạp qua Redis và áp dụng ở bước MT: nói câu có thuật ngữ trong họp (cần pipeline + OpenAI) rồi xem bản dịch.
4. System admin thêm: Settings > Global glossary (cần cấp quyền, xem 8.7).

### 8.6 Knowledge search

1. Cần Qdrant chạy (`QDRANT_URL`) và `OPENAI_API_KEY` hoặc `GEMINI_API_KEY`; worker chạy.
2. Transcript của các cuộc họp đã kết thúc được worker đưa vào chỉ mục (event `embedding:index_requests`).
3. Menu Knowledge (`/{workspace}/knowledge`): tìm theo từ khoá, lọc theo loại nguồn. Kết quả có trích dẫn (timestamp/trang).
4. Giới hạn: chưa có upload tài liệu workspace; member thường chưa có allowlist document/meeting (fail closed: kết quả có thể trống với MEMBER). Thử bằng Host.

### 8.7 Runtime settings

1. Workspace: Settings > **Advanced** (`/{workspace}/settings/advanced`, OWNER/ADMIN). Chỉnh các namespace (`pipeline`, `suggestion`, `rag`, `ai_providers`) theo workspace hoặc theo phòng; thay đổi có hiệu lực không cần restart (Redis pub/sub).
2. Phạm vi **platform** cần quyền SYSTEM_ADMIN. Không có API cấp quyền này; cấp bằng SQL (đổi email):
   ```bash
   psql -h localhost -U postgres -d warptalk -c "INSERT INTO auth.user_roles (id, user_id, role_id, assigned_at) SELECT gen_random_uuid(), u.id, r.id, now() FROM auth.users u, auth.roles r WHERE u.email = 'demo-host-xxxx@example.com' AND r.name = 'SYSTEM_ADMIN' ON CONFLICT DO NOTHING"
   ```
   Đăng xuất rồi đăng nhập lại để JWT mang cờ `sa`. Khi đó menu "Global glossary" và scope platform xuất hiện.
3. Thử: giảm `vad_silence_hangover_ms` của namespace `pipeline` (scope workspace/platform), xem log pipeline: `pipeline settings changed, reloading`.

---

## 9. Đo latency

Harness `pipeline-bench` chạy pipeline thật với provider giả theo spec 08 (không cần key, không cần mạng). Chi tiết số đo và giả định: [`docs/perf/README.md`](perf/README.md); số liệu: `docs/perf/{baseline,optimized,runs}.json`.

```bash
cargo build -p warptalk-pipeline --release --bin pipeline-bench
B=./target/release/pipeline-bench; C="--rooms 4 --speakers 4 --seconds 90 --seed 42 --json"
A="--speech-ms 2000 --pause-ms 3000 --sentence-ms 4000"
$B $C $A --baseline     # mọi tối ưu tắt
$B $C $A                # cấu hình mặc định đang chạy production
$B --rooms 2 --speakers 2 --seconds 20      # lần thử nhanh, kết quả dạng bảng (bỏ --json)
```

Cờ chính: `--rooms --speakers --seconds --seed --json --stt-only --baseline --early-endpoint-ms N --no-early-mt --no-tts-prewarm --mt-p50-ms --stt-p50-ms`; xem `--help` cho danh sách đầy đủ. Chỉ số quan trọng: `e2e_first_dub_audio`, `e2e_first_translated_caption`. Chỉ số này đo tại điểm publish, chưa gồm SFU và jitter buffer (khoảng +150 ms theo spec 08).

Đo thật khi chạy pipeline: đặt `METRICS_BIND=127.0.0.1:9090` cho pipeline rồi `curl http://127.0.0.1:9090/metrics | grep warptalk_stage_latency_ms`.

---

## 10. Chạy test

### Rust

Integration test cần Postgres và Redis cục bộ; test bỏ qua (in `DATABASE_URL unset; skipping`) nếu thiếu biến.

```bash
scripts/dev-up.sh                       # hoặc chỉ: docker compose -f deploy/docker/docker-compose.yml up -d --wait
export DATABASE_URL=postgres://postgres@localhost:5432/warptalk
export REDIS_URL=redis://localhost:6379
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # hoặc từng crate: cargo test -p warptalk-pipeline
```

Compose dùng trust auth cho Postgres nên URL không cần mật khẩu. Script `scripts/dev-services.sh` (Postgres + Redis không docker) **chỉ dành cho Linux** (dùng `pg_ctlcluster`, `/usr/lib/postgresql`), đừng chạy trên Mac.

### Web (Playwright E2E)

Backend của bộ E2E: API trên `127.0.0.1:18080`, LiveKit **không** cấu hình (test trạng thái "not configured"), `DEV_MAIL_CAPTURE=1`. Playwright tự khởi động `scripts/e2e-backend.sh`, nhưng script đó gọi `dev-services.sh` (Linux only) nên **trên Mac hãy chạy API E2E bằng tay** trước; Playwright thấy `:18080/health` sống thì tái dùng:

```bash
docker compose -f deploy/docker/docker-compose.yml up -d --wait
cargo build -p warptalk-migrator -p warptalk-api
set -a; . deploy/docker/.env; set +a
target/debug/warptalk-migrator
HTTP_BIND=127.0.0.1:18080 CORS_ORIGINS=http://127.0.0.1:3199,http://localhost:3199 DEV_MAIL_CAPTURE=1 \
  env -u LIVEKIT_URL -u LIVEKIT_API_KEY -u LIVEKIT_API_SECRET target/debug/warptalk-api &

cd apps/web
npm ci
npx playwright install chromium         # một lần
npx playwright test                     # toàn bộ (commit gần nhất ghi 208/208 pass, trước khi thêm spec glossary/language/members)
npx playwright test e2e/tests/auth.spec.ts --project=chromium    # một file
npx playwright test --update-snapshots  # cập nhật ảnh chụp
npx playwright show-report              # báo cáo HTML (apps/web/playwright-report)
```

Cần `redis-cli` và `psql` trên PATH (fixture gọi chúng, mục 1). Ảnh chụp visual lưu ở `apps/web/e2e/__screenshots__/`: trên Mac có thể lệch vài pixel so với ảnh sinh trên Linux (font), khi đó chạy `--update-snapshots` cho riêng máy bạn. Kế hoạch test: `apps/web/e2e/TEST_PLAN.md`. Gợi ý: đừng chạy bộ E2E cùng lúc với demo trên cùng DB nếu muốn dữ liệu demo sạch (E2E tạo nhiều user/workspace ngẫu nhiên).

### macOS

`cd apps/macos && make test`.

---

## 11. Troubleshooting

| Triệu chứng | Nguyên nhân / cách xử lý |
|---|---|
| `port is already allocated` / `address already in use` | Trùng cổng (Postgres/Redis Homebrew đang chạy ở 5432/6379). `brew services stop postgresql@16 redis` hoặc đổi `POSTGRES_PORT`/`REDIS_PORT` trong `.env` **và** `DATABASE_URL`/`REDIS_URL`. Xem ai giữ cổng: `lsof -nP -iTCP:8090 -sTCP:LISTEN` |
| API thoát: `database is not migrated (no 'identity' migrations found): run warptalk-migrator first` | Chưa chạy migrator hoặc chạy vào DB khác. `target/debug/warptalk-migrator` với cùng `DATABASE_URL`. Thứ tự do migrator quyết định (outbox, settings, identity, workspace, room, meeting, transcript, knowledge, assistant). Migration forward-only có checksum: **không sửa file migration đã chạy**; nếu đã lỡ, `docker compose ... down -v` rồi migrate lại |
| Worker thoát: `no 'transcript' / 'meeting' migrations` | Như trên (worker cần `transcript`, `assistant`, `meeting`) |
| API không khởi động: `JWT_SECRET ... must be at least 32 bytes` | Đặt `JWT_SECRET` >= 32 ký tự |
| Web: trang trắng hoặc `502` ở `/api/...` | Rewrite của Next dev tới sai cổng. `API_PROXY_TARGET` mặc định là `127.0.0.1:8090` (E2E dùng 18080); đặt lại rồi khởi động lại `npm run dev` (rewrites đọc lúc khởi động) |
| Đăng ký xong không xác minh được email | API chạy thiếu `DEV_MAIL_CAPTURE=1` (không có SMTP). Dev: `redis-cli GET dev:mail:verify:<email chữ thường>` rồi dán token vào trang `/verify-email`. `demo-seed.sh` tự làm |
| `503 LIVEKIT_NOT_CONFIGURED` khi join | API thiếu `LIVEKIT_URL/API_KEY/API_SECRET` (cả ba). Nạp `.env`. Backend E2E cố ý không có LiveKit |
| Join được nhưng không có hình/tiếng, ICE `failed`, kẹt "Connecting" | LiveKit trong Docker quảng bá `node_ip: 127.0.0.1` (`deploy/docker/livekit.yaml`): chỉ đúng khi trình duyệt/app cùng máy với Docker. Thiết bị khác trong LAN thì đặt `node_ip` = IP LAN, `LIVEKIT_URL=ws://<IP LAN>:7880` và mở UDP 50000-50100. Kiểm tra dải UDP đã publish: `docker compose ... ps`. VPN/Little Snitch/firewall chặn UDP cũng gây lỗi này; thử tắt VPN. Trên Docker Desktop không dùng `network_mode: host` |
| Webhook LiveKit không tới API (phòng không chuyển trạng thái khi mọi người thoát) | LiveKit gọi `http://api:8080/api/v1/meetings/webhooks/livekit` (API trong Docker, profile `apps`; API chạy ngoài Docker thì đổi thành `host.docker.internal:8090` trong `livekit.yaml` rồi `docker compose restart livekit`). Xem `docker compose ... logs livekit` |
| `curl http://localhost:7880` không trả `OK` | `docker compose -f deploy/docker/docker-compose.yml logs livekit`; thường do lỗi cú pháp `livekit.yaml` hoặc trùng cổng 7880 |
| Pipeline thoát: `MEDIA_TRANSPORT=livekit needs a build with the livekit-rtc feature` | Bạn đặt `MEDIA_TRANSPORT=livekit` với bản build thường. Bỏ biến đó (mặc định `simulated`) hoặc build `--features livekit-rtc` theo mục 6 |
| Pipeline thoát: `PIPELINE_FAKE_PROVIDERS is for MEDIA_TRANSPORT=simulated only` | Đừng dùng fake với livekit |
| Pipeline chạy nhưng "nobody speaks" | Thiếu `SIM_PARTICIPANTS` (transport mô phỏng) |
| Pipeline log `running STT only` | Thiếu `OPENAI_API_KEY` và không bật fake. Hoặc có key nhưng bạn muốn fake: fake bị bỏ qua khi có key, phải gỡ key khỏi môi trường pipeline (`env -u OPENAI_API_KEY`) |
| Build `webrtc-sys` lỗi C++ khó hiểu | clang cũ hơn 21. `$CC --version`, đặt `CC`/`CXX` (mục 6). Lỗi này là lỗi C++, không phải Rust |
| `/ai/workspaces/{id}/config` trả `503 BYO_DISABLED` | `AI_KEYS_KEK` thiếu hoặc không phải base64 của 32 byte |
| BYO với endpoint local bị `INVALID_BASE_URL` | Đặt `AI_ALLOW_INSECURE_BASE_URL=true` cho API |
| Knowledge search lỗi / trống | Không có `OPENAI_API_KEY` hoặc `GEMINI_API_KEY`; Qdrant chưa chạy (`curl localhost:6333/readyz`); worker chưa chạy; chưa có transcript |
| Worker log `error sending request` ở `embedding:index_requests` | Qdrant hoặc provider embedding không với tới được; consumer tự retry |
| Chat không được dịch | Thiếu key AI cho worker (log `no AI key configured`) |
| macOS: hộp thoại Micro/Audio hiện lại sau mỗi lần build | Ký ad-hoc đổi danh tính code. Dùng `Config/Local.xcconfig` với `DEVELOPMENT_TEAM` |
| macOS: quyền bị từ chối/kẹt (TCC) | Cmd+Q app, `tccutil reset Microphone com.example.warptalk` (hoặc `ScreenCapture`, `All`), mở lại, cấp lại trong System Settings > Privacy & Security. Cấp "Ghi màn hình" xong phải mở lại app |
| macOS: `NO_AUDIO_PROCESS` | Chrome/Zoom/Teams chưa phát tiếng; vào họp, bật loa rồi thử lại |
| macOS: trang web trong app trắng; bridge `FORBIDDEN_ORIGIN` | `webURL` trong Settings phải **trùng chính xác** origin web (`http://localhost:3000` khác `http://127.0.0.1:3000`). Nếu `localhost` phân giải sang `::1` mà trang không tải, chạy `npm run dev -- -H 127.0.0.1` và đổi cả hai nơi sang `127.0.0.1` |
| Docker Desktop báo thiếu RAM / LiveKit chậm | Cấp tối thiểu 4 GB cho Docker; dải UDP 101 cổng là mức tối thiểu hợp lý |
| Docker chưa chạy | `docker info`; mở Docker Desktop/OrbStack |
| `jq: command not found` trong `demo-seed.sh` | `brew install jq` |
| `demo-seed.sh`: không thấy token trong Redis | API không chạy với `DEV_MAIL_CAPTURE=1`, hoặc API dùng Redis khác `REDIS_URL` của script (`REDIS_URL=... scripts/demo-seed.sh`) |

---

## 12. Giới hạn đã biết

Nguồn: `docs/architecture/PROGRESS.md`, `apps/pipeline/LIVEKIT.md`, `apps/macos/README.md`, `docs/architecture/external-meeting.md`.

**Bị chặn bởi môi trường phát triển (cần quyết định của chủ repo):**
- Chưa build được LiveKit Rust RTC: `webrtc-sys` cần clang >= 21, máy dev có clang 18; tải LLVM 21 bị chặn. Transport `livekit-rtc` được viết theo mã nguồn `livekit 0.12.0` / `libwebrtc 0.4.2` nhưng **chưa biên dịch**; các lời gọi chưa kiểm chứng đánh dấu `// UNVERIFIED` trong `apps/pipeline/src/livekit_transport.rs`. Dependency đã khai báo sẵn (optional); chỉ cần `--features livekit-rtc` + clang >= 21 (mục 6).
- Chưa tải được binary `livekit-server` để test họp thật bằng Playwright: E2E họp dùng trạng thái "LiveKit not configured" hoặc mock. Luồng họp thật qua LiveKit chưa được kiểm thử tự động.
- Các host `openai.com`, `ai.google.dev`, `developers.google.com`, `developers.zoom.us`, `learn.microsoft.com` bị chặn khi nghiên cứu: mọi tuyên bố về API của các nền tảng này (kể cả Meet/Teams/Zoom trong `external-meeting.md`) đánh dấu **UNVERIFIED**.
- App macOS **chưa từng biên dịch** (viết trên Linux); toàn bộ danh sách `VERIFY` ở mục 7 chưa kiểm tra. Plan B của `BridgePublisher` chưa cài đặt. App Sandbox đang TẮT và Info.plist cho phép HTTP thường: chỉ cho demo.
- Số đo latency (`docs/perf`) dùng provider giả theo spec 08; các giả định (STT trả ranh giới câu trước commit, 20% TTS cold-open, TTS 2x realtime) cần xác nhận với provider thật. Chỉ số e2e không gồm SFU/jitter buffer.

**Tính năng chưa làm hoặc chưa đủ:**
- WarpBot: chat (SSE) + tool tìm kiếm knowledge đã có. **Plugin chưa làm**: catalog/install/connect (OAuth, API key), MCP client, tool policy, duyệt yêu cầu, marketplace workspace, audit (`/assistant/mcp/tools/audits`), insights (`/assistant/workspaces/{ws}/insights/tools`): các endpoint này trả 404, UI plugin phải ẩn. Đính kèm file trong chat đang tắt (`ATTACHMENTS_SUPPORTED = false`).
- Metering billing; notification; lời mời series/artifacts: chưa.
- Voice clone (spec 13): chưa. Voice consent đang là Noop.
- Knowledge: member chưa có allowlist meeting/document (fail closed); upload tài liệu workspace chưa làm.
- Pipeline chưa phát `__MEETING_END__`; isochrony cần `source_ms` trên `SttSegment`.
- `room`: chưa có capacity limit, credit check khi resume, endpoint bật/tắt voice clone, các trạng thái route degrade.
- `workspace` dùng role uuid cố định; BR-31/32 (một membership Internal) chưa enforce.
- Outbox chưa có lease khi claim (consumer phải idempotent). `IdentityUserDirectory` gọi từng user một.
- Họp ngoài trên macOS: không có far-speaker hint (mọi lời bên kia là "Google Meet participants"); lease capturer chưa phản ánh trạng thái audio (B8).
- Email thật: gửi qua Resend (`RESEND_API_KEY`, `EMAIL_FROM`, `APP_BASE_URL`); chưa có SMTP; dev dùng `DEV_MAIL_CAPTURE=1` (không bao giờ bật ở môi trường dùng chung). Mẫu email dùng một ngôn ngữ mặc định (`EMAIL_DEFAULT_LANG`), chưa theo ngôn ngữ từng người dùng.
- Bộ docker-compose này chỉ dành cho dev (LiveKit `--dev`, Postgres trust auth, khoá `devkey/secret`). Chưa có Dockerfile cho app.

## Knowledge: OCR và reindex (worker)

Biến môi trường của worker (`W`); mặc định tắt OCR nên PDF scan / ảnh kết thúc `skipped` với `skipReason=ocr_unavailable`.

| Biến | Bắt buộc | Mặc định | Ví dụ | Ghi chú |
|---|---|---|---|---|
| `KNOWLEDGE_OCR` | không | `none` | `tesseract` | `none` \| `tesseract` \| `openai`. Thiếu binary / khoá thì log WARN và quay về `none` |
| `KNOWLEDGE_TESSERACT_BIN` | không | `tesseract` | `/usr/bin/tesseract` | Tên hoặc đường dẫn; chạy bằng vector tham số, không qua shell |
| `KNOWLEDGE_PDFTOPPM_BIN` | không | `pdftoppm` | `/usr/bin/pdftoppm` | poppler-utils; thiếu thì ảnh vẫn OCR được nhưng PDF scan bị `skipped` |
| `KNOWLEDGE_OCR_LANGS` | không | `vie+eng` | `eng` | Chỉ chấp nhận `[A-Za-z0-9_+]` |
| `KNOWLEDGE_OCR_TIMEOUT_SECS` | không | `60` | `120` | Giới hạn cho từng tiến trình / request HTTP |
| `KNOWLEDGE_OCR_MAX_PAGES` | không | `30` | `50` | Chỉ nhận dạng N trang đầu của PDF scan |
| `KNOWLEDGE_OCR_DPI` | không | `200` | `300` | 72-600 |
| `KNOWLEDGE_OCR_OPENAI_MODEL` | không | `gpt-4o-mini` | | Chế độ `openai`: model vision. **Nội dung tài liệu rời nền tảng**: workspace tắt `externalLlmAllowed` thì tài liệu `skipped` (`external_llm_disallowed`). Chưa kiểm chứng với API thật (UNVERIFIED) |
| `KNOWLEDGE_OCR_OPENAI_BASE_URL` | không | `OPENAI_BASE_URL` hoặc `https://api.openai.com/v1` | | Dùng chung `OPENAI_API_KEY` |

- Docker: `docker build -f deploy/docker/Dockerfile.rust --build-arg WITH_OCR=1 --target worker .` cài `tesseract-ocr`, `tesseract-ocr-vie`, `poppler-utils`.
- `skipReason` (trong `GET /workspaces/{id}/documents[/{docId}]`): `ocr_unavailable`, `unsupported_type`, `invalid_encoding`, `external_llm_disallowed`, `no_indexable_content`, `ai_retrieval_not_allowed`.
- Reindex (owner/admin): `POST /workspaces/{id}/documents/{docId}/reindex` và `POST /workspaces/{id}/documents/reindex?status=skipped|failed|all&limit=50` (tối đa 200/lần; trả `remaining`, gọi lại để tiếp tục). Tài liệu `pending` không bị đụng tới nên gọi lặp là an toàn; `all` gồm cả `completed` (làm mới toàn bộ). Ảnh png/jpg/webp upload được và đi qua OCR.

## Worker: số pipeline gợi ý đồng thời (`SUGGESTION_MAX_PIPELINES`)

`SuggestionWorker` chạy tối đa N pipeline model cùng lúc trên toàn bộ phòng của một tiến trình worker (semaphore toàn cục; mỗi phòng vẫn chỉ 1 pipeline tại một thời điểm). Mặc định **8**.

| Biến | Bắt buộc | Mặc định | Ví dụ | Ghi chú |
|---|---|---|---|---|
| `SUGGESTION_MAX_PIPELINES` | không | `8` | `24` | Số nguyên 1 đến 512; thiếu, rỗng, `0` hoặc không đọc được thì về 8 (log WARN); trên 512 bị cắt về 512. Giá trị đang dùng được log ở INFO khi worker khởi động (`suggestion worker concurrency`). Đổi cần khởi động lại worker. Là giới hạn **mỗi tiến trình**: chạy k worker thì tổng là k x N |

Chọn giá trị theo `docs/perf/copilot-latency.md` (bench giả, mỗi phòng phát lại một cuộc họp theo thời gian thực):

- Dưới khoảng 10 phiên đồng thời, 8 là đủ: xếp hàng gần 0, độ trễ gần bằng `decide + generate` của model (~1,0 s).
- Ở khoảng **50 phiên đồng thời với 8 pipeline** bắt đầu xếp hàng: cụm tới 26 pipeline chờ, xếp hàng p50 1,3 s / p95 2,8 s, `answer` p95 4,0 s. Nâng lên **24** đưa `answer` p95 về 1,4 s và xếp hàng về 0.
- Quy tắc thô: `N >= (số phòng đang họp) x 0,08 x hệ số bùng nổ 3 đến 4`. Chỉ tiêu: thời gian xếp hàng p95 < 200 ms; vượt nghĩa là thiếu pipeline, không phải model chậm.
- Trần thực tế là rate limit của nhà cung cấp (TPM/RPM), bench giả không có: tăng N từng bước (8 -> 16 -> 24), theo dõi lỗi 429 / `Overloaded` và độ trễ, đừng tăng quá ngưỡng của khoá.
- Đo lại trước khi đổi: `warptalk-ai-eval bench-latency --sessions 1,10,50 --max-pipelines <N> --out out/bench.json` (xem `docs/perf/copilot-latency.md`, `docs/architecture/ai-eval.md`).

## API: giới hạn socket âm thanh copilot theo người dùng (Redis)

Giới hạn số socket `/copilot/sessions/{id}/audio` mỗi người dùng dùng chung Redis giữa mọi replica API (`copilot:audio-conns:{user}`, mỗi kết nối là một lease được heartbeat; replica chết thì lease hết hạn sau `COPILOT_AUDIO_LIMIT_LEASE_SECS`).

| Biến | Mặc định | Ghi chú |
|---|---|---|
| `COPILOT_AUDIO_MAX_CONCURRENT_PER_USER` | `2` | Vượt quá trả 429 `TOO_MANY_AUDIO_STREAMS` |
| `COPILOT_AUDIO_LIMIT_LEASE_SECS` | `60` | Tối thiểu 3; heartbeat mỗi lease/3 |
| `COPILOT_AUDIO_LIMIT_FAIL_OPEN` | `true` | Redis không liên lạc được: `true` cho kết nối qua (ưu tiên sẵn sàng), `false` trả 503 |
