# Phân tích NexQ (github.com/naxhq/NexQ) cho WarpTalk

Ngày: 2026-11-24. Nguồn: `git clone --depth 1` vào `/tmp/claude-0/nexq-src` (commit `9d304fd`, 2026-11-23), chỉ đọc file, không chạy script/build. Mọi `path:line` bên dưới tính từ gốc repo NexQ.

Tag: **[CODE]** = đọc trực tiếp trong source; **[DOC]** = README/docs của NexQ tự nói (chưa chứng minh bằng code); **INFERENCE** = suy luận, chưa có bằng chứng trực tiếp.

Giới hạn nghiên cứu: GitHub API bị chặn trong môi trường này (`gh api` trả 403) và clone chỉ có 1 commit, nên **không biết số sao, số fork, số lượt tải, số contributor, lịch sử commit**. README có badge nhưng giá trị là động. Có thể kiểm tra tay trên trang repo.

## 1. Sản phẩm, nền tảng, license, độ trưởng thành

NexQ là "AI meeting assistant và interview copilot" mã nguồn mở, chạy trên desktop, hiển thị overlay riêng tư trả lời câu hỏi trong cuộc họp/phỏng vấn (README.md:5-6). Đây **không phải** sản phẩm dịch thuật realtime: trọng tâm là transcript hai phía + gợi ý câu trả lời. Dịch chỉ là tính năng phụ (README.md "Live translation with 5 providers").

| Mục | Kết quả | Bằng chứng |
|---|---|---|
| Nền tảng | Windows 10/11 (chính), macOS Apple Silicon (beta, cần macOS 14.2+ cho system audio) | README.md badge Platform; README "Quick start" |
| License | **MIT**, Copyright (c) 2026 Vahid Alizadeh. Cho phép dùng thương mại, sửa, bán lại; điều kiện duy nhất là giữ nguyên thông báo bản quyền + license | LICENSE:1-14 [CODE] |
| Phiên bản | v2.44.0 (2026-11-23), `Cargo.toml:3`; CHANGELOG.md có khoảng 53 mục phiên bản, dùng semantic-release/conventional commits | CHANGELOG.md:1; Cargo.toml:3 |
| Tốc độ phát triển | Rất nhanh: v2.41.x đến v2.44.0 trong 2 ngày (22-23/11/2026) | CHANGELOG.md:1-30 |
| Chưa ký | Installer Windows không code-sign; macOS beta không notarize (`signingIdentity: "-"`, ad-hoc); người dùng phải chạy `xattr -cr` | README.md "Quick start"; src-tauri/tauri.macos.conf.json (signingIdentity "-"); SECURITY.md:61 |
| Cộng đồng | Có CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, issue template, CI + release workflow. Có file `devlog.md` và `Fix STT language.md` ở gốc là rác của một fork/contributor (nhóm Rus-Localization) | devlog.md:1-8 |
| Sao/release/tải | KHÔNG XÁC MINH (API bị chặn) | - |

Ghi chú license: MIT chỉ áp cho code NexQ. Mô hình/thư viện nó tải về hoặc bundle có license riêng (xem mục 5 và 6).

## 2. Kiến trúc

### 2.1 Stack

| Lớp | Công nghệ | Bằng chứng |
|---|---|---|
| Shell | **Tauri 2** (Rust backend + WebView: WebView2 trên Windows, WKWebView trên macOS) | docs/ARCHITECTURE.md:5-9; Cargo.toml:13 |
| Frontend | React 18 + TypeScript + Vite, Zustand, Tailwind/shadcn. Hai cửa sổ: `launcher` 900x650 và `overlay` 500x700 (always-on-top, trong suốt) | docs/ARCHITECTURE.md:"Windows" |
| Backend | Rust + tokio, SQLite (`rusqlite` bundled), `reqwest`, `tokio-tungstenite` | Cargo.toml:14-40 |
| Audio | `cpal` 0.18 (mic + WASAPI loopback), `coreaudio-sys` + `objc2` (macOS tap), `sonora` (WebRTC AEC3 thuần Rust), `libopus_sys` (encode recording) | Cargo.toml:41-52, 86-95 |
| ML local | `whisper-rs` (whisper.cpp), `ort` ONNX Runtime (load-dynamic, bundle `libonnxruntime.dylib`), sherpa-onnx, `tokenizers` | Cargo.toml:36-42; tauri.macos.conf.json (resources) |

