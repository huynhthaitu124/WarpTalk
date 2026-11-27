# Meet bot tự xây (headless Chrome): biết ai đang nói

Trạng thái: **prototype** (`apps/meet-bot/bot.cjs` + channel bot trong audio WS). Thay cho hướng Recall.ai ở `external-meeting.md` §3.5 (quyết định 2026-11-27: tự xây).

## 1. Vì sao
Hiện tại chế độ External chỉ có 2 luồng: `me` là micro, `others` là toàn bộ tiếng phát ra từ app họp. Mọi người phía bên kia bị trộn vào một luồng, nên:
- không biết ai đang nói,
- hai người nói đè lên nhau thì STT nghe lẫn.

Bot vào thẳng cuộc họp sẽ giải quyết được cả hai việc này.

## 2. Ý tưởng chính
- Google Meet không gửi một luồng audio riêng cho từng người. Trình duyệt nhận **3 audio track WebRTC**, mỗi track mang tiếng của một trong 3 người nói to nhất lúc đó, và SFU tự đổi người theo thời gian.
  - Vì vậy hai người nói đè gần như luôn nằm ở **hai track khác nhau**, tức là hai luồng STT riêng và không bị lẫn.
- **Tên người nói** lấy từ giao diện Meet. Mỗi ô người tham gia có tên và biểu tượng "đang nói".
  - Bot đọc DOM khoảng mỗi 100 ms, rồi so mức âm lượng của từng track với danh sách ai đang có biểu tượng nói.
  - Track đang có tiếng được gán cho người đang có biểu tượng nói. Nếu nhập nhằng thì ghi "Speaker ?".

## 3. Thành phần

| Phần | Việc |
|---|---|
| `apps/meet-bot` (mới, Node 20 + TypeScript + Playwright) | Mỗi cuộc họp chạy một Chromium **có giao diện trên Xvfb**, vì Meet chặn headless. Bot vào link dưới tên "WarpTalk Bot", bấm "Ask to join" và chờ host cho vào. Trong trang, bot hook `RTCPeerConnection` để lấy từng audio track, chuyển sang PCM 16 kHz qua AudioWorklet và gửi lên API. Bot cũng đọc DOM để lấy danh sách người, ai đang nói, và biết khi nào bị kick hoặc cuộc họp kết thúc. |
| Docker | Service `meet-bot` trong profile `apps` (image có Chromium, Xvfb, PulseAudio null sink), nên `stack.sh up` chạy luôn. |
| API (`copilot` module) | Session thêm `capture = bot`. `POST /copilot/sessions/{id}/bot` để mời bot, `DELETE` để cho bot rời. Job được đẩy vào Redis stream `meetbot:jobs`. Bot báo trạng thái (`joining`, `waiting_admission`, `in_call`, `left`, `error`) qua `POST .../bot/status`. |
| Audio WS (mở rộng `audio_ws.rs`) | **Đã làm:** channel `bot0..bot2` (tag 2..4). Tên người nói đi theo từng utterance: `commit` có thêm trường `speaker` (tuỳ chọn), vì người trên một track đổi theo thời gian. Mỗi channel là một phiên STT riêng. Segment mang `far_speaker_name = tên thật`. `speaker_id` = uuid v5(room, tên), nên transcript, tóm tắt và gợi ý WarpBot sẽ thấy A, B, C. Bảng transcript đã có cột `far_speaker_name`, nên **không cần đổi schema transcript**. Copilot chỉ cần một migration nhỏ: `sessions.capture`, `sessions.bot_status`. |
| Web | Dialog "Add WarpBot" có thêm lựa chọn **"Bot vào cuộc họp"** bên cạnh "Thu từ máy này". Thêm chip trạng thái bot ("Đang chờ host cho vào…"). Caption và transcript hiện tên người nói. |
| Auth bot → API | Token dịch vụ ngắn hạn cho từng session (giống audio-ticket), cấp khi tạo job. Bot không giữ JWT của người dùng. |

## 4. Giới hạn và rủi ro (cần biết trước)
1. **Host phải bấm cho bot vào.** Nếu tổ chức Workspace chặn khách ẩn danh, bot cần tài khoản Google: anh tự đăng nhập một lần rồi bot dùng lại cookie lưu trong volume. Tôi không nhập mật khẩu hộ được.
2. **Meet đổi giao diện thì phần đọc DOM có thể gãy.** Selector được gom vào một file, kèm test e2e chạy thật để phát hiện sớm. Đây là chi phí bảo trì lâu dài.
3. **ToS của Google:** bot tự động hoá trình duyệt nằm ở vùng xám, dù cách này phổ biến (Otter, Fireflies cũng làm vậy). Bot hiện rõ trong danh sách người tham gia.
4. Nếu có nhiều hơn 3 người nói cùng lúc thì vẫn mất tiếng, vì Meet chỉ gửi 3 track.
5. **Giai đoạn 1 chưa có:** phát giọng dub ngược vào Meet (bot nói), Zoom và Teams.
6. Mỗi bot tốn khoảng 300–500 MB RAM và 0.5 CPU.

