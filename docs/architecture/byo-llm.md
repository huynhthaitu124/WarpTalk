# BYO-LLM: workspace tự mang LLM provider (OpenAI / Anthropic / Gemini)

Trạng thái: đề xuất thiết kế (chưa có code). Ngày nghiên cứu: 2026-11-21. Phạm vi: (a) WarpBot (chat + tool call + RAG, spec 06 §6, 14 §5/§7), (b) live suggestion agent hai tầng decide/generate (spec 09 §10/§12/§14/§15). Realtime translation (STT/MT/TTS) vẫn chạy trên provider của nền tảng, ngoài phạm vi: độ trễ ngân sách ở `08` không cho phép đổi provider theo workspace.

Quy ước chú thích: `[V]` = đã đọc trang tài liệu chính thức. `[S]` = chỉ thấy qua snippet kết quả tìm kiếm của trang chính thức (môi trường chặn `platform.openai.com`, `developers.openai.com`, `ai.google.dev`, `openai.com` khi fetch, nên OpenAI và Gemini chủ yếu là `[S]`). `[?]` = không xác minh được, lấy từ kiến thức nền hoặc suy luận, phải kiểm tra trước khi code. Danh sách tổng hợp ở §11.

## 0. Hiện trạng bản gốc (upstream) và khoảng trống

| Bề mặt | Hiện trạng upstream | Hệ quả cho BYO |
|---|---|---|
| WarpBot chat | `AsyncOpenAI.responses.create(stream=True, tools=...)`, model reasoning (`gpt-5.6-luna`); lý do: reasoning model từ chối function tool trên chat completions (`chat_worker.py:900-907, 1079`) | Trait phải mô hình hoá reasoning + tool call + stream; OpenAI adapter dùng Responses API, không phải chat completions |
| Summary, knowledge facts | `chat.completions` + `response_format=json_object` (`assistant.py:236-246, 439-462`), gpt-4.1, 2048 tokens | Cần "JSON output" trừu tượng; Anthropic không nhận `json_object` (§2) nên chuẩn hoá về JSON-schema |
| Suggestion | decide `gpt-4o-mini` (64 tok) + generate `gpt-4.1` (200 tok), JSON mode, timeout 8 s, `max_retries=0`, lỗi thì im lặng (`config.py` `SuggestionSettings`; spec 09 §10, §13) | BYO phải giữ hợp đồng "lỗi => silence", không retry trong đường nóng |
| Key | một `api_key` toàn cục theo env (`SUGGESTION_`, `ASSISTANT_`, `ASSISTANT_CHAT_`) | Chưa có khái niệm key theo workspace |
| Consent | `allow_external_llm` (Redis `translationRoom:{room}:ai_policy`, fail-closed, cache 60 s) và `external_llm_allowed` trong các stream index/fact (spec 09 §8, 12 §6.2, 14 §3.4) | Dùng lại làm cổng bắt buộc cho BYO (§8) |
| DLP | Không có trên đường transcript live (spec 09 §8, §15 mục 8 là (proposal)) | BYO làm việc này khẩn cấp hơn vì dữ liệu rời hạ tầng khách |

## 1. Nghiên cứu từng provider

### 1.1 API shape, tool call, structured output, streaming

| Mục | OpenAI | Anthropic | Gemini |
|---|---|---|---|
| Endpoint chính | Responses API (khuyến nghị) và Chat Completions `[S]` developers.openai.com/api/docs/guides/migrate-to-responses | `POST /v1/messages` (header `x-api-key`, `anthropic-version`) `[V]` platform.claude.com/docs/en/api/errors | `generateContent` / `streamGenerateContent` `[S]` ai.google.dev/api/generate-content |
| Tool call | `tools[]` function + JSON-schema; kết quả gửi lại theo `call_id` `[?]` | `tools[].input_schema`; block `tool_use` / `tool_result` `[V]` (token-counting, errors) | `functionDeclarations`; part `functionCall`; trả `functionResponse` kèm id `[S]` ai.google.dev/gemini-api/docs/generate-content/function-calling |
| `tool_choice` ép buộc | có `[?]` | `any`/`tool` bị 400 trên Fable 5.2, Opus 5.5, Sonnet 5.5 (chỉ `auto`/`none`) `[V]` platform.claude.com/docs/en/api/errors | `[?]` |
| Structured output | `json_schema` strict; `store` mặc định lưu state 30 ngày `[S]` | `output_config.format = json_schema` và `strict: true` trên tool; schema giới hạn (không `oneOf`, không min/max số, `additionalProperties:false` bắt buộc) `[V]` platform.claude.com/docs/en/build-with-claude/structured-outputs | `responseMimeType` JSON + `responseJsonSchema` trong generation config `[S]` ai.google.dev/gemini-api/docs/generate-content/structured-output |
| Reasoning/thinking | `reasoning_effort`; tóm tắt reasoning qua event `response.reasoning_summary_text.delta` (upstream dùng) | Adaptive thinking, `effort`; block `thinking` có `signature` phải gửi lại nguyên vẹn khi tool use `[V]` errors page | Thought signatures `[S]` ai.google.dev/gemini-api/docs/generate-content/thought-signatures |
| SSE stream | Responses: `response.created`, `response.output_text.delta`, `response.completed`, `error`; usage ở `response.completed` `[S]` developers.openai.com/api/docs/guides/streaming-responses | `message_start`, `content_block_start/delta/stop`, `message_delta`, `message_stop`, `ping`, `error`; delta: `text_delta`, `input_json_delta`, `thinking_delta`, `signature_delta`; lỗi có thể xuất hiện sau HTTP 200 `[V]` platform.claude.com/docs/en/build-with-claude/streaming | `streamGenerateContent`; function call đến nguyên một chunk `[S]`; chi tiết `alt=sse` `[?]` |
| Đếm token | Trả về trong `usage`; không có endpoint đếm trước đã xác minh `[?]` | `POST /v1/messages/count_tokens`, miễn phí, ước lượng, rate limit riêng; tokenizer từ Opus 4.7 trở đi nhiều hơn khoảng 30% `[V]` token-counting | `countTokens` `[?]` (không xác minh được quota) |
| Chi tiết quan trọng | `store=false` nên đặt luôn vì ta tự lưu hội thoại | Không prefill từ 4.6; `temperature` <1 bị 400 trên 4.7+ nên bỏ trường này `[V]` openai-sdk page | Tool và built-in tool kết hợp được `[S]` |