Không có Swift, không có Electron. WarpTalk cũng dùng Rust backend nên nhiều module của NexQ gần với stack của ta (nhưng NexQ là app desktop một người dùng, ta là SaaS multi-tenant + app Swift native).

### 2.2 Thu âm

Hai "bên" độc lập: mic ("You") và system output ("Them"), mỗi bên có engine STT, ngôn ngữ, mute riêng (README.md "Why NexQ"; docs/ARCHITECTURE.md:"The meeting pipeline" mục 1).

| Nền tảng | Mic | Âm thanh cuộc họp ("Them") | Bằng chứng |
|---|---|---|---|
| Windows | `cpal` input | **WASAPI loopback** của thiết bị output đã chọn (qua cpal). Loopback không phát packet khi im lặng nên có code xử lý khoảng trống | src-tauri/src/audio/system_capture.rs:27-49 (macOS-gated opener ngược lại), audio/echo_cancel.rs:17, 295 |
| macOS 14.2+ | `cpal` input | **Core Audio Process Tap**: gọi `AudioHardwareCreateProcessTap` (resolve bằng `dlsym` để chạy được khi OS cũ), tạo `CATapDescription` bằng objc2 (`initExcludingProcesses:[] andDeviceUID:`; tức tap toàn bộ output của device, không loại process nào), bọc trong **aggregate device private**, đọc bằng IOProc rồi downmix mono + resample 16 kHz | audio/macos_system_capture.rs:195-275, 297-298 [CODE] |
| macOS < 14.2 | - | Không có native; dùng virtual loopback (BlackHole) như một input device | docs/user-guide/audio-setup.md:61; Info.plist (nhắc BlackHole) |

Điểm đáng chú ý:
- **Không dùng ScreenCaptureKit** để thu tiếng, không cần driver ảo, không cần quyền Screen Recording. Dùng `NSAudioCaptureUsageDescription` (quyền "System Audio Recording Only") (Info.plist; macos_system_capture.rs:59 thông báo lỗi trỏ tới Privacy & Security). Đây đúng là hướng "Core Audio tap" mà WarpTalk macOS nên cân nhắc.
- Entitlement: `device.audio-input`, `allow-jit`, `allow-unsigned-executable-memory`, `disable-library-validation` (để nạp onnxruntime không cùng team) (src-tauri/entitlements.plist). Ta dùng Swift native thì cần entitlement riêng, nhưng nên lưu ý chuyện ký thư viện ML đi kèm.
- Tap chạy trên thread riêng, ring queue lock-free (`CaptureQueue`, atomic read/write), `capture_supervisor.rs` (1751 dòng) tự dựng lại stream khi đổi thiết bị/lỗi; thư mục audio có `echo_cancel.rs` (AEC3) để tránh mic nghe lại loa, `side_swap.rs`, `recording_mixer.rs`.
- Tap macOS có hạn chế đã biết: tap toàn output device, không tách theo app (INFERENCE: vì danh sách process loại trừ rỗng). Muốn thu riêng Zoom/Meet cần `initStereoMixdownOfProcesses:`/per-process tap (INFERENCE, API của Apple, NexQ không làm).

### 2.3 Pipeline realtime

```
mic / system -> resample 16 kHz mono -> VAD (energy) -> STT router (per side)
 -> segment_accumulator (partial -> paragraph) -> transcript_update / transcript_final
 -> transcript_buffer -> question_detector -> (Assist/Ask) -> prompt builder (+RAG) -> LLM stream -> overlay
```
(docs/ARCHITECTURE.md:"The meeting pipeline" 1-5)

