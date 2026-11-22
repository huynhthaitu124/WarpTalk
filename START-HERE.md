# WarpTalk (Rust + Next.js + macOS) — bắt đầu từ đây

Bản này chạy được trên Mac (Apple Silicon/Intel). Hướng dẫn đầy đủ: [`docs/RUNBOOK.md`](docs/RUNBOOK.md).

## 1. Cài công cụ (một lần)

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Node 22, Docker Desktop, jq, redis-cli, psql
brew install node@22 jq redis libpq && brew install --cask docker
# App macOS (tuỳ chọn): Xcode 16.3+, xcodegen
brew install xcodegen
```

## 2. Chạy toàn bộ hệ thống (5 lệnh)

```bash
cp deploy/docker/.env.example deploy/docker/.env      # sửa key nếu có (OpenAI/Cartesia...)
bash scripts/dev-up.sh --start                        # Postgres, Redis, Qdrant, LiveKit (Docker) + migrator + api + worker + pipeline
(cd apps/web && npm ci && npm run dev)                # web: http://localhost:3000
API_URL=http://127.0.0.1:8080 bash scripts/demo-seed.sh   # tạo 2 user + workspace + room demo, in ra mật khẩu
open http://localhost:3000
```

Không có key AI: dùng `bash scripts/dev-up.sh --start --fake` (pipeline giả lập). Dừng: `bash scripts/dev-up.sh --stop`.

## 3. Phụ đề/dub từ giọng thật (LiveKit thật)

```bash
brew install llvm && export CC=$(brew --prefix llvm)/bin/clang CXX=$(brew --prefix llvm)/bin/clang++
cargo build -p warptalk-pipeline --features livekit-rtc --release
```
Chi tiết + các điểm chưa kiểm chứng: `apps/pipeline/LIVEKIT.md`.

## 4. App macOS

```bash
cd apps/macos && make gen && make build && make run      # cần Xcode 16.3+
```
Checklist lần chạy đầu, quyền hệ thống, 3 điểm VERIFY còn lại: `apps/macos/README.md`. Đóng gói web vào app: `make bundle`.

## 5. Kiểm thử

```bash
cargo test --workspace                                 # cần Postgres/Redis đang chạy (scripts/dev-up.sh)
(cd apps/web && npx playwright install chromium && npx playwright test)
cargo run -q -p warptalk-pipeline --bin pipeline-bench --release -- --rooms 4 --speakers 4 --seconds 60
```

## 6. Tài liệu chính

| File | Nội dung |
|---|---|
| `docs/architecture/README.md` | Kiến trúc, ADR |
| `docs/architecture/PROGRESS.md` | Đã xong / còn lại / bị chặn |
| `docs/architecture/byo-llm.md` | Thiết kế BYO ChatGPT/Claude/Gemini |
| `docs/architecture/external-meeting.md` | Meet/Teams/Zoom, macOS capture, test plan |
| `docs/architecture/CONTRACT_AUDIT.md` | Audit hợp đồng giữa các thành phần |
| `docs/perf/` | Số đo latency pipeline |
| `apps/web/DESIGN_SYSTEM.md`, `apps/web/LEGACY_AUDIT.md` | Design system, so sánh với legacy |
| `apps/web/e2e/TEST_PLAN.md` | Test case Playwright |

## 7. Giới hạn đã biết (đọc trước khi demo)

- Adapter LiveKit của pipeline và app macOS **chưa từng được compile** (môi trường phát triển không có clang 21 / Xcode). Lần build đầu trên Mac có thể cần sửa vài chỗ đánh dấu `UNVERIFIED` / `VERIFY`.
- Một số giao thức provider (OpenAI Realtime, Cartesia, Gemini) chưa đối chiếu được với tài liệu chính thức (bị chặn mạng) → đánh dấu `UNVERIFIED` trong code.
- PDF/DOCX chưa trích text (document vào trạng thái skipped). Chưa có billing, notification, quên mật khẩu, lịch.
