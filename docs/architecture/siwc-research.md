# Nghiên cứu: "Sign in with ChatGPT" (SIWC) plan usage cho WarpTalk

Ngày nghiên cứu: 2026-11-24. Người nghiên cứu: agent (Claude).

## 0. Phương pháp và mức độ tin cậy nguồn (đọc trước)

Môi trường nghiên cứu **chặn** `openai.com`, `developers.openai.com`, `help.openai.com`, `workos.com`, `thenewstack.io`, `dev.to`, `everpod.ai` (lỗi DNS/403 qua proxy). Vì vậy **không trang OpenAI nào được đọc trực tiếp bằng tay**. Nguồn thực sự đọc được:

| Loại nguồn | Cách đọc | Độ tin cậy |
|---|---|---|
| Repo chính thức `github.com/openai/sign-in-with-chatgpt-devkit` (README, `docs/security.md`, LICENSE, `packages/local/src/*.ts`) | `git clone` + đọc file | Cao (code + doc OpenAI viết). Commit mới nhất 2026-10-07. |
| GitHub issues của bên thứ ba có trích/tóm tắt docs & Terms (go-llmprovider-sdk #2, zvid #1331, learnquiz #1075/#1076/#1079, x-ai-auto-post #2, devkit #9) | WebFetch (qua mô hình tóm tắt) | Trung bình: là tóm tắt/trích của người dùng, có thể sai chữ; một số trích "Wayback snapshot" của Terms 29/09/2026. |
| Snippet tìm kiếm của các trang OpenAI (developers.openai.com/siwc/*, openai.com/policies/sign-in-with-chatgpt-terms, help.openai.com/en/articles/20001542, openai.com/index/devday-2026-recap) | Chỉ thấy snippet do công cụ tìm kiếm tóm tắt | Trung bình-thấp |
| Blog bên thứ ba (WorkOS, DEV, Apidog, eesel, ...) | Chỉ snippet | Thấp; nhiều chỗ mâu thuẫn |

Quy ước tag: **[VERIFIED]** = tôi đọc trực tiếp nội dung nguồn chính (code/doc trong repo OpenAI); **[VERIFIED-2ND]** = đọc được qua trích dẫn trong GitHub issue của bên thứ ba (nguồn gốc là trang OpenAI nhưng chưa tự đọc); **[SNIPPET]** = chỉ thấy trong kết quả tìm kiếm; **[INFERENCE]** = suy luận của tôi.

Việc cần làm lại khi có mạng: đọc trực tiếp `https://developers.openai.com/siwc/llms-full.txt`, `https://openai.com/policies/sign-in-with-chatgpt-terms/`, `https://developers.openai.com/siwc/request-client-id`, `https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations`, `https://help.openai.com/en/articles/20001542-using-your-chatgpt-plan-in-other-apps-and-sites`.

## 1. Kết luận ngắn

- **Không thể tự đăng ký và dùng ngay cho app thương mại.** Có hai "làn": (a) làn *open-source / chạy local* (tự phục vụ, không cần duyệt) và (b) làn *commercial / hosted* (chỉ "selected commercial partners", đăng ký qua interest form, có waitlist). WarpTalk (commercial, có web app + backend) thuộc làn (b); phần desktop macOS thương mại source-đóng nằm ở vùng xám giữa (a) và (b).
- **Backend của ta không được gọi OpenAI bằng token của user.** Terms: token chỉ được lưu local dưới quyền user; request phải xuất phát từ runtime local của user hoặc remote runtime chỉ user đó kiểm soát. Backend multi-tenant của WarpTalk không đáp ứng.
- **Không hỗ trợ audio/transcription/realtime** trong preview → không dùng được cho pipeline realtime translation (STT/TTS). Chỉ text/vision qua Responses API.
- **Điều khoản "No charge":** user phải dùng được plan qua SIWC mà không phải trả tiền cho app → xung đột với mô hình bán subscription gắn cứng với tính năng đó.
- Thực tế khả thi nhất: dùng SIWC (nếu OpenAI duyệt) chỉ cho tính năng AI assistant dạng text trong **desktop app**, gọi trực tiếp từ client; pipeline realtime vẫn dùng API key của WarpTalk/BYOK.

## 2. Trả lời từng câu hỏi

### 2.1 Tính đủ điều kiện của app

| Câu hỏi | Phát hiện | Nguồn | Tag |
|---|---|---|---|
| Ai đăng ký được? | "Sign in with ChatGPT is currently available to selected commercial partners through a limited trial." | trích trong zvid #1331 từ docs chính thức: https://github.com/lsegal/zvid/issues/1331 ; snippet trang "Request a client ID": https://developers.openai.com/siwc/request-client-id | VERIFIED-2ND + SNIPPET |
| Plan usage cho ai? | "ChatGPT plan usage is available to all open-source partners and selected private clients." | https://github.com/maccavelli/go-llmprovider-sdk/issues/2 (trích quickstart), https://github.com/lsegal/zvid/issues/1331 | VERIFIED-2ND |
| App hosted/trả phí | "Commercial or hosted apps: Sign-in is a limited trial. Request a client ID through OpenAI's interest form." Form: https://openai.com/form/sign-in-with-chatgpt-interest/ | https://github.com/lsegal/zvid/issues/1331 | VERIFIED-2ND |
| Làn open-source tự phục vụ | Không cần client secret/partner API key; app tự đăng ký động lần đăng nhập đầu: `client_id=dynamic_agent_client` + `agent_name_hint`, callback trả về `client_id` thật (dạng `oaiapp_...`) để dùng cho code exchange/refresh. Đoạn code SDK OpenAI: `client_id: previous?.clientId ?? "dynamic_agent_client"` | https://github.com/openai/sign-in-with-chatgpt-devkit/blob/main/packages/local/src/oauth.ts ; docs trích: go-llmprovider-sdk #2 | VERIFIED (code) |
| Redirect của làn này | Loopback `http://127.0.0.1:<port>/auth/callback`; sau đăng ký chỉ port được đổi; "Do not substitute with localhost" | oauth.ts (`host: "127.0.0.1"`, `CALLBACK_PATH="/auth/callback"`); docs trích ở go-llmprovider-sdk #2 | VERIFIED + VERIFIED-2ND |
| Quy trình duyệt, tiêu chí, doanh thu tối thiểu, xác minh domain/business, vùng, phí/revenue share | **Không tìm thấy bất kỳ tiêu chí công khai.** Một maintainer-issue ghi: "No published criteria, timeline, fee, or revenue share." Đây là lời của người dùng, không phải OpenAI. | https://github.com/lsegal/zvid/issues/1331 | VERIFIED-2ND (là *không có thông tin*; chưa chứng minh được OpenAI không có) |
| Lấy client_id commercial ở đâu | Qua interest form (không có đường dẫn dashboard tự phục vụ được tìm thấy). Trang docs "On your website": "Request an OAuth client first"; redirect URI phải khớp chính xác; PKCE S256 bắt buộc | snippets https://developers.openai.com/siwc/website , https://developers.openai.com/siwc/request-client-id | SNIPPET |
| Danh sách partner launch | OpenAI DevDay recap nêu Devin, Notion, Vercel, T3, OpenClaw, Dactyl trong 16 partner | https://openai.com/index/devday-2026-recap/ (snippet qua tổng hợp) | SNIPPET |
| License SDK mẫu | DevKit dùng "Sign-in with ChatGPT DevKit Noncommercial License v1.0": cấm dùng thương mại; "Commercial use of OpenAI's Contribution requires a separate written agreement with OpenAI." Giao thức OAuth/API là public (ta có thể tự viết client), nhưng **không được tái dùng code `@siwc/local`/`@siwc/react` trong sản phẩm thương mại** | https://github.com/openai/sign-in-with-chatgpt-devkit/blob/main/LICENSE | VERIFIED |
| README devkit | Nhắm "developers building open-source apps that run on a user's own machine" | https://github.com/openai/sign-in-with-chatgpt-devkit/blob/main/README.md | VERIFIED (qua WebFetch README) |

**Đánh giá (INFERENCE):** một app macOS thương mại source-đóng không phải "open-source partner"; nếu tự dùng đường dynamic registration mà không có thỏa thuận, có rủi ro vi phạm Terms/bị chặn. Phải xin phép OpenAI qua form.

### 2.2 Phía người dùng

| Mục | Phát hiện | Nguồn | Tag |
|---|---|---|---|
| Plan | Help Center: ai cũng đăng nhập (identity) được; plan usage tùy chọn trong app thương mại cần plan Go, Plus, Pro hoặc Business. Các blog khác nói chỉ Plus/Pro; có nguồn nói Business chưa được liệt kê → mâu thuẫn | https://help.openai.com/en/articles/20001542-using-your-chatgpt-plan-in-other-apps-and-sites (snippet); https://flaviocopes.com/sign-in-with-chatgpt/ ; https://www.eesel.ai/blog/sign-in-with-chatgpt | SNIPPET (mâu thuẫn) |
| Free | Identity sign-in được; plan usage cần plan trả phí | learnquiz #1076 https://github.com/cristianuibar/learnquiz/issues/1076 ; Help snippet | VERIFIED-2ND / SNIPPET |
| Enterprise/Edu | Không tìm thấy tuyên bố công khai rằng được phép. Mã lỗi `subscription_sharing_user_not_eligible`: "Token sharing is not available for this ChatGPT account or workspace." (user/workspace/policy eligibility) | errors.ts trong devkit; Help snippet | VERIFIED (mã lỗi); Enterprise/Edu: không rõ |
| Business | "select the intended Business workspace and choose to let the app use your ChatGPT plan" | Help snippet | SNIPPET |
| Vùng EEA/UK/CH | **Không tìm thấy loại trừ vùng riêng cho SIWC.** Có nhắc lỗi 403 liên quan vùng trong tóm tắt issue x-ai-auto-post #2 (không rõ nguồn chính). Các loại trừ EEA/UK/CH tìm được thuộc tính năng khác (ChatGPT Sites) | https://github.com/HARA-Akihiko/x-ai-auto-post/issues/2 | SNIPPET/không rõ |
| Hạn mức | Plus: giới hạn 5 giờ dùng chung cho mọi app dùng plan (kể cả Codex); Pro: không áp dụng. Người dùng đặt trần hàng tuần theo % allowance cho từng app (ChatGPT Settings > Usage, `https://chatgpt.com/settings/usage` — hằng `CHATGPT_USAGE_URL` trong SDK) | docs trích go-llmprovider-sdk #2; index.ts của devkit; Help snippet | VERIFIED (URL), VERIFIED-2ND (5h), SNIPPET (weekly %) |
| Consent | Identity và plan usage là hai quyền riêng; user có thể từ chối/tắt plan usage ở màn consent. SDK kiểm tra scope `chatgpt.tokens.use.direct` có trong token response, nếu không báo `sharing_not_enabled`: "You are signed in, but token sharing is disabled." | index.ts devkit | VERIFIED |
| Dữ liệu | Dùng plan không cấp cho app quyền đọc hội thoại, memory, file, billing, và không chia sẻ API key | Help snippet; learnquiz #1075 | SNIPPET / VERIFIED-2ND |
| Đăng xuất | Đăng xuất khỏi app không ngắt kết nối phía OpenAI; phải gỡ trong ChatGPT. SDK có gọi revoke | https://www.notebookcheck.net/ChatGPT-Plus-in-other-apps-signing-out-does-not-stop-the-usage.1411949.0.html ; oauth.ts | SNIPPET / VERIFIED (revoke) |
| Admin Business/Enterprise | Không tìm thấy tài liệu về admin control riêng cho SIWC | — | Không rõ |

### 2.3 Kỹ thuật

| Mục | Phát hiện | Nguồn | Tag |
|---|---|---|---|
| Discovery | `https://auth.openai.com/.well-known/openid-configuration` | oauth.ts | VERIFIED |
| Authorization URL | `https://auth.openai.com/api/accounts/authorize` | docs trích go-llmprovider-sdk #2 | VERIFIED-2ND (SDK lấy từ discovery) |
| Token URL | `https://auth.openai.com/api/accounts/oauth/token` | cùng nguồn | VERIFIED-2ND |
| Scopes | `openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`; `resource=https://api.openai.com/v1`. Identity-only chỉ cần `openid profile email` | oauth.ts dòng 9-11 | VERIFIED |
| PKCE / nonce / state | Authorization Code + PKCE S256 + OIDC nonce + state; ID token RS256 verify `iss/aud/exp/nonce/sub` | oauth.ts; security.md | VERIFIED |
| Client public vs confidential | Làn local: public client, không secret. Làn website: backend đổi code & verify ID token, không để secret/verifier trong JS; không rõ client secret có được cấp không | oauth.ts; learnquiz #1079 | VERIFIED (local) / VERIFIED-2ND |
| Desktop/native | Hỗ trợ qua loopback 127.0.0.1 + system browser; ví dụ Electron (macOS) "Paste Perfect". Không thấy custom URL scheme/deep link | README, security.md, oauth.ts | VERIFIED |
| Token lifetime | Access ~1 giờ; refresh 30 ngày, **xoay vòng mỗi lần refresh** (cấp refresh mới 30 ngày); phải tuần tự hóa refresh; các lỗi `refresh_token_reused/invalidated/expired` ⇒ đăng nhập lại | docs trích x-ai-auto-post #2, go-llmprovider-sdk #2; errors.ts | VERIFIED (mã lỗi), VERIFIED-2ND (thời hạn) |
| Lưu token | SDK bắt buộc mã hóa qua OS keychain (Electron `safeStorage`), không fallback plaintext | docs/security.md | VERIFIED |
| API gọi được | `POST https://api.openai.com/v1/responses` (header `Authorization: Bearer <access_token>`, không set org/project); `GET /v1/models` trả `models[]` (`slug`, `display_name`, `visibility=="list"`) theo catalog của tài khoản đó. Không dùng `backend-api` của ChatGPT | responses.ts, models.ts; docs trích go-llmprovider-sdk #2 | VERIFIED |
| Giới hạn preview | `store:false` và `stream:true` bắt buộc; `input` phải là mảng; cấm item role `system` (dùng `instructions`/developer); bỏ `previous_response_id` qua HTTP; không hỗ trợ `max_output_tokens`, `temperature`, `prompt_cache_retention`... (15 field); vision/file input được nếu model cho phép | go-llmprovider-sdk #2, x-ai-auto-post #2, learnquiz #1075; snippet https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations | VERIFIED-2ND |
| Không hỗ trợ | audio/video input, Files upload API, **transcription API**, image generation, file search, Code Interpreter, computer use, hosted MCP/connectors, tool_search | snippet preview-limitations; learnquiz #1076 | SNIPPET + VERIFIED-2ND |
| Chat Completions, Realtime API, TTS, embeddings | **Không có bằng chứng được hỗ trợ**; docs chỉ nêu Responses API. SDK trả `subscription_sharing_route_not_supported` ("This request uses a route that token sharing does not support.") và `..._unsupported_capability` | errors.ts | VERIFIED (mã lỗi); phần còn lại INFERENCE là không hỗ trợ |
| Model | Danh sách theo tài khoản qua `/v1/models`; tài liệu không nêu tên cố định. Issue devkit #9 báo dùng slug `gpt-6.1-sol`, `gpt-6-luna` (không chắc đại diện cho mọi tài khoản) | https://github.com/openai/sign-in-with-chatgpt-devkit/issues/9 | VERIFIED-2ND |
| Prompt caching | `cached_tokens=0` trên đường này (issue mở, OpenAI chưa phản hồi) | devkit issue #9 | VERIFIED-2ND |
| Streaming | Bắt buộc SSE; lỗi hết hạn mức có thể đến dưới dạng `response.failed` giữa stream | responses.ts; docs trích | VERIFIED |
| Server-side dùng token user | **Không** cho phép theo Terms (xem 2.4). Docs open-source: callback loopback, VM tự host nhận credential qua kênh an toàn như SSH | Terms §1, §2 trích go-llmprovider-sdk #2 | VERIFIED-2ND |
| Hết allowance | Lỗi `subscription_sharing_usage_limit_exceeded` (HTTP 429, không retry); `..._usage_unavailable` (retry được). "OpenAI does not silently switch the request to another billing path." App **tự** quyết fallback sang API key của mình (OpenAI không tự chuyển) → INFERENCE: fallback là quyết định của app, miễn đúng Terms | errors.ts; docs trích | VERIFIED (mã lỗi), VERIFIED-2ND (câu trích) |
| Rate limit số liệu, API quota | Không công bố; "no quota API" | unbiased-app PR #68 https://github.com/circuitandchisel/unbiased-app/pull/68 | VERIFIED-2ND |

### 2.4 Điều khoản (Sign in with ChatGPT Terms, bản 29/09/2026, cập nhật 04/11/2026 theo một nguồn)

Trang gốc `https://openai.com/policies/sign-in-with-chatgpt-terms/` không đọc trực tiếp được. Nội dung dưới đây lấy từ trích dẫn (qua Wayback) trong https://github.com/maccavelli/go-llmprovider-sdk/issues/2 — **VERIFIED-2ND**, cần đọc lại bản gốc.

| Chủ đề | Trích/ý | Hệ quả cho WarpTalk |
|---|---|---|
| Lấy token | "Obtain and use SIWC access and refresh tokens ('Authentication Tokens') only through OpenAI's supported sign-in flow" (§1) | Phải dùng flow chính thức |
| Lưu token | "Any persistent storage of Authentication Tokens must be local and under the user's control, not in a remote or managed environment." (§1) | **Cấm lưu refresh/access token trên DB/backend của ta** |
| Nơi phát sinh request | "Requests must originate from the user's local runtime or a remote runtime only that user controls." (§2) | **Backend dùng chung của ta không hợp lệ**; desktop app hợp lệ |
| Background | "Obtain express consent before background use." (§2) | Việc nền cần consent rõ ràng; không có cấm tuyệt đối nhưng phải gắn với đúng user |
| Kích hoạt chéo | "Another user's activity must not trigger requests to the authenticated user's account." | Cấm: người khác trong cuộc họp kích hoạt gọi bằng plan của host |
| Chỉ cho app đã kết nối | "Use the user's plan only for the application they connected." | Không dùng plan cho tính năng/app khác |
| Không API tổng quát | "Do not provide general-purpose API access for other tools or unrelated requests." | Không làm proxy |
| Thương mại | "No charge." (§2); bản tóm tắt: user phải dùng được plan qua SIWC mà không phải trả cho nhà cung cấp app hay nâng cấp | Không được paywall đường SIWC; vẫn có thể bán các tính năng khác (INFERENCE, cần xác nhận) |
| Cấm | "Pooling, transferring, reselling, gifting, or sharing ChatGPT plan usage or Authentication Tokens"; "Using one user's subscription to fulfill another user's requests." (§4); "Alter, reverse engineer, or otherwise amend the SIWC software." | Không pooling; không dùng plan của admin cho cả team |
| Branding | "Use your app's own name during sign-in and activation." (§1); "Use OpenAI names, logos, and buttons only as authorized." (§5); nút "Continue with ChatGPT" theo UI/UX Guidelines; không ngụ ý OpenAI bảo trợ | Tuân thủ |
| Dữ liệu/training, audit, chấm dứt | **Không có trong đoạn trích** | Chưa biết; hỏi OpenAI |
| Doanh thu/phí | Không thấy phí hoặc revenue share công bố | Hỏi OpenAI |

Tại thời điểm viết, vài nguồn bên thứ ba nói "formal developer terms for the identity service had not been published" — mâu thuẫn với việc Terms tồn tại (có thể là bản cũ); ưu tiên Terms chính thức (SNIPPET).

### 2.5 Chương trình tương tự ở Anthropic và Google

| Hãng | Phát hiện | Nguồn | Tag |
|---|---|---|---|
| Anthropic | Không có "Sign in with Claude" cho app bên thứ ba. Agent SDK docs: bên thứ ba không được cung cấp claude.ai login/rate limit cho sản phẩm của họ trừ khi Anthropic phê duyệt. Chặn OAuth subscription ở client bên thứ ba từ 2026-01-09; Anthropic nói end-user vẫn đăng nhập Claude Code bản gốc. Tình hình sau đó (credit Agent SDK 5-6/2026, tạm dừng 15/6) bị mô tả mâu thuẫn | https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan ; https://winbuzzer.com/2026/02/19/anthropic-bans-claude-subscription-oauth-in-third-party-apps-xcxwbn/ ; https://venturebeat.com/technology/anthropic-reinstates-openclaw-and-third-party-agent-usage-on-claude-subscriptions-with-a-catch | SNIPPET |
| Google | Không tìm thấy chương trình chính thức cho app bên thứ ba dùng gói Google AI Pro/Ultra. Gemini CLI personal OAuth bị ngừng (nguồn: 18/6/2026); "piggyback" OAuth của CLI bị coi vi phạm. Diễn đàn Google hỏi "is a sanctioned program planned?" chưa có đáp án chính thức | https://discuss.ai.google.dev/t/gemini-subscription-access-for-third-party-apps-is-a-sanctioned-program-planned/175577 ; https://github.com/google-gemini/gemini-cli/issues/29796 | SNIPPET |

Kết luận (INFERENCE): OpenAI là nhà cung cấp duy nhất có chương trình chính thức cho phép (có kiểm soát); Anthropic/Google nên dùng API key/BYOK.

### 2.6 Ví dụ thực tế (app độc lập, không phải ChatGPT app)

Danh sách từ thư mục partner của OpenAI được các site tổng hợp lại (chưa đọc thư mục gốc) — SNIPPET, https://www.eesel.ai/blog/sign-in-with-chatgpt , https://flaviocopes.com/sign-in-with-chatgpt/ , https://explainx.ai/blog/openai-sign-in-with-chatgpt-devday-2026 :

- Thương mại có plan usage: Amp Code, Conductor, Dactyl, Devin (Cognition), Hermes Agent, Hyperagent, Kilo Code, Lovable (coming soon), Notion, Vercel, Vorflux, Warp. Xác nhận bởi OpenAI recap (SNIPPET): Devin, Notion, Vercel, Dactyl. Warp: chỉ nguồn thứ ba (một issue còn cho tên Warp trong nhóm partner: zvid #1331, VERIFIED-2ND).
- Open-source: OpenClaw, OpenCode, Pi, T3.
- Chỉ sign-in (không plan usage): Airtable, Canva, GitLab, HubSpot, Supabase.
- Ghi chú: chủ yếu là công cụ lập trình/agent, **không có app dịch/họp realtime** trong danh sách đã thấy. Hầu hết đều là dev tool chạy local hoặc partner được OpenAI chọn.

## 3. Áp dụng cho WarpTalk

### 3.1 Có dùng được không?

| Tính năng | Dùng SIWC plan usage? | Lý do |
|---|---|---|
| Pipeline realtime (STT/dịch/TTS, audio) | **Không** | Transcription API, audio input, Realtime/TTS không được hỗ trợ (VERIFIED-2ND/SNIPPET); độ trễ + `stream:true` Responses không thay thế được pipeline audio |
| AI assistant / gợi ý văn bản trong **desktop app** | **Có thể, sau khi OpenAI duyệt**, gọi trực tiếp từ máy user tới `api.openai.com/v1/responses` | Đáp ứng "local runtime" & lưu token local bằng Keychain; Responses API stateless text/vision |
| AI assistant qua **backend** (web app, tóm tắt sau họp, job nền cho cả team) | **Không** | Cấm lưu token ngoài máy user và request từ backend multi-tenant; cấm dùng plan của user cho người khác |
| Web app (trình duyệt) | Chỉ identity sign-in (khả thi sau duyệt); plan usage từ trình duyệt là hướng một số team cân nhắc (learnquiz) nhưng chưa được OpenAI xác nhận cho hosted | INFERENCE |

### 3.2 Các bước cần làm

1. Gửi interest form https://openai.com/form/sign-in-with-chatgpt-interest/ nêu rõ: app thương mại, desktop macOS (local) + web, use case text assistant, token chỉ lưu Keychain, request từ client.
2. Đọc bản gốc Terms + docs `siwc` (hiện chưa đọc trực tiếp) và nhờ legal rà soát (đặc biệt "No charge", "general-purpose API", background).
3. Không dùng đường `dynamic_agent_client` trong production khi chưa có đồng ý bằng văn bản của OpenAI; không dùng code `@siwc/*` (license phi thương mại) — tự viết client theo giao thức public nếu được cho phép.
4. Thiết kế ports: `AssistantLlmProvider` có 2 adapter (SIWC-client-side, API key server/BYOK); backend chỉ nhận kết quả, không bao giờ nhận token. Fallback sang API key phải là lựa chọn rõ ràng của app/user (OpenAI không tự chuyển billing).
5. Xử lý các lỗi: `subscription_sharing_usage_limit_exceeded`, `..._user_not_eligible`, `invalid_grant`, `refresh_token_reused`; tuần tự hóa refresh.
6. Không bán SIWC như tính năng trả phí; giữ tính năng free tier cho đường này.

### 3.3 Rủi ro

| Rủi ro | Mức |
|---|---|
| Không được duyệt/waitlist dài (không có tiêu chí, thời gian) | Cao |
| Preview, spec đổi (nhiều field bị cấm, `store:false`, không caching) | Trung bình |
| Terms: "No charge", "chỉ cho app đã kết nối", cấm kích hoạt chéo trong cuộc họp nhiều người | Trung bình-cao |
| Trần Plus 5 giờ dùng chung với Codex và các app khác; user có thể cạn quota giữa cuộc họp | Trung bình |
| Business/Enterprise/Edu không rõ; EEA/UK/CH không rõ | Trung bình |
| Phụ thuộc OpenAI duy nhất; Anthropic/Google không có đường tương đương | Thấp-trung bình |
| Kết luận trong tài liệu này dựa chủ yếu trên trích dẫn thứ cấp vì không đọc được trang OpenAI | Cao (cần xác minh lại) |

### 3.4 Câu hỏi mở gửi OpenAI

1. App thương mại source-đóng chạy local trên macOS (không open-source) có đủ điều kiện "open-source partner / selected private client" không? Quy trình, tiêu chí, thời gian, phí/revenue share?
2. Có cấp client_id cố định (không dynamic registration) cho desktop native không? Có hỗ trợ redirect khác loopback (custom scheme/universal link)?
3. Dùng backend của chúng tôi như "remote runtime chỉ user đó kiểm soát" (mỗi user một sandbox riêng, token mã hóa bằng khóa của user) có được chấp nhận không? Hay hoàn toàn cấm?
4. "No charge": chúng tôi có được bán gói trả phí cho các tính năng khác của cùng app và vẫn dùng SIWC cho tính năng AI assistant không?
5. Hỗ trợ Realtime API, transcription (gpt-4o-transcribe), TTS qua plan usage ở lộ trình nào? Nếu không, có được dùng API key của chúng tôi song song trong cùng ứng dụng?
6. Cuộc họp nhiều người: gợi ý AI dựa trên lời nói của người khác gửi bằng plan của user A có vi phạm "Another user's activity must not trigger requests" không?
7. Job nền (tóm tắt sau họp) bằng plan của user: "express consent" cụ thể là gì; có giới hạn tần suất?
8. Plan được hỗ trợ chính xác (Go/Plus/Pro/Business/Enterprise/Edu), vùng (EEA/UK/CH), kiểm soát admin cho Business/Enterprise?
9. Rate limit/quota số liệu, API đọc quota còn lại, mã lỗi chính thức và hành vi giữa stream?
10. Điều khoản về dữ liệu/training, lưu log, audit, chấm dứt/thu hồi client, yêu cầu branding chi tiết.
11. Danh sách model chính thức được phép, và việc `store:false` + không caching có thay đổi lộ trình không?

## 4. Danh sách nguồn chính

- Repo chính thức OpenAI: https://github.com/openai/sign-in-with-chatgpt-devkit (README, `docs/security.md`, `LICENSE`, `packages/local/src/{oauth,responses,models,errors,index}.ts`) — VERIFIED
- Terms: https://openai.com/policies/sign-in-with-chatgpt-terms/ — không đọc trực tiếp (trích qua go-llmprovider-sdk #2)
- Docs: https://developers.openai.com/siwc , /siwc/website , /siwc/request-client-id , /siwc/token-sharing-open-source , /siwc/token-sharing-open-source/preview-limitations , /siwc/llms-full.txt — chỉ snippet
- Help Center: https://help.openai.com/en/articles/20001542-using-your-chatgpt-plan-in-other-apps-and-sites — snippet
- Cookbook: https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt — snippet
- Issues thứ ba: https://github.com/maccavelli/go-llmprovider-sdk/issues/2 ; https://github.com/lsegal/zvid/issues/1331 ; https://github.com/cristianuibar/learnquiz/issues/1075 , /1076 , /1079 ; https://github.com/HARA-Akihiko/x-ai-auto-post/issues/2 ; https://github.com/openai/sign-in-with-chatgpt-devkit/issues/9 ; https://github.com/circuitandchisel/unbiased-app/pull/68
- Blog/news (snippet): https://workos.com/blog/sign-in-with-chatgpt-plan-usage-scope ; https://thenewstack.io/sign-in-with-chatgpt/ ; https://www.eesel.ai/blog/sign-in-with-chatgpt ; https://flaviocopes.com/sign-in-with-chatgpt/