- **VAD**: chỉ là energy-threshold (RMS 300 trên PCM16, timeout im lặng 1.5 s, làm mượt alpha 0.3). Không dùng Silero/ML VAD (audio/vad.rs:1-60 [CODE]). Với WarpTalk đây là mức thô; ta nên dùng VAD tốt hơn nếu cần cắt câu để MT.
- **Chunking/streaming**: tùy engine. Deepgram qua WebSocket `wss://api.deepgram.com/v1/listen` có interim/final (stt/deepgram.rs:3); Parakeet/Zipformer chạy streaming local qua ORT/sherpa; Whisper API gửi lô ~5 s nên "không live" (docs/user-guide/ai-providers.md:64); whisper.cpp local chỉ dùng cho bản ghi, không cho cuộc gọi live (README "Providers" bảng).
- Mẹo độ trễ: (a) **giữ session LLM sống** và chỉ gửi phần transcript mới (docs/user-guide/claude-subscription.md "How sessions work"); (b) tắt extended thinking cho Fast tier bằng env `CLAUDE_CODE_DISABLE_THINKING=1` ("first words usually under a second") (llm/cli_agent/command.rs:16-19, 74, 187-194); (c) Codex: tắt plugin/MCP/skills của user giảm khởi tạo từ 1.9-3.2 s xuống 0.25 s (llm/cli_agent/codex/launch.rs:30-32); (d) Fast/Deep tier model (Haiku-low cho live, Sonnet-medium cho recap) (claude-subscription.md "Fast and Deep models").
- Web Speech API trong WebView được dùng làm STT mic mặc định không cần key (src/hooks/useSpeechRecognition.ts:2). Trên macOS có thể trả `service-not-allowed` (audio-setup.md:85). Không áp dụng cho ta.

Không có TTS trong NexQ (grep `tts|text-to-speech` chỉ khớp chuỗi ngẫu nhiên như "speak", không có module TTS) [CODE]. **Pipeline STT→MT→TTS của WarpTalk không có tương đương**; NexQ chỉ đi STT → LLM → text trên overlay.

## 3. Lõi AI: lấy AI từ đâu (câu hỏi chính)

### 3.1 Bảng provider

| Chức năng | Local (chạy trên máy user) | Cloud bằng API key của user | Subscription của user |
|---|---|---|---|
| STT (12 engine) | Parakeet TDT, Sherpa-ONNX, ORT Streaming, Whisper.cpp (chỉ recording), Windows Speech | Deepgram, Groq Whisper, Whisper API (+endpoint tùy chỉnh), Azure Speech, Azure MAI Streaming, Cartesia, Web Speech (không key) | không |
| LLM (11) | Ollama, LM Studio | OpenAI, Anthropic, Gemini, Groq, OpenRouter, custom endpoint | **Claude (Pro/Max) qua `claude` CLI; Antigravity (Google AI plan) qua `agy acp`; Codex (ChatGPT plan) qua `codex app-server`** (beta) |
| Dịch (5) | OPUS-MT ONNX offline | DeepL, Google, Microsoft, hoặc "LLM đã cấu hình" | qua LLM subscription |

(README.md "Providers"; docs/user-guide/ai-providers.md:109-116)

**Không có server của NexQ nào.** Không có tài khoản, không có backend, không có proxy. Mọi request đi **từ máy client** thẳng tới provider user chọn (README.md "Privacy": "No telemetry... NexQ only contacts GitHub" cho update). Đây là khác biệt cốt lõi với WarpTalk (SaaS).

### 3.2 Cơ chế "dùng subscription": SPAWN CLI CHÍNH THỨC, không đụng token

NexQ **không** triển khai OAuth, không đọc session cookie, không tự bắt token, không browser automation. Nó chạy CLI chính thức mà user đã cài và đã đăng nhập, rồi nói chuyện với CLI qua stdin/stdout:

