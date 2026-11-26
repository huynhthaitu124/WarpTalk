# ADR 0001 — WarpTalk là Meeting Assistant Copilot (desktop-first, không LiveKit)

- Trạng thái: **Chấp nhận** (2026-11-24, product owner)
- Thay thế một phần: ADR-2 trong `docs/architecture/README.md` (pipeline dịch realtime qua LiveKit) — chuyển sang trạng thái *tạm ẩn*.
- Liên quan: `siwc-research.md`, `nexq-analysis.md`, `byo-llm.md`, `external-meeting.md`, `CONTRACT_AUDIT.md`.

## Bối cảnh

WarpTalk ban đầu là nền tảng dịch họp realtime (STT → MT → TTS/dub qua LiveKit). Phần đó phức tạp, tốn hạ tầng media và phụ thuộc vào build LiveKit RTC chưa kiểm chứng. Product owner chốt định vị mới: **meeting assistant copilot** — trọng tâm là **transcript cuộc họp** và **suggestion / follow-up**, chạy trên mọi cuộc họp (Google Meet, Zoom, Teams, họp trực tiếp) thông qua app desktop, hiển thị bằng overlay. Không phải "interview copilot", không quảng bá khả năng ẩn khỏi người khác.

Ràng buộc đã biết:
- App thương mại → không dùng token subscription của user trên server (xem `siwc-research.md`); key AI của platform không được nằm trong app.
- Backend đã có: transcript (ingest idempotent, correction, finalize), suggestion engine cấu hình runtime + follow-up, knowledge/RAG, BYO LLM, settings runtime, SSE suggestions, glossary, documents.
- App macOS đã có: process tap (bắt audio app họp), overlay NSPanel, bridge JS↔native, WebView chạy web Next.js.

## Quyết định

1. **Ẩn dịch họp/dub/voice clone/bridge LiveKit sau feature flag** (`features` namespace, mặc định tắt). Giữ code, không deploy pipeline/LiveKit trong giai đoạn này.
2. **Khái niệm "Phiên copilot"** (copilot session) cho cuộc họp bên ngoài: một room loại `COPILOT_SESSION` thuộc workspace, có transcript, follow-up, biên bản; quyền xem theo workspace/host.
3. **Bắt audio trên máy user**: mic = kênh "Tôi" (`me`), process tap app họp = kênh "Người khác" (`others`).
4. **STT streaming gọi thẳng từ app tới provider bằng ephemeral token** do server cấp theo phiên (key platform chỉ ở server, token ngắn hạn). Dự phòng: app đẩy audio lên server dùng `warptalk-stt`. Tuỳ chọn riêng tư: WhisperKit local (P2). Mọi phương án cắm sau trait `SpeechToText`.
5. **copilot-core** (Rust, nhúng vào app qua UniFFI): dùng lại `warptalk-ingress` (VAD/endpointer) + client STT streaming; chỉ gửi đoạn có tiếng nói; gửi **segment transcript (text)** lên server.
6. **Server nhận segment** (`POST /copilot/sessions/{id}/segments`) và ghi vào `stt:results` theo đúng contract hiện có → transcript, suggestion engine, embedding knowledge chạy không cần sửa.
7. **Tóm tắt sau phiên**: worker nghe `__MEETING_END__` → summary, biên bản, action item (owner/deadline chỉ khi xuất hiện trong transcript), lưu và hiển thị trên web; dùng `LlmProvider` (platform/BYO).
8. **Overlay** hiển thị transcript live + suggestion + follow-up (SSE `/meetings/{id}/suggestions/stream`). Web là nơi xem lịch sử, biên bản, search.

## Phương án đã cân nhắc

| Phương án | Kết luận |
|---|---|
| Giữ pipeline LiveKit cho copilot | Loại: tốn hạ tầng, latency thêm hop, không cần fan-out cho một người |
| App gửi audio lên server, server STT | Dự phòng (đơn giản key, tốn băng thông + thêm hop) |
| Toàn bộ AI local (STT + LLM) | Tuỳ chọn riêng tư sau; chất lượng/thiết bị không đồng đều |
| Subscription ChatGPT/Claude/Gemini làm lõi | Loại cho lõi (điều khoản, không realtime); SIWC phía client cho assistant text nếu OpenAI duyệt |
| Chạy CLI subscription kiểu NexQ | Chỉ có thể là tuỳ chọn thử nghiệm có cảnh báo; không làm trong P0–P1 |

## Hệ quả

- Không cần LiveKit/`webrtc-sys`/clang 21 cho sản phẩm giai đoạn này; `docker-compose` mặc định bỏ LiveKit + pipeline (profile riêng).
- Cần xác minh API ephemeral token cho transcription session của OpenAI (UNVERIFIED trong môi trường dev).
- Quyền riêng tư: transcript lưu theo cấu hình workspace; thông báo ghi/lưu transcript cho người dùng; PII scan trước khi index (đã có).
- Nhận diện người nói giai đoạn đầu chỉ "Tôi/Người khác"; tên người nói từ caption Meet qua Accessibility ở P2.

## Lộ trình

- **P0** (backend + core, test được trong container): feature flags; phiên copilot + API segment + API ephemeral token; tóm tắt/biên bản/action item sau phiên; `copilot-core` chạy dạng CLI với file audio.
- **P1** (Mac): binding UniFFI; màn Phiên copilot + overlay transcript/suggestion/follow-up; web trang phiên, lịch sử, biên bản.
- **P2**: WhisperKit local, tên người nói từ caption, Ollama/LM Studio, SIWC phía client nếu được duyệt.