### 1.2 Prompt caching (ảnh hưởng chi phí decide chạy khoảng 150 lần mỗi meeting)

| | OpenAI | Anthropic | Gemini |
|---|---|---|---|
| Cách bật | Tự động; `prompt_cache_key` để tách accounting theo khách; GPT-5.6+ có breakpoint tường minh (tối đa 4) `[S]` developers.openai.com/api/docs/guides/prompt-caching | `cache_control` ở top-level (tự động) hoặc từng block (tối đa 4 breakpoint); Bedrock `bedrock-runtime` không nhận top-level, phải dùng breakpoint tường minh `[V]` platform.claude.com/docs/en/build-with-claude/prompt-caching | Implicit mặc định cho 2.5+; explicit cache object có đảm bảo giảm giá `[S]` ai.google.dev/gemini-api/docs/generate-content/caching |
| Ngưỡng tối thiểu | 1.024 token (GPT-5.6+) `[S]` | 512 token cho Haiku 5.5/Sonnet 5.5/Opus 5.5; 1.024 cho Sonnet 5; 4.096 cho Haiku 4.5 `[V]` | Tài liệu mâu thuẫn (1.024 đến 4.096), diễn đàn báo cao hơn `[S]`; phải đo thực tế |
| Giá | Ghi 1,25x (30 phút), 2,5x (12 giờ, Responses); đọc 0,1x (0,05x trên GPT-6.1 Sol/Terra) `[S]` | Ghi 1,25x (5 phút) / 2x (1 giờ); đọc 0,1x, riêng Opus 5.5 và Sonnet 5.5 là 0,05x `[V]` | Chiết khấu đọc `[?]` |
| Trường usage | `cached_tokens` trong `usage` `[?]` | `cache_creation_input_tokens`, `cache_read_input_tokens`, `input_tokens` chỉ tính phần sau breakpoint cuối; tổng input = ba trường cộng lại `[V]` | `usageMetadata.cachedContentTokenCount` `[?]` |
| Tác động rate limit | `[?]` | Token đọc từ cache không tính vào ITPM `[V]` rate-limits | `[?]` |

Hệ quả thiết kế: `Usage` chuẩn hoá phải tách `input_uncached`, `cache_read`, `cache_write`, `output`, `reasoning`, vì Anthropic báo input chưa gồm cache còn OpenAI thường gồm `[?]`. Prompt phải xếp phần tĩnh (system, tool schema, tài liệu snapshot) lên đầu, phần động (cửa sổ transcript 8 lượt) xuống cuối để cache hiệu quả với cả ba provider.

### 1.3 Rate limit, lỗi đáng retry

| | OpenAI | Anthropic | Gemini |
|---|---|---|---|
| Header | `x-ratelimit-remaining-requests`, `x-ratelimit-reset-tokens`, `Retry-After` `[S]` developers.openai.com/api/docs/guides/rate-limits | `anthropic-ratelimit-{requests,tokens,input-tokens,output-tokens}-{limit,remaining,reset}`, `retry-after`, `request-id` `[V]` platform.claude.com/docs/en/api/rate-limits | Không tìm thấy header tài liệu hoá `[?]`; dựa vào mã lỗi |
| Retry được | 429 rate/`slow_down`, 408, 409, >=500 `[S]` developers.openai.com/api/docs/guides/error-codes | 429 (có `retry-after`), 500, 504, 529 `[V]` platform.claude.com/docs/en/api/errors | 429 `RESOURCE_EXHAUSTED`, 503 `UNAVAILABLE`, 408, 5xx `[S]` ai.google.dev/gemini-api/docs/troubleshooting |
| Không retry (cần người xử lý) | `credit_balance_exhausted`, `organization_spend_limit_exceeded`, `project_spend_limit_exceeded` (cũng là 429) `[S]` | 401, 402 `billing_error`, 403; 429 kèm `error.details.error_code = enforced_spend_limit_reached` (không có `retry-after`); spend limit tự đặt trả 400 `[V]` | 402 hết prepay credit, 400, 403 `[S]` |

Quy tắc: phân loại bằng `error_code` chứ không chỉ theo HTTP status, vì 429 vừa là "chậm lại" vừa là "hết tiền". Adapter map về `LlmError::{RateLimited{retry_after}, Overloaded, Timeout, Auth, Billing, QuotaExhausted, InvalidRequest, ContentFiltered, Unsupported, Transport}`.

### 1.4 Lưu trữ dữ liệu, training, vùng