| Provider | Binary | Giao thức | Đăng nhập ở đâu | Bằng chứng |
|---|---|---|---|---|
| Claude | `claude` (Claude Code), `-p --input-format stream-json --output-format stream-json --verbose --include-partial-messages ... --model X --effort Y --system-prompt-file F` | stream-json, FIFO turn | User tự chạy `claude` rồi `/login` trong terminal riêng; NexQ chỉ có nút "Open terminal to log in" | llm/cli_agent/command.rs:100-159; claude-subscription.md "Setup"; session.rs:1-2 |
| Codex | `codex app-server` (JSON-RPC) | app-server protocol, `thread/start` | `codex login` chọn "Sign in with ChatGPT"; NexQ **từ chối** login bằng API key | llm/cli_agent/codex/launch.rs:1-32; status.rs:53-55, 90-93 |
| Antigravity | `agy acp --disable-telemetry --sandbox --gemini-dir <thư mục riêng>` | ACP (Agent Client Protocol) | `agy` login của Google | llm/cli_agent/acp/launch.rs:16-39; spawner.rs:6-10 |

Cách lấy/credential:
- **NexQ không bao giờ nhận credential subscription.** Chỉ đọc trạng thái: `claude auth status` (JSON: loggedIn, authMethod, subscriptionType, email) (llm/cli_agent/detect.rs:104-150, 169-170), `codex login status`. Ngoại lệ duy nhất: đọc `~/.codex/auth.json`, **parse payload JWT của `id_token` để lấy email + plan**, không đọc access/refresh token (llm/cli_agent/codex/models.rs:222-260; status.rs:12-13 "Tokens never leave the CLIs") [CODE]. Dù chỉ đọc claim, đây vẫn là đọc file chứa token của app khác.
- Gỡ biến môi trường để buộc dùng login subscription: `ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN, CLAUDE_CODE_OAUTH_TOKEN, ANTHROPIC_BASE_URL, CLAUDE_CODE_USE_BEDROCK/VERTEX/FOUNDRY` bị `env_remove` khỏi process con (command.rs:76-86, 175-177). Có phân loại login: chỉ `authMethod=="claude.ai"` + có `subscriptionType` mới coi là subscription, còn lại hiển thị "bills an API account" (detect.rs:89-111).
- Yêu cầu: user phải **tự cài CLI** (Claude Code từ code.claude.com; Codex qua `npm i -g @openai/codex`; Antigravity từ antigravity.google) (ai-providers.md:129). NexQ tìm binary mỗi lần spawn (spawner.rs:1-3). Tìm `claude`: trên Windows `claude.exe` (không nhận bản npm `.cmd`), trên Unix `~/.local/bin/claude` rồi PATH (detect.rs:54-80, spawner.rs:44-49).
- Gia cố/cô lập (vì CLI này là agent lập trình có công cụ nguy hiểm): Claude chạy `--safe-mode --tools "" --disallowedTools "*" --permission-prompts none --no-session-persistence`, cwd là sandbox rỗng `%LOCALAPPDATA%\NexQ\agent-sandbox` (command.rs:100-136, 161-167). Với "Personal sources" đổi sang `--restricted` + `--add-dir` + `--allowedTools` + `--strict-mcp-config`. Codex: `--disable shell_tool` + tắt plugin/hooks/memories/browser use, tắt từng MCP server của user theo tên, system prompt thay prompt coding mặc định (codex/launch.rs:4-28). Antigravity: `--gemini-dir` riêng để plugin/MCP của user không nạp, cwd rỗng xóa khi đóng. Ghi chú: các cờ `--safe-mode`, `--restricted`, `--permission-prompts` là của phiên bản Claude Code mà tác giả nhắm tới; ta **không xác minh** chúng tồn tại ở bản CLI công khai (INFERENCE: có thể là cờ mới 2026).
- Tool bị tắt nhưng vẫn có thông tin leak: transcript + tài liệu context gửi tới Anthropic/OpenAI/Google dưới điều khoản tài khoản của user (claude-subscription.md "Privacy").
- Session giữ lâu: một session Claude "Fast" mở lúc bắt đầu meeting, nạp sẵn system prompt + tài liệu (tới ~100k token), mỗi lần gọi chỉ gửi delta transcript + câu hỏi; session "Deep" cho meeting cũ, đóng sau 15 phút rảnh (claude-subscription.md "How sessions work").
- Fallback: nếu CLI lỗi/logout/rate-limit, chuyển sang "fallback provider" (API/Ollama) kèm toast; kiểm tra lại mỗi phút (claude-subscription.md "If Claude is unavailable").