### Prototype (2026-11-27)
- `node apps/meet-bot/bot.cjs` (env `BOT_EMAIL`, `BOT_PASSWORD`, `API_URL`, tuỳ chọn `SESSION_ID`) gắn CDP vào Chrome bot (port 9333) **sau khi đã được cho vào**, tạo copilot session, lấy audio-ticket, mở audio WS.
- Trong trang: mỗi track `<audio>` → AudioContext 16 kHz → ScriptProcessor → PCM16; VAD theo RMS (`VAD_DB`, mặc định -48 dB, hangover 768 ms).
- Tên người nói = phiếu bầu nhiều nhất từ caption trong khoảng thời gian utterance; nếu không có caption mà chỉ còn 1 người khác trong phòng thì lấy người đó.

### Kết quả spike (2026-11-27)
- **Nguyên nhân bị chặn là CDP.** Khách ẩn danh mà quá trình vào phòng có CDP/Playwright điều khiển thì bị từ chối ngay ("You can't join this video call").
  - Mở Chrome thường, **không gắn CDP**, rồi nhập tên và bấm "Ask to join" bằng input cấp OS thì host thấy yêu cầu và cho vào bình thường.
  - Gắn CDP **sau khi đã được cho vào** thì không sao.
  - Thiết bị giả và tuổi profile không phải nguyên nhân. Không cần đăng nhập Google.
- Hướng production:
  - Chrome chạy trên Xvfb, `xdotool` gõ tên và bấm nút, chờ host cho vào (DOM có nút "Leave call"), rồi mới gắn CDP để lấy audio và DOM.
  - Vì không chạy init script trước khi vào phòng được, nên hook `RTCPeerConnection` (CSRC/SSRC) bị mất. Nếu cần lấy lại thì có hai cách:
    - extension content script `world: MAIN`, `run_at: document_start`. Chrome branded 137+ chặn `--load-extension`, nên phải dùng Chrome for Testing hoặc Chromium, và UA phải khớp đúng phiên bản thật;
    - chỉ dùng track của các thẻ `<audio>` (đang làm), không có CSRC.
- Dữ liệu đo trong cuộc họp (3 người: host, Fathom, bot; chỉ host nói):
  - Có 3 track `<audio>` cố định. Tiếng host chỉ nằm trên 1 track.
  - Caption region `[role=region][aria-label="Captions"]` có dòng tên người nói ("Tú Huỳnh"), rồi đến dòng text. Bấm `c` là bật caption.
  - Tên ô người tham gia = dòng đầu của `innerText` trong `[data-participant-id]`. Các `.notranslate` là icon (`mic_off`, `mic_none`).
- **Chưa kiểm chứng:** 2 người nói lần lượt và nói đè (cần người thứ hai vào họp), và class "đang nói" trên ô (với 1 người nói thì phép đo chưa có ý nghĩa).

## 5. Kế hoạch

| # | Bước | Xong khi |
|---|---|---|
| 1 | Spike: script Playwright vào một Meet thật, log 3 track + tên người đang nói | Thấy track ↔ tên khớp khi 2 người nói lần lượt và đè nhau (anh cần mở một Meet và cho bot vào) |
| 2 | `copilot`: migration, API mời/rời bot, job Redis, token dịch vụ, channel `bot:N` + nhãn speaker trong WS | `cargo test -p warptalk-copilot` xanh, có test cho đổi nhãn speaker |
| 3 | `apps/meet-bot`: worker nhận job, nhiều cuộc họp song song, reconnect, báo trạng thái, tự rời khi họp xong hoặc bị kick | Chạy được với Meet thật, transcript có A, B, C |
| 4 | Docker service + `stack.sh` | `stack.sh up` trên máy sạch có bot |
| 5 | Web: lựa chọn bot trong dialog, chip trạng thái, tên người trong caption | Thử end-to-end trong app |
| 6 | RUNBOOK + memory | |

Ước lượng: bước 1 nửa ngày, bước 2–5 khoảng 2–3 ngày làm việc của agent, cộng các lần anh cần vào Meet để test.