| | OpenAI | Anthropic | Gemini |
|---|---|---|---|
| Training trên API traffic | Không, trừ khi khách opt in `[S]` developers.openai.com/api/docs/guides/your-data | Không dùng để train khi chưa có sự cho phép rõ ràng `[V]` platform.claude.com/docs/en/manage-claude/api-and-data-retention | Dịch vụ trả phí: không dùng để cải thiện sản phẩm mặc định; chia sẻ là opt-in `[S]` ai.google.dev/gemini-api/docs/logs-policy |
| Lưu mặc định | Abuse-monitoring log tới 30 ngày; Responses `store` giữ state 30 ngày `[S]` | Không lưu nội dung mặc định, ngoại lệ "Covered Models" (Fable/Mythos 5.x) bắt buộc lưu 30 ngày; nội dung bị gắn cờ có thể giữ tới 2 năm `[V]` | Log của khách tối đa 55 ngày (chọn 7/14/28/55); abuse monitoring 55 ngày `[S]` |
| ZDR | Cần OpenAI duyệt trước (ZDR hoặc Modified Abuse Monitoring), ép `store=false` `[S]` | Theo hợp đồng, theo từng organization; prompt caching và structured output (chỉ cache schema 24 giờ) đủ điều kiện; Batch, Files, code execution thì không `[V]` | Trên Vertex: `store=false`, vẫn có abuse log (Google model tới 90 ngày, model đối tác 30 ngày) `[S]` docs.cloud.google.com/vertex-ai/generative-ai/docs/vertex-ai-zero-data-retention |
| Vùng / thay thế doanh nghiệp | Data residency theo project (lưu trữ nhiều vùng, xử lý tại chỗ chỉ một số vùng; châu Âu cần MAM/ZDR) `[S]` developers.openai.com/api/docs/guides/data-residency; Azure OpenAI: Regional / Data Zone / Global (nguồn thứ cấp) `[S]` | `inference_geo` (`us`/`global`); Bedrock, Google Cloud (Anthropic-managed hoặc Google-managed), Microsoft Foundry, Claude Platform on AWS; trên Bedrock và Google-managed thì nhà cloud là data processor `[V]` | Vertex: endpoint vùng hoặc multi-region `us`/`eu` giữ xử lý trong vùng; endpoint `global` thì không `[S]` docs.cloud.google.com/vertex-ai/generative-ai/docs/learn/data-residency |

Hệ quả: lưu mỗi cấu hình một `data_boundary` và `retention_note` (admin tự khai, kèm cảnh báo UI), vì nền tảng không thể kiểm chứng hợp đồng ZDR của khách. Model có capability `requires_provider_retention` (Fable/Mythos 5.x) bị chặn khi workspace bật chế độ `strict_zdr`.

## 2. Endpoint tương thích OpenAI: đủ hay cần adapter native?

| Provider | Endpoint tương thích | Giới hạn | Kết luận |
|---|---|---|---|
| Anthropic | `https://api.anthropic.com/v1/` qua OpenAI SDK `[V]` platform.claude.com/docs/en/api/openai-sdk | Chính Anthropic nói "chủ yếu để test và so sánh, không phải giải pháp production lâu dài"; không có prompt caching; `strict` của tool bị bỏ qua; `response_format` chỉ nhận `json_schema` strict (`json_object` và `text` trả 400); `reasoning_effort` bị bỏ qua; không trả thinking; `usage.prompt_tokens_details` rỗng; system message bị gộp lên đầu | Không đủ: mất cache (đòn bẩy chi phí chính) và usage chi tiết. Cần adapter native |
| Gemini | `/v1beta/openai/` (Vertex dùng base URL khác, chỉ auth Google Cloud) `[S]` ai.google.dev/gemini-api/docs/openai | `reasoning_effort` chỉ low/medium/high ánh xạ ngân sách cố định, xung đột với `thinking_level/budget`; không tắt được thinking trên 2.5 Pro và 3; Google khuyến nghị gọi native nếu không dùng thư viện OpenAI | Dùng được cho thử nghiệm nhanh, nhưng caching, thought signature và usage thiếu chắc chắn `[?]`. Cần adapter native |
| OpenAI-compatible bên thứ ba (Azure OpenAI, gateway, vLLM, OpenRouter) | Chat Completions | Khả năng khác nhau tùy gateway | Giữ một adapter `openai_compatible` với capability flag bảo thủ, `base_url` + header tuỳ biến |

## 3. Họ model phù hợp từng vai trò (giá = USD/MTok input/output, tại 2026-11-21)

| Vai trò | OpenAI | Anthropic | Gemini |
|---|---|---|---|
| suggestion_decide (rẻ, nhanh, 64 token, JSON) | `gpt-6-luna` 0,10/0,50 `[S]` developers.openai.com/api/docs/pricing | `claude-haiku-5-5` từ 0,10/0,50 `[V]` platform.claude.com/docs/en/models/overview | `gemini-3.5-flash-lite` 0,30/2,50 (thinking mặc định minimal); `gemini-2.5-flash-lite` 0,10/0,40 `[S]` ai.google.dev/gemini-api/docs/pricing |
| assistant / suggestion_generate / summary (chất lượng) | `gpt-6.1-terra` 1,00/5,00 hoặc `gpt-6.1-sol` 2,00/10,00 `[S]` | `claude-sonnet-5-5` 2/10; `claude-opus-5-5` 4/20 `[V]` | dòng Flash mới nhất (giá `[?]`); Pro cho tác vụ khó |
| embedding | `text-embedding-3-small` 0,02, `-large` 0,13 `[S]` | Không thấy model embedding first-party trong bảng models `[V]`; đối tác (Voyage) `[?]` | `gemini-embedding-001` 0,15, 128-3072 chiều (khuyến nghị 768/1536/3072) `[S]` ai.google.dev/gemini-api/docs/embeddings |

Ghi chú vận hành: tên model đổi theo tuần (upstream đã ghi probe `gpt-5.6-luna` ngày 1/10/2026), nên hệ thống không hard-code danh sách model: `model` là chuỗi tự do, capability lấy từ registry (§5.3) cộng probe lúc validate. Claude 5.x và GPT-6 là reasoning model: `max_tokens` dùng chung cho reasoning và câu trả lời. Upstream từng dính 400 vì cạn 200 token (config.py). Do đó trait có `reasoning: Off|Low|Medium|High` và adapter tự cộng headroom. Ước lượng chi phí decide (giả định 150 lần x ~5k token prompt = 0,75M input) khoảng 0,08 USD/meeting ở giá 0,10/MTok trước cache; giả định, chưa đo.