### 3.3 Rủi ro ToS/kỹ thuật của cách này (đánh giá, không phải kết luận pháp lý)

| Rủi ro | Mô tả | Mức |
|---|---|---|
| Tính hợp lệ của "wrapper quanh CLI" | NexQ chạy binary chính thức không sửa đổi, nên không "extract token" như các công cụ bị cấm. Nhưng việc đưa nội dung cuộc họp tự động, liên tục (hàng trăm lượt/giờ) qua một CLI dành cho lập trình, bằng gói consumer, **có thể vượt quá "ordinary, individual usage"** theo điều khoản gói Pro/Max/ChatGPT. Điều khoản các hãng thay đổi và đã siết với third-party; ta **không đọc** điều khoản hiện hành trong lần nghiên cứu này, nên đây là INFERENCE cần luật sư/đọc lại | Cao nếu ta phân phối như tính năng thương mại |
| Hệ quả cho nhà phân phối | NexQ miễn phí/MIT: user tự cài CLI và tự chịu. Nếu WarpTalk thu tiền và nhúng/bắt buộc cách này, ta dễ bị coi là "reselling/commercial access to a subscription" | Cao |
| Dựa vào chi tiết bên trong CLI | Cờ CLI, giao thức stream-json/app-server/ACP, định dạng `auth.json`, `claude auth status` đều không phải API ổn định hợp đồng; NexQ có cả test fixture + probe version vì hay vỡ (cli_agent/fixtures, testdata, `mcp_handshake.rs` 1151 dòng, `routing.rs` 2299 dòng) | Cao (bảo trì) |
| Đọc `~/.codex/auth.json` | Dù chỉ lấy claim, đây là file chứa refresh token; không nên bắt chước | Trung bình |
| An toàn | CLI là agent có shell; chống bằng cờ + session kill nếu có tool-call (codex/launch.rs:25-26). Lỗ hổng một cờ mới là mất cô lập | Trung bình |
| Độ trễ/hiệu năng | Khởi tạo CLI 0.25 s tới vài giây, session ấm; không phù hợp luồng MT từng câu ngắn ở quy mô lớn (INFERENCE) | Trung bình |
| Trải nghiệm | User phải cài CLI + đăng nhập terminal; Windows-first (`sandbox_dir` dùng `LOCALAPPDATA`, path `claude.exe`) (command.rs:161-167). Mac có `find_claude_binary` Unix nhưng `sandbox_dir` fallback `temp_dir` (INFERENCE: ít kiểm thử trên mac) | Trung bình |

So với `siwc-research.md` của ta: NexQ **không** dùng "Sign in with ChatGPT" (OAuth devkit chính thức của OpenAI). Nó đi lối thứ ba: spawn CLI của hãng. Lối này tự phục vụ, không cần OpenAI duyệt, nhưng không có sự cho phép rõ ràng và chạy hoàn toàn trên máy user (đúng ràng buộc "request phải xuất phát từ runtime local của user" mà ta ghi trong siwc-research.md:20-26). Cũng như SIWC, nó **không cung cấp audio/STT/TTS**, chỉ text (và ảnh với Claude).

## 4. Tính năng dịch/trợ lý, prompt, cấu hình