Embedding phải nhất quán theo collection: vector của hai model không so sánh được, kích thước khác nhau. Đổi embedding provider = re-index (§6.5). Mặc định giữ embedding trên nền tảng; BYO embedding là tuỳ chọn riêng, tắt mặc định.

## 4. Crate Rust: so sánh và khuyến nghị

Số liệu crates.io (tra 2026-11-21), `[V]` crates.io/api/v1/crates/...: chưa đọc mã nguồn các crate nên đánh giá tính năng là `[?]`.

| Lựa chọn | Phiên bản / mức dùng | Ưu | Nhược với yêu cầu của ta |
|---|---|---|---|
| `async-openai` | 0.43.0, 11,2M downloads, cập nhật 2026-11-01 | Bao phủ OpenAI rộng, type đầy đủ | Chỉ OpenAI; type theo schema OpenAI, khó chuẩn hoá; vẫn phải tự viết Anthropic và Gemini |
| `genai` | 0.7.5 (pre-1.0), 643K downloads, cập nhật 2026-11-14; MIT/Apache; OpenAI, Gemini, Anthropic, Bedrock, Vertex, Ollama... | Một API cho nhiều provider | Trừu tượng hoá có thể che `cache_read/write`, thinking signature, error code chi tiết `[?]`; chưa 1.0 nên vỡ API thường xuyên; key theo từng request/workspace và `base_url` tuỳ biến cần kiểm tra |
| `rig-core` | 0.47.0, 4,6M downloads, cập nhật 2026-11-20 | Framework agent và RAG có sẵn | Ý đồ lớn hơn nhu cầu (ta đã có RAG qua Qdrant và vòng tool riêng); kéo nhiều dependency; ràng buộc kiến trúc |
| `gemini-rust` | 2.1.0, 62K downloads, cập nhật 2026-09-05 | Gemini native | Cộng đồng nhỏ, thêm phụ thuộc cho một provider |
| Crate Anthropic riêng | Không thấy ứng viên rõ trong kết quả tìm kiếm crates.io `[?]` | n/a | n/a |
| Tự viết adapter `reqwest` + `serde` + `async-trait` | đã có trong workspace | Kiểm soát trọn vẹn usage/cache/opaque reasoning/retry-after; không thêm dependency lạ; test bằng mock HTTP | Phải tự bảo trì khi API đổi; phải tự viết bộ parse SSE |

Khuyến nghị: **adapter mỏng tự viết**, mỗi provider một module (khoảng vài trăm dòng, `serde` struct cục bộ chỉ chứa trường ta dùng, phần còn lại giữ `serde_json::Value`). Lý do: (1) giá trị cốt lõi của BYO nằm ở những chi tiết mà crate đa provider hay làm phẳng (cache usage, block reasoning phải gửi lại nguyên vẹn, phân loại lỗi spend-limit vs rate-limit); (2) cần key/base_url/header theo từng request nên không muốn client singleton; (3) crate pre-1.0 và model churn hàng tuần làm rủi ro phụ thuộc cao; (4) đã dùng reqwest/serde/async-trait. Giữ `genai` làm phương án dự phòng để so sánh hành vi trong test, không đưa vào build.

Phụ thuộc workspace cần người sở hữu `Cargo.toml` gốc thêm trước khi code (tôi không sửa file đó): feature `stream` cho `reqwest` (hiện chỉ `json`, `rustls-tls`), `bytes`, `aes-gcm`, `zeroize`, `secrecy`, và dev-dependency mock HTTP (ví dụ `wiremock`). Parser SSE viết tay (khoảng 60 dòng) để khỏi thêm crate.

## 5. Thiết kế `crates/providers/llm`

### 5.1 Vị trí và trách nhiệm

`warptalk-llm` nằm ở `crates/providers/llm` (cạnh `warptalk-openai` vốn giữ Realtime WS cho STT/translation). Module `assistant`, `transcript`, suggestion worker phụ thuộc vào đây qua port trait (đúng luật `modules -> providers`). Provider crate không biết tới DB: nhận `ResolvedLlmConfig` (đã giải mã key) từ tầng ứng dụng.

### 5.2 Trait và mô hình chuẩn hoá (mô tả, không phải code)

| Phần | Nội dung |
|---|---|
| `LlmProvider` (async, `Arc<dyn>`) | `complete(req) -> Completion`; `stream(req) -> Stream<StreamEvent>`; `count_tokens(req) -> Option<u32>` (None nếu provider không hỗ trợ); `capabilities(model) -> ModelCaps`; `validate(cfg) -> ValidationReport` |
| `ChatRequest` | `model`, `messages`, `tools`, `tool_choice` (Auto/None/Required/Named, chỉ khi cap cho phép), `output` (Text, `JsonSchema{name, schema}`), `max_output_tokens`, `reasoning` (Off/Low/Medium/High), `temperature` (Option, adapter bỏ nếu model từ chối), `cache_hints` (các điểm cắt tĩnh/động), `timeout`, `request_tag` (workspace, purpose để log, không có nội dung) |
| `Message` | role `System/User/Assistant/Tool`; `parts`: `Text`, `ToolCall{id, name, args: Value}`, `ToolResult{call_id, content, is_error}`, `Opaque{provider, blob}` |
| `Opaque` | Giữ nguyên block mà provider yêu cầu gửi lại: thinking + signature của Anthropic (sửa/bỏ sẽ bị 400, `[V]`), reasoning item OpenAI, thought signature Gemini `[S]`. Lưu cùng message trong DB (cột jsonb) để hội thoại WarpBot nhiều lượt vẫn hợp lệ |
| `Completion` | `parts`, `stop_reason` (End, ToolUse, MaxTokens, Filtered, Other), `usage`, `provider_request_id` |
| `StreamEvent` | `TextDelta`, `ReasoningSummaryDelta`, `ToolCallStart{id,name}`, `ToolCallArgsDelta`, `ToolCallEnd`, `Usage`, `Done{stop_reason}`, `Error(LlmError)`. Lỗi giữa stream (Anthropic cho phép sau HTTP 200) đi vào kênh này |
| `Usage` | `input_uncached`, `cache_read`, `cache_write`, `output`, `reasoning`, `total` tính bởi adapter; cờ `estimated` khi provider không trả |
| `ModelCaps` | xem §5.3 |
| `EmbeddingProvider` (trait riêng) | `embed(texts) -> Vec<Vec<f32>>`, `profile() -> {provider, model, dims, normalize}` |

### 5.3 Capability flags (registry tĩnh trong code, ghi đè được theo workspace)

| Flag | Vì sao cần (nguồn) |
|---|---|
| `tools`, `parallel_tools` | Gate WarpBot; model không hỗ trợ tool thì WarpBot chỉ chat |
| `forced_tool_choice` | Sonnet 5.5/Opus 5.5 trả 400 (`[V]`) |
| `json_schema_strict`, `json_object_mode` | Anthropic chỉ có schema; schema giới hạn nên có bước "làm phẳng schema" và validate lại phía ta |
| `reasoning_control` (None/Effort/Budget), `reasoning_always_on` | Opus 5.5 không tắt được thinking, Sonnet 5.5 dùng `between_tools`, Haiku 5.5 tắt được (`[V]`) |
| `accepts_temperature` | Claude 4.7+ trả 400 nếu <1 (`[V]`) |
| `prompt_cache` (None/Implicit/Explicit), `cache_min_tokens` | Ngưỡng khác nhau (§1.2) |
| `streaming_tool_args` | Gemini trả function call nguyên chunk `[S]` |
| `max_context`, `max_output` | Cắt snapshot và `max_tokens` |
| `data_retention_floor_days` | Fable/Mythos 5.x = 30 (`[V]`), dùng cho chế độ `strict_zdr` |
| `price_in/out/cache_read/cache_write` | Ước lượng chi phí (§7), là bảng có version, chỉ để ước tính |
| model lạ (không có trong registry) | Caps bảo thủ (chỉ text, không tool) cộng probe lúc validate để nâng cấp |

### 5.4 Adapter

| Adapter | API gọi | Ghi chú |
|---|---|---|
| `openai` | Responses API cho assistant/reasoning; Chat Completions cho `openai_compatible` | Luôn `store=false`; đọc usage từ `response.completed`; gửi `prompt_cache_key = workspace_id` |
| `anthropic` | `/v1/messages` native | Đặt `cache_control` ở phần tĩnh; map `input_json_delta` thành `ToolCallArgsDelta`; giữ thinking block ở `Opaque`; không gửi `temperature`/`top_p` trên 4.7+ |
| `gemini` | `generateContent`/`streamGenerateContent` (AI Studio) hoặc Vertex (base_url + Bearer token do khách cấp) | Gửi lại thought signature; chuẩn hoá `functionResponse` id |
| `openai_compatible` | Chat Completions tại `base_url` | Azure OpenAI/gateway/vLLM: header và query tuỳ biến (`api-version`), caps bảo thủ |

Mỗi adapter có bộ test với mock HTTP ghi lại phản hồi thật (fixture), gồm: stream có tool call, lỗi giữa stream, 429 hai loại, usage có cache.

## 6. Cấu hình theo workspace

### 6.1 Schema đề xuất (module `assistant`, schema `assistant`; id v7, thời gian UTC, không FK chéo schema; bảng này cần được thêm vào `05a` trước khi viết migration)

`assistant.workspace_llm_configs` (một dòng mỗi workspace và mỗi mục đích, hoặc hợp nhất theo profile):

| Cột | Kiểu | Ghi chú |
|---|---|---|
| `id` | uuid | v7 |
| `workspace_id` | uuid | mọi truy vấn lọc theo cột này |
| `purpose` | text | `assistant`, `suggestion_decide`, `suggestion_generate`, `summary`, `embedding` |
| `provider` | text | `openai`, `anthropic`, `gemini`, `openai_compatible` |
| `model` | text | chuỗi tự do |
| `base_url` | text null | Azure/Vertex/gateway; kiểm tra SSRF (§6.4) |
| `extra_headers` | jsonb null | tên header cho phép, giá trị mã hoá nếu nhạy cảm |
| `credential_id` | uuid | tham chiếu `workspace_llm_credentials` (một key dùng cho nhiều purpose) |
| `reasoning` | text null | Off/Low/Medium/High |
| `max_output_tokens`, `timeout_ms` | int null | trần theo purpose |
| `data_boundary` | text | `external` hoặc `workspace_controlled` (§8) |
| `fallback_policy` | text | `none`, `platform_if_allowed` (§9) |
| `enabled`, `status`, `last_validated_at`, `last_error_code` | bool/text/timestamptz/text | `status`: `ok`, `degraded`, `invalid` |
| `version` | bigint | tăng mỗi lần sửa, dùng cho cache invalidation |

`assistant.workspace_llm_credentials`:

| Cột | Ghi chú |
|---|---|
| `id`, `workspace_id`, `provider` | |
| `ciphertext` (bytea), `nonce` (bytea) | AES-256-GCM của API key |
| `wrapped_dek` (bytea), `kek_id` (text) | DEK đã bọc bởi KEK |
| `key_hint` | 4 ký tự cuối, để hiển thị |
| `created_by`, `created_at`, `rotated_at`, `revoked_at` | audit |

`assistant.workspace_llm_budgets`: `workspace_id`, `period` (day/month), `max_input_tokens`, `max_output_tokens`, `max_est_cost_micro_usd`, `hard_stop bool`, `alert_threshold_pct`. Usage ghi vào `subscription.usage_records` của billing (§7), không nhân đôi.