- **Hành động LLM**: Assist, WhatToSay, Shorten, FollowUp, Recap, AskQuestion, MeetingSummary, ActionItemsExtraction, BookmarkSuggestions, MeetingAsk (intelligence/prompt_templates.rs:5-19). Prompt ngắn, tiếng Anh, kết thúc bằng "Respond in the same language used in the conversation transcript" (prompt_templates.rs:21-45). Có Scenarios (interview, lecture, webinar...) với prompt sửa được (README "Answer in the moment").
- **Question detection** (`intelligence/question_detector.rs`) + Auto-Answer tùy chọn; chọn phạm vi transcript (context scope).
- **RAG local**: SQLite FTS5 + embedding qua Ollama (docs/ARCHITECTURE.md "Supporting subsystems").
- **Dịch**: router 5 provider (DeepL/Google/Microsoft/LLM/OPUS-MT). Dịch bằng LLM chỉ là `translate_via_llm` dựng prompt "Translate the following text from X to Y" rồi đẩy qua LLMRouter (commands/translation_commands.rs:117-125, 475-493); provider trait `LlmTranslator` chỉ là marker (translation/llm_provider.rs:19-26). Dịch cloud: cắt >5000 ký tự, retry 1 lần, 3 lỗi liên tiếp thì phát `translation_error` (translation_commands.rs:127-160). Có cache kết quả. **Dịch thường là thao tác theo yêu cầu hoặc hậu kỳ (PostMeetingTranslationToolbar), không phải luồng dịch đồng thời liên tục** (INFERENCE từ cấu trúc UI: `TranslationPopup`, `PostMeetingTranslationToolbar`, `SideLanguagePicker`).
- **OPUS-MT offline**: tải encoder/decoder/tokenizer ONNX từ HuggingFace `Xenova/opus-mt-{src}-{tgt}`, từng cặp ngôn ngữ riêng (translation/opus_mt_registry.rs:25-86). Chất lượng thấp hơn LLM; mỗi cặp một model.
- **Cấu hình**: model Fast/Deep + effort + thinking theo từng provider (llm/thinking.rs, 713 dòng), AI Call Log (prompt, độ trễ, token, chi phí) để debug, ngôn ngữ STT theo từng bên đổi nóng giữa họp.
- Mô hình mặc định (Claude): Fast = Haiku/Low, Deep = Sonnet/Medium; có thể chọn Opus; Codex mặc định theo account (claude-subscription.md "Fast and Deep models"; codex/launch.rs:47).

## 5. Phân phối, đóng gói, riêng tư

- **Đóng gói**: Windows NSIS installer per-user, không cần admin, không ký; macOS `.app` + `.dmg`, hardened runtime nhưng ad-hoc sign, không notarize (tauri.conf.json:62; tauri.macos.conf.json; README "Quick start"). Auto-update bằng `tauri-plugin-updater`, chữ ký **Ed25519 (minisign)**, cài khi thoát app, không giữa họp (tauri.conf.json:84-85; README "Quick start"; docs/ARCHITECTURE.md "Supporting subsystems"). Khóa ký trong GitHub secrets (.github/workflows/release.yml:58-59).
- **Model tải riêng, không bundle** (trừ onnxruntime dylib): tải từ HuggingFace và GitHub releases của k2-fsa/sherpa-onnx về thư mục `models/` (stt/local_engines/model_registry.rs). Kích thước thực: Whisper tiny 75 MB / base 142 MB / small 488 MB / medium 1.5 GB / distil-large-v3 809 MB / large-v3-turbo 1.6 GB (model_registry.rs:27-94); Parakeet TDT 0.6B v3 int8 465 MB (đa ngữ 25 ngôn ngữ châu Âu), Parakeet CTC 110M 100 MB, Zipformer EN 122-296 MB, SenseVoice ~217 MB (zh/en/ja/ko/yue) (model_registry.rs:107-214; ai-providers.md:49).
- **Không có LLM local bundle**: chỉ tích hợp Ollama/LM Studio (user tự cài). Không dùng llama.cpp/MLX trực tiếp [CODE: không có crate `llama`/`mlx` trong Cargo.toml]. Không có TTS local.
- **Riêng tư**: "Nothing leaves your machine unless you use a cloud provider"; không telemetry/analytics; key trong Windows Credential Manager / macOS Keychain (`credentials/windows_cred.rs`, `macos_keychain.rs`); dữ liệu SQLite `nexq.db` + recordings trong app-data (README "Privacy"; docs/ARCHITECTURE.md "Persistence"). Mặc định ban đầu dùng cloud: Web Speech cho mic, Deepgram cho phía kia (README "Your first meeting"), tức mặc định **không** riêng tư.
- **"Stealth"**: "Hide from screen share" đặt `WDA_EXCLUDEFROMCAPTURE` (Windows) / `NSWindowSharingNone` (macOS), ẩn khỏi taskbar/Dock; thừa nhận trên macOS 14+ một số recorder dùng ScreenCaptureKit vẫn bắt được (README "Privacy"; commands/stealth_commands.rs:9-15, 720). Mục đích marketing là "interview copilot" hỗ trợ che giấu AI khỏi người phỏng vấn; README có mục "Use it responsibly" (xin phép ghi âm, đừng dùng nơi cấm AI). **Đây là điểm khác về đạo đức/thương hiệu: WarpTalk (dịch họp minh bạch) không nên thừa hưởng định vị "ẩn khỏi màn hình chia sẻ" và việc ghi âm không báo.**