### 6.2 Mã hoá key (envelope encryption)

| Bước | Quy tắc |
|---|---|
| KEK | Từ env qua `warptalk-config` (32 byte base64) hoặc KMS adapter (AWS KMS/GCP KMS) sau này; `kek_id` cho phép xoay KEK mà không giải mã lại dữ liệu (chỉ re-wrap DEK) |
| DEK | Sinh ngẫu nhiên cho mỗi credential; mã hoá key bằng AES-256-GCM, AAD = `workspace_id | provider | credential_id` để ciphertext không dùng được ở workspace khác |
| Giải mã | Chỉ trong process `api`/worker, bọc trong kiểu `Secret` (zeroize khi drop, `Debug` in ra `***`); không bao giờ ghi vào log, metric label, trace span, audit payload hay response (chỉ trả `key_hint`) |
| Thay/xoá | Ghi key mới tạo credential mới, `revoked_at` cho cái cũ; xoá workspace thì xoá cứng ciphertext |
| Quyền | Chỉ vai trò owner/admin workspace; mọi thao tác ghi audit (ai, khi nào, không kèm key) |
| Lộ key | Endpoint `revoke` ngay lập tức xoá cache và phát invalidation (§6.3) |
| Ghi chú | Cần `aes-gcm`, `zeroize`, `secrecy` được thêm vào workspace deps (§4) |

### 6.3 Thứ tự giải quyết và hot reload

Thứ tự: `workspace override (enabled, status=ok)` -> `platform default (env, như hiện nay)`. Nếu workspace chọn BYO nhưng cấu hình `invalid`, quyết định theo `fallback_policy` (§9), không âm thầm đổi sang nền tảng.

| Thành phần | Thiết kế |
|---|---|
| Cache trong process | `RwLock<HashMap<(workspace, purpose), Arc<ResolvedConfig>>>` với TTL 60 s (cùng nhịp cache `ai_policy` hiện có); đường suggestion theo từng segment chỉ đọc cache, không chạm DB |
| Invalidation | Khi ghi cấu hình/credential: cập nhật DB rồi `PUBLISH llm:cfg:invalidate {workspace_id, version}`; mọi replica xoá mục tương ứng và nạp lại ở lần dùng kế tiếp. Pub/sub mất tin thì TTL 60 s là lưới an toàn |
| Chính sách đồng ý | `external_llm_allowed`/`allow_external_llm` vẫn đi qua kênh hiện hữu (Redis `ai_policy`), không gộp vào config để giữ nguyên fail-closed |
| Không restart | Client `reqwest` dùng chung một pool; key/base_url đi theo từng request, không có client theo workspace trong bộ nhớ lâu dài |
| Trạng thái chạy | Circuit breaker theo `(config id)` lưu Redis `llm:cb:{id}`; backoff theo `retry-after` lưu `llm:cool:{id}` để các replica cùng tuân thủ |

### 6.4 Validate key (endpoint)

`POST /api/v1/assistant/llm-configs/validate` (hoặc `{id}/validate`), chỉ owner/admin:

| Bước | Hành vi |
|---|---|
| 1 | Chặn SSRF cho `base_url`: chỉ `https`, phân giải DNS rồi từ chối IP riêng/loopback/link-local/metadata (`169.254.169.254`), không theo redirect, allowlist host cho provider chuẩn |
| 2 | Gọi tối thiểu (1 prompt vài token, hoặc `count_tokens` nếu có) để kiểm tra auth và model tồn tại |
| 3 | Probe capability: một tool call giả, một JSON-schema nhỏ; ghi kết quả vào `ModelCaps` ghi đè của workspace |
| 4 | Trả `{ok, latency_ms, caps, warnings[]}`; cảnh báo nếu model có `data_retention_floor_days` > 0 hoặc là reasoning model có thể làm chậm decide (> timeout 8 s) |
| 5 | Lỗi map qua `AppError` thành `{ "code": "llm_auth_failed" ... }`; thông điệp provider chỉ log đã che, không trả key |

Rate limit endpoint validate (ví dụ 10 lần mỗi giờ mỗi workspace) để không bị dùng làm công cụ dò key.

### 6.5 Đổi embedding provider = re-index

Collection hiện tại `workspace_{id}` gắn với một `embedding_profile` (provider, model, dims, normalize) lưu cùng metadata collection. Đổi profile: tạo `workspace_{id}_v{n}`, re-embed từ chunk đã lưu (không quét bảo mật lại), kiểm tra chi phí ước tính cho admin xem trước, cutover nguyên tử (đổi con trỏ), xoá collection cũ sau thời gian chờ. Trong lúc re-index truy vấn vẫn đọc collection cũ. Lưu ý: `_semantic_search` truy vấn đồng thời `workspace_{id}` và `global_glossary` bằng một vector truy vấn (spec 14 §5.2), nên với profile khác nhau phải sinh hai vector truy vấn (một theo mỗi profile), nếu không điểm số vô nghĩa. Luật chặn index khi `external_llm_disabled_without_local_embedding_provider` (spec 14 §3.4) giữ nguyên.

## 7. Billing và giới hạn