## 6. WarpTalk có thể học gì, và rủi ro

### 6.1 Có thể học / tái sử dụng (MIT, giữ copyright notice nếu chép code)

| Hạng mục | Giá trị | Ghi chú |
|---|---|---|
| Core Audio Process Tap + aggregate private + IOProc ring queue (macos_system_capture.rs) | Cao | Cho app Swift của ta nên viết lại theo Swift API (`CATapDescription`, `AudioHardwareCreateProcessTap`) thay vì chép Rust/objc2; học các bẫy: tap format đổi giữa chừng (`invalid` flag), drift compensation `drift=1`, `dlsym` để tương thích 14.0/14.1, xử lý quyền `NSAudioCaptureUsageDescription` |
| Supervisor dựng lại stream khi đổi thiết bị; AEC3 (`sonora`) chống echo mic nghe loa | Cao (nếu ta thu mic + system cùng máy) | `sonora` là crate thuần Rust, kiểm tra license trước |
| Mô hình "hai bên" (You/Them) tách luồng, mỗi bên engine + ngôn ngữ riêng | Cao | Hợp với dịch hai chiều |
| Danh sách model local và kích thước (Parakeet TDT 0.6B v3 int8 465 MB, SenseVoice, Whisper) + `downloader` kiểm checksum (`sha2`) | Trung bình | Là dữ liệu tham khảo cho phương án "model local bundle" của ta; kiểm license từng model (Parakeet CC-BY-4.0, Whisper MIT; INFERENCE, chưa xác minh trong lần này) |
| OPUS-MT ONNX offline | Thấp-Trung bình | Chất lượng/độ phủ ngôn ngữ thấp hơn MT cloud/LLM, cặp ngôn ngữ riêng lẻ |
| Mẹo độ trễ LLM: giữ session ấm, gửi delta, thinking off, tier Fast/Deep | Cao | Áp dụng được cho adapter API (prompt caching) trong `byo-llm.md` mà không cần CLI |
| `AI Call Log` (prompt, latency, token, cost từng lượt) | Trung bình | Gợi ý cho observability |
| Cơ chế updater Ed25519 + cài khi thoát | Thấp (ta có đường riêng cho Swift/Sparkle) | - |

### 6.2 Câu trả lời cho câu hỏi chiến lược "AI từ subscription hay model local"

NexQ cho thấy **cả ba lối cùng tồn tại ở dạng BYO (tự mang)**, không lối nào là sản phẩm thương mại có SLA:
1. **Local**: khả thi cho STT (Parakeet/Whisper/Zipformer, 100 MB đến 1.6 GB tải riêng) và cho dịch yếu (OPUS-MT); LLM local chỉ nhờ Ollama của user. Không có TTS local. => Phương án "model local bundle" của ta tốn công nhất cho MT/TTS chất lượng; NexQ không chứng minh được chất lượng MT/TTS local.
2. **API key của user**: lối chính, ổn định, hợp pháp rõ ràng (khớp `byo-llm.md`).
3. **Subscription qua CLI**: chỉ là "beta" cho tính năng trợ lý văn bản, **không phải đường STT/MT/TTS realtime**; chạy trên máy user, đi qua CLI dành cho lập trình.