| Vấn đề | Đề xuất | Lý do |
|---|---|---|
| Ai trả | BYO: khách trả trực tiếp provider; nền tảng không trừ credit cho token LLM của purpose đó. Embedding/translation/STT/TTS vẫn trừ credit như cũ | Tránh tính tiền hai lần; khớp kỳ vọng "tự mang key" |
| Cổng tính năng | Cờ entitlement `byo_llm_enabled` trong plan (`subscription.plans`, hoặc `workspace_entitlement_overrides`) | Tính năng có giá trị doanh nghiệp, kiểm soát rollout; không có cờ thì workspace không tạo được cấu hình |
| Ghi nhận | Vẫn ghi `subscription.usage_records` với `cost_source = byo`, `credits = 0` và token chi tiết (cache read/write) | Một nguồn sự thật về tiêu thụ; hiển thị cho khách; hỗ trợ đối soát |
| Chi phí | Ước tính từ `Usage` x bảng giá có version (§5.3), nhãn "ước tính", vì giá thật phụ thuộc hợp đồng của khách | Không giả vờ biết hoá đơn của họ |
| Ngân sách token | Bộ đếm Redis `llm:budget:{workspace}:{purpose}:{yyyymm|yyyymmdd}` (INCRBY theo token), đồng bộ định kỳ xuống DB; vượt `alert_threshold_pct` thì báo admin; vượt trần và `hard_stop` thì chặn, chuyển theo fallback (§9) | Khách cũng cần ngăn một meeting chạy mất kiểm soát trên key của họ |
| Giới hạn suggestion | Giữ nguyên `max_per_meeting`, `cooldown`, `min_confidence` (spec 09 §10); thêm trần đồng thời mỗi config (semaphore) để nhiều meeting song song không dồn vào một key | Rate limit của khách là tài nguyên chung của mọi room trong workspace |
| Thất bại billing | Ghi usage lỗi thì log và đếm metric, không chặn trả lời (cùng nguyên tắc "không fail vì phụ") | |

## 8. An toàn

| Lớp | Thiết kế |
|---|---|
| Prompt injection (độc lập provider) | Một lớp `PromptGuard` bao quanh `LlmProvider` (decorator), không nằm trong từng adapter: đóng khung nội dung không tin cậy (transcript, tài liệu, kết quả tool) bằng delimiter cố định, nhắc "chỉ là dữ liệu", loại ký tự điều khiển, lọc đầu ra (độ dài `max_suggestion_chars`, bỏ marker không do registry cấp). Luật spec 06 §6.3 và 09 §6: nội dung truy xuất không bao giờ là sự cho phép của người dùng |
| Cổng `external_llm_allowed` | Kiểm tra TRƯỚC khi văn bản rời process, fail-closed khi khoá thiếu/đọc lỗi (giữ hành vi spec 09 §8). Áp cho mọi lời gọi tới provider có `data_boundary = external`, bao gồm BYO. Cache 60 s cho suggestion (thu hồi có hiệu lực giữa meeting) |
| BYO có phải "external"? | Đề xuất: có, mặc định. Trường `data_boundary = workspace_controlled` (endpoint trong tenant của khách: Azure OpenAI riêng, Vertex/Bedrock của họ, gateway tự host) cho phép gọi dù `external_llm_allowed=false`, vì dữ liệu không rời ranh giới của workspace. Do admin tự khai nên cần xác nhận kiểu "tôi hiểu" và ghi audit. Quyết định sản phẩm mở (§11) |
| DLP | Decorator `DlpFilter` chạy trước provider: che email/điện thoại/thẻ bằng cùng bộ quét regex của security worker (`regex_scanners.py:56`, spec 09 §15 mục 8 là (proposal)), áp cho transcript live, tài liệu truy xuất, câu hỏi người dùng. Che trước khi gửi nên không tốn token. Workspace có thể bật `dlp_mode`: `mask` (mặc định với BYO external), `block`, `off` (chỉ khi `workspace_controlled`) |
| Chuỗi decorator | `FallbackProvider( BudgetGuard( ConsentGate( DlpFilter( PromptGuard( Adapter )))))`; thứ tự cố định, test thứ tự bằng test kiến trúc |
| Cổng xác nhận tool call (LI-14) | Nằm ở lớp ứng dụng của WarpBot, không ở adapter: mỗi tool có metadata `effect = read | write | external_write`; tool `write` tạo/sửa/chia sẻ/gửi/xoá dữ liệu bị chặn tại bước thực thi, trả `pending_confirmation{tool, args_hash}` cho UI; chỉ lượt xác nhận của người dùng (không phải nội dung do model sinh hay do tài liệu) mới mở khoá. Vì `strict` không được đảm bảo trên mọi provider (tương thích OpenAI của Anthropic bỏ qua `strict`, `[V]`), ta tự validate `args` theo JSON-schema của tool trước khi thực thi |
| Không tin model | Tool bắt buộc qua `tool_choice` không dùng cho logic an toàn (Sonnet/Opus 5.5 không hỗ trợ ép, `[V]`); an toàn dựa trên cổng thực thi, không dựa trên việc model "chịu" gọi hay không |
| Log | Không log prompt/response/args mặc định cho BYO; chỉ log `workspace_id`, `purpose`, `provider`, `model`, `usage`, `latency`, `provider_request_id`, mã lỗi chuẩn hoá |

## 9. Fallback khi BYO lỗi

| Tình huống | Hành vi mặc định | Ghi chú |
|---|---|---|
| Suggestion (decide/generate) lỗi hoặc quá 8 s | Im lặng như hiện nay, không retry, không đổi provider | Gợi ý trễ vô giá trị (spec 09 §13) |
| 429 có `retry-after` / 529 / 503 | Assistant: retry 1 lần (jitter, tôn trọng `retry-after` tối đa vài giây), sau đó fallback theo chính sách; suggestion: bỏ qua segment, đặt `llm:cool:{id}` | |
| 401/403, 402, hết quota/spend (kể cả 429 kèm `enforced_spend_limit_reached`, `credit_balance_exhausted`) | Không retry; đặt `status = degraded`/`invalid`, thông báo admin (module notification), mở circuit | Retry vô ích và có thể bị khoá thêm |
| 400 do capability sai (ví dụ `tool_choice` ép buộc) | Gỡ tính năng theo `ModelCaps` và thử lại một lần; ghi cảnh báo | |
| Lỗi nội dung/bị lọc | Trả thông báo người dùng; không fallback | Đổi provider không nên né bộ lọc |

`fallback_policy`:

| Giá trị | Nghĩa |
|---|---|
| `none` (mặc định) | BYO lỗi thì WarpBot báo "dịch vụ AI của workspace đang lỗi" và suggestion im lặng |
| `platform_if_allowed` | Chỉ sang provider nền tảng khi đồng thời: admin bật tường minh, `external_llm_allowed = true`, ngân sách credit còn, DLP vẫn áp dụng. Mỗi lần fallback ghi audit và hiển thị cho admin vì nhà xử lý dữ liệu đổi từ nhà cung cấp của khách sang nền tảng |

Fallback giữa hai BYO (`secondary_byo`) để sau; không làm ở bản đầu. Circuit breaker mở sau N lỗi liên tiếp trong cửa sổ, nửa mở bằng một probe mỗi 30 giây, để không đẩy mọi request vào provider đang chết.

## 10. Kế hoạch triển khai

| Pha | Nội dung | Tiêu chí xong |
|---|---|---|
| 0. Chuẩn bị | Chốt các câu hỏi §11; xin thêm workspace deps (§4); bổ sung bảng vào `05a` | Quyết định ghi lại |
| 1. Lõi | `warptalk-llm`: trait, mô hình chuẩn hoá, `ModelCaps`, adapter `openai` (Responses) và `openai_compatible`, parser SSE, mock HTTP test; chạy đường nền tảng hiện tại qua trait (không đổi hành vi) | Bộ test adapter xanh; WarpBot và suggestion dùng trait với provider nền tảng |
| 2. Anthropic | Adapter native, cache_control, `Opaque` thinking, ánh xạ lỗi | Test fixture stream/tool/usage cache |
| 3. Gemini | Adapter native AI Studio, sau đó Vertex qua `base_url` | Như trên |
| 4. Cấu hình | Migration, mã hoá envelope, repo, API CRUD + validate, cache + pub/sub invalidation, audit | Đổi key có hiệu lực dưới 5 s không restart |
| 5. An toàn và billing | Decorator Consent/DLP/PromptGuard/Budget, cổng LI-14, `usage_records` với `cost_source=byo`, ngân sách | Kiểm thử đối kháng: injection, key không lọt log, thu hồi consent giữa meeting |
| 6. Fallback | Circuit breaker, `platform_if_allowed`, thông báo admin | Chaos test: 429/529/timeout/mid-stream error |
| 7. Beta | Bật theo plan cho 1-3 workspace, chỉ purpose `suggestion_*` và `assistant`; embedding BYO để sau cùng | Theo dõi tỉ lệ lỗi, chi phí, độ trễ decide; sau đó mở rộng |
| 8. Embedding BYO | Profile embedding, re-index có xem trước chi phí, hai vector truy vấn | Cutover không mất truy vấn |

## 11. Chưa xác minh và quyết định mở

Chưa xác minh (`[?]` hoặc `[S]` trong tài liệu):

| # | Nội dung | Ghi chú |
|---|---|---|
| 1 | Toàn bộ trang OpenAI và Gemini chỉ qua snippet tìm kiếm (fetch bị chặn `platform.openai.com`, `developers.openai.com`, `openai.com`, `ai.google.dev`; cần allow trong network settings nếu muốn đọc đầy đủ) | Mọi số liệu giá, retention, caching, model của hai nhà này cần đọc lại trang gốc |
| 2 | Định dạng tool call OpenAI chi tiết (Responses vs Chat), `tool_choice` ép buộc của OpenAI và Gemini | Lấy từ kiến thức nền và hành vi upstream |
| 3 | Tên trường usage cache: OpenAI `cached_tokens`; Gemini `cachedContentTokenCount`; việc OpenAI `prompt_tokens` gồm token cache | |
| 4 | Gemini: `alt=sse`, header rate limit, `countTokens` và quota, giảm giá cache đọc, ngưỡng cache thật (tài liệu mâu thuẫn) | |
| 5 | OpenAI: endpoint đếm token trước khi gọi; tên model chính xác hiện hành cho decide/generate (GPT-6 Luna/Terra/Sol xuất hiện không nhất quán giữa các trang giá `[S]`) | |
| 6 | Gemini: giá và ID dòng Flash mới nhất; có tương thích OpenAI đủ cho caching/thought signature không | |
| 7 | Azure OpenAI: `api-version`, header auth, vùng/data zone (chỉ nguồn thứ cấp Microsoft Q&A) | |
| 8 | Bedrock: chi tiết ngoài việc `bedrock-runtime` không nhận `cache_control` top-level `[V]`; ZDR/retention của Bedrock và Vertex-managed do nhà cloud quy định | |
| 9 | Embedding Anthropic: không thấy model first-party; đối tác (Voyage) chưa kiểm tra | |
| 10 | Đánh giá các crate dựa trên metadata crates.io, chưa đọc mã nguồn hay thử `genai`/`rig`; chưa tìm thấy crate Anthropic chính thức | |
| 11 | Ước tính chi phí decide (150 lần x 5k token) là giả định | |

Quyết định sản phẩm cần chốt:

| # | Câu hỏi | Mặc định đề xuất |
|---|---|---|
| D1 | BYO có bị `external_llm_allowed=false` chặn không, và `workspace_controlled` có được miễn không | Chặn, trừ `workspace_controlled` có xác nhận và audit |
| D2 | Có giảm giá/phí nền tảng cho BYO không | Không tính credit token, vẫn tính phí tính năng qua plan |
| D3 | Có cho BYO purpose `embedding` ở bản đầu không | Không (pha 8) |
| D4 | Fallback sang nền tảng có mặc định bật không | Tắt |
| D5 | Lưu hội thoại WarpBot kèm block `Opaque` (có thể chứa reasoning của provider) có phù hợp chính sách dữ liệu không | Lưu mã hoá, TTL theo retention của hội thoại |