Khuyến nghị cho product owner (INFERENCE, dựa trên bằng chứng ở trên):
- **Không** sao chép lối spawn CLI cho sản phẩm thương mại làm đường chính. Rủi ro ToS (consumer plan dùng tự động trong sản phẩm bán tiền), mong manh kỹ thuật (giao thức nội bộ đổi theo phiên bản), và không giải quyết STT/TTS. Chỉ cân nhắc như tùy chọn "nâng cao, tự chịu rủi ro" ở app desktop **sau khi** luật sư xem điều khoản hiện hành của Anthropic/OpenAI/Google, và tuyệt đối không từ backend multi-tenant (khớp kết luận siwc-research.md).
- Giữ BYO API key (đã có `byo-llm.md`) là đường subscription-thay-thế an toàn; thêm local STT (Parakeet/whisper.cpp) làm tùy chọn offline/riêng tư; chấp nhận MT/TTS vẫn cloud hoặc BYO.
- Học kiến trúc thu âm hai bên + AEC + Core Audio tap, và các mẹo độ trễ.

### 6.3 Rủi ro khi chép cách làm/code của NexQ

| Rủi ro | Giải thích |
|---|---|
| License code | MIT cho phép dùng thương mại; **bắt buộc giữ nguyên dòng copyright + nội dung MIT** trong bản phân phối chứa code chép. Tuy nhiên theo quy ước dự án (CLAUDE.md) ta "port behaviour, not code". Cũng cần kiểm license các dependency/model/ONNX mà NexQ kéo vào (whisper.cpp MIT; sherpa-onnx Apache-2.0; ort MIT/Apache; OPUS-MT CC-BY-4.0; Parakeet CC-BY-4.0; Deepgram/Azure là dịch vụ trả phí) - tất cả là INFERENCE, chưa mở license từng cái |
| Tên/nhãn hiệu/hình ảnh | MIT không cấp quyền nhãn hiệu "NexQ", logo |
| Độ trưởng thành | Dự án rất mới (2026), một tác giả chính, thay đổi dồn dập (3 release/2 ngày), chưa ký/notarize; không có SLA, không có bằng chứng đã được kiểm thử ở quy mô lớn (chưa biết sao/tải) |
| Windows-first | macOS là beta; sandbox/spawn path và nhiều tính năng (call detection, screenshot) chỉ Windows (README "Features") |
| Định vị đạo đức/pháp lý | "Interview copilot", "hide from screen share", ghi âm hai bên: rủi ro đồng ý ghi âm (GDPR, hai bên đồng ý theo bang), khác với dịch họp minh bạch của WarpTalk |
| Ràng buộc kiến trúc của ta | Mô hình "client trực tiếp tới provider, không server" mâu thuẫn với SaaS multi-tenant/billing/quota. Mọi thứ về CLI subscription chỉ khả thi ở client desktop |
| Bảo mật chuỗi cung ứng | Spawn CLI bên thứ ba + MCP server của user mở rộng bề mặt tấn công; NexQ phải tự khóa bằng cờ |

## 7. Điểm chưa xác minh

- Số sao, release, lượt tải, số contributor (API bị chặn).
- Việc các cờ Claude CLI `--safe-mode`, `--restricted`, `--permission-prompts` có trong bản công khai hiện hành.
- Điều khoản sử dụng hiện hành của Claude Pro/Max, ChatGPT, Google AI về việc dùng CLI trong ứng dụng thứ ba.
- Chất lượng thực tế của Parakeet/OPUS-MT trên cặp ngôn ngữ của ta (VI, v.v.). Parakeet v3 không phủ VI theo mô tả của NexQ (25 ngôn ngữ châu Âu, ai-providers.md:49); cần kiểm riêng.
- macOS: chưa có bằng chứng CLI subscription chạy ổn trên mac (chỉ có `find_claude_binary` Unix).
