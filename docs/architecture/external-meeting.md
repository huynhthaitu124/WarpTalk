# External Meeting (Google Meet / Teams / Zoom) – phân tích & kế hoạch

Trạng thái: **bản nháp v0.1** (2026-11-21). Phạm vi: backend Rust + app desktop macOS mới. Deadline demo: **thứ 2 2026-11-23**.

Nguồn code upstream (read-only): `warptalk-desktop` (Electron), `warptalk-web` (renderer chạy trong Electron, giữ toàn bộ logic LiveKit của bridge), `warptalk-backend`, `warptalk-ai`. Đường dẫn dưới đây tính từ thư mục upstream, ví dụ `desktop/` = `warptalk-desktop/src/main/`, `web/` = `warptalk-web/src/`.

Quy ước: **[V]** = đã xác minh từ tài liệu chính thức/search kết quả; **[U]** = chưa xác minh được (host `developers.google.com`, `developers.zoom.us`, `learn.microsoft.com` bị chặn khi fetch trực tiếp, chỉ đọc được snippet từ WebSearch; Apple Core Audio chỉ lấy được tiêu đề trang, nội dung là kiến thức chung).

---

## 0. Kết luận nhanh (đọc phần này nếu chỉ có 2 phút)

1. Bản gốc **không có bot**: máy của host là "capturer", bắt âm thanh Meet qua virtual audio device, publish lên LiveKit dưới identity cố định (stand-in). Toàn bộ pipeline bridge nằm ở **renderer web** (`web/lib/audio/bridge-*.ts`), Electron main chỉ lo phát hiện Meet, cài driver, popup, loopback Windows.
2. **macOS là công dân hạng hai** ở bản gốc: không có process loopback, không có chế độ text-only, caption sensor / mic-state / call-state đều chỉ có bản PowerShell (Windows). Mac bắt buộc cài 2 virtual device (HAL plug-in, cần admin + `killall coreaudiod`).
3. App macOS mới nên **bỏ virtual driver cho chiều vào** (far side): dùng **Core Audio process tap** (macOS 14.2+) hoặc **ScreenCaptureKit** (13+). Chiều ra (đưa tiếng dub vào Meet) **vẫn cần virtual mic**, trừ khi dùng bot. Đây là phần còn lại không né được.
4. Bot-based: **Meet Media API sẽ bị tắt 2027-01-15 [V]** và yêu cầu mọi participant enrol preview: loại khỏi kế hoạch. Teams cần bot Windows/.NET [V]. Zoom cần OBF/ZAK hoặc RTMS [V]. Nếu muốn bot đa nền tảng nhanh: **mua (Recall.ai)**, không tự xây trước thứ 2.
5. Backend Rust chưa có gì về bridge ngoài `bridge_token` (và token này thiếu kiểm tra loại room, xem B-R1). Cần module `external` + trait `ExternalSource` (mục 4).

---

## 1. Thiết kế hiện tại & bản đồ code path (bản gốc)

### 1.1 Luồng tổng thể

```
 Chrome (Meet) ──audio ra loa──▶ virtual speaker (BlackHole16ch / WarpTalk Speaker / Hi-Fi Cable)
                                         │ getUserMedia (web/lib/audio/bridge-audio-legs.ts:175)
                                         ▼
          renderer (warptalk-web trong Electron) ── LiveKit Room #2 (identity stand-in, publish-only,
                                         │          autoSubscribe:false, dtx:false, red:true)
                                         ▼                         web/lib/audio/bridge-inbound-connection.ts:107-113
                    LiveKit SFU ─▶ ingress (far_side_gate) ─▶ STT (far_side_dedupe, far_speaker) ─▶ MT ─▶ TTS
                                         ▲                                                              │
 Chrome (Meet) ◀── "mic" = virtual mic ◀─┴── playRemoteTrackToDevice(dub track, outboundSink) ◀── LiveKit Room #1 (host)
                                              web/lib/audio/bridge-audio-legs.ts:113-165
```

### 1.2 Bảng code path

| Chức năng | Nơi | Ghi chú |
|---|---|---|
| Phát hiện Meet (URL tab) | `desktop/meet-url-sensor.ts` (Win, UIA qua PowerShell), `meet-url-sensor-mac.ts` (Apple Events/JXA, poll 3 s, cần TCC Automation, `NSAppleEventsUsageDescription`) | mac: chỉ đọc active tab của Chrome/Arc/Brave/Edge/Vivaldi/Chromium/Safari; không biết PID (`capture-target.ts:42-43`) |
| Presence / call state / mic state | `meet-presence.ts`, `meet-call-state.ts` (1419 dòng), `meet-mic-state.ts` | UIA PowerShell (`meet-mic-state.ts:484`) → **Windows only** |
| Caption sensor (đọc caption Meet làm far-speaker hint) | `meet-caption-sensor.ts:250`, `meet-captions.ts`, `meet-caption-vocab.ts` | PowerShell → **Windows only**; hint đẩy vào Redis `meeting:{room}:far_speaker_hints` |
| Phát hiện driver | `desktop/virtual-audio.ts` (`detectVirtualAudio` ~686; mac `MAC_DEVICE_SETS` ~270; `describeMacVirtualAudio` ~780) | đọc `/Library/Audio/Plug-Ins/HAL` (:462); ready khi đủ 2 device |
| Cài driver mac | `virtual-audio.ts:874-893` (`buildMacDriverInstallScript`), `index.ts:1347-1428` (`osascript … with administrator privileges`), `scripts/build-mac-audio-driver.sh` (fork BlackHole GPL-3, ad-hoc sign mặc định, :22-24, :138-141) | `.pkg` postinstall cũng cài; fallback BlackHole upstream |
| Capability | `audio-runtime.ts:11-32` → `runtime:capability` | `virtualMicReady`, `audioDriverReady` |
| Win loopback | `windows-loopback-runtime.ts`, IPC `audio:start-capture` `index.ts:431-451` | `INCLUDE_TARGET_PROCESS_TREE`; mac không có (`virtual-audio.ts:805`: `textOnly: possible:false`) |
| Inbound (far side → LiveKit) | `web/lib/audio/bridge-inbound-connection.ts` (`openBridgeInbound`) | token từ `meetings/rooms/{id}/bridge-token` (chỉ host + room bridge) |
| Outbound (dub → virtual mic) | `web/lib/audio/bridge-audio-legs.ts:113` `playRemoteTrackToDevice` (AudioContext + `setSinkId`) | |
| Mic của host → Meet + ducking | `web/lib/meeting/meet-audio-track.ts` (mix mic, duck 0.15 khi far-side nói > −50 dBFS, hang 400 ms; :106-110) | echo path duy nhất có ducking |
| Capturer lease | `web/hooks/use-bridge-capturer-lease.ts`; BE `BridgeRoomsController` (`POST bridge/claim`, `capturer/heartbeat`, `capturer/takeover`, `PUT bridge/audio-mode`, `voice-clone-consents`); `BridgeRoomConstants`: heartbeat 15 s, stale 45 s | lease = "renderer còn sống", không phải "audio còn chảy" |
| Popup caption | `desktop/index.ts:1117-1243` (`openTranscriptWindow`, 460x620, `alwaysOnTop`) + `transcript-panel.ts` (ledger + navigator) | load URL web `TRANSCRIPT_ROUTE/{roomId}` |
| Cổng gate far-side | `ai/livekit_ingress_worker/far_side_gate.py` (spectral envelope, off mặc định), `ai/stt_worker/far_side_dedupe.py` (text, on) | cùng audio-time clock (`anchor_ms + start_ms`) |
| Far-speaker | `ai/shared/far_speaker.py` (hint → tên, `_NEAREST_HINT_MAX_CONFIDENCE`=0.5; sole-near 0.45–0.85), `far_speaker_late.py` (stream `stt:far_speaker_late:{room}`), BE `FarSpeakerRelabelWorker` (so với Google transcript) | ngưỡng hiển thị 0.6 |
| Kết thúc | BE `MeetConferenceEndWorker` (30 s, lease 2 phút, `conferenceRecords?filter=space.meeting_code`), `MeetConferenceEndPolicy.Decide`, `BridgeRecordingEndWorker` | cần host đã nối plugin google_meet + scope `meetings.space.readonly` |
| Roster / transcript Google | `assistant/.../GoogleMeetRestClient.cs` (`participants`, `transcripts`, `entries`), gRPC `MeetConferenceService` | Meet REST v2 |

### 1.3 Rust hiện tại

| Có | Chưa có |
|---|---|
| `providers/livekit` `bridge_token` (publish-only, cannot subscribe) `token.rs:205`; `meeting` `bridge_token` `service.rs:192`, `BRIDGE_IDENTITY="external-bridge"` `domain/mod.rs:7`; contracts có `far_speaker_*` `events.rs:48-56`; `transcript/ingest.rs:146-150` lọc theo `FAR_SPEAKER_MIN_CONFIDENCE` | loại room `EXTERNAL_BRIDGE`, claim/capturer lease/takeover, audio-mode, voice-clone consent, far-speaker hints stream, far-side gate/dedupe, conference-end worker, provider adapters |

---

## 2. Lỗi & rủi ro tìm thấy

Mức: **H** = chặn demo/mất dữ liệu, **M** = sai hành vi thấy được, **L** = nhỏ. "Upstream" = bản gốc, "Rust" = repo này. Mục có **[U]** là suy luận từ code, chưa chạy thật.

| ID | File:line | Mức | Vấn đề | Sửa đề xuất |
|---|---|---|---|---|
| B1 | `desktop/index.ts:1361-1364` | M | Cờ `audio-drivers-offered-<ver>` ghi **trước** khi user chọn: bấm "Not now" hoặc cài lỗi → không bao giờ hỏi lại cho đến bản sau. Mac mới không có device, bridge im lặng | Ghi cờ chỉ khi `started:true`; luôn có nút "Cài driver" trong Settings |
| B2 | `desktop/virtual-audio.ts:893` | M | `killall coreaudiod` khi cài **ngắt toàn bộ âm thanh hệ thống** 1-2 s, kể cả khi đang họp | Chặn cài khi `isMeetingActive()`; hoặc app mới không cần driver cho chiều vào |
| B3 | `desktop/virtual-audio.ts:138-141` (script `build-mac-audio-driver.sh`) | H [U] | HAL plug-in ký **ad-hoc** mặc định; macOS mới (Apple Silicon, Sequoia+) có thể từ chối nạp plug-in chưa notarize / Gatekeeper chặn pkg | Ký Developer ID + notarize (WT-674); demo thứ 2: dùng BlackHole upstream đã ký (`brew install --cask blackhole-2ch blackhole-16ch`, `virtual-audio.ts` `BLACKHOLE_BREW_COMMAND`) |
| B4 | `desktop/virtual-audio.ts:805` | H | macOS **không có text-only mode** (`process-loopback-unsupported`): user không cài driver thì không dùng được dù chỉ muốn caption | App mới: process tap/SCK cho text-only (mục 5) |
| B5 | `desktop/meet-caption-sensor.ts:250`, `meet-mic-state.ts:484`, `meet-call-state.ts` | H | Caption/mic-state/call-state **chỉ Windows** → mac không có far-speaker hint ⇒ mọi lời far side thành "Google Meet participants", dedupe/mute-aware ducking mất | Mac: Accessibility API (AXObserver) đọc caption DOM, hoặc Meet REST `participants` + diarization (WT-677); xem mục 4.3 |
| B6 | `web/lib/meeting/meet-audio-track.ts:106-110` | M | Ducking mic local xuống 0.15 khi far-side > −50 dBFS: nhiễu nền/echo loopback vượt −50 dBFS giữ mic bị nén, host không chen ngang được. Ngưỡng hard-code | Ngưỡng theo noise-floor thích nghi; cấu hình runtime; metric `duck_ratio` |
| B7 | `web/lib/audio/bridge-inbound-connection.ts:94-104` | M | Khi room #2 rớt (token hết hạn, SFU evict) chỉ gọi `onDisconnected`, **không tự reconnect**; stand-in im lặng trong khi capturer vẫn heartbeat | Auto-reconnect có backoff + refresh bridge-token; heartbeat gắn "audio liveness" (B8) |
| B8 | `use-bridge-capturer-lease.ts` + BE `CapturerStaleAfter` 45 s | H | Lease chỉ chứng minh renderer sống. Track audio chết/silent vẫn giữ lease ⇒ không ai takeover | Heartbeat kèm `rms_last_5s`, `track_state`; BE/ingress đánh stale khi stand-in silent > N s trong lúc Meet có người nói (so với roster) |
| B9 | `web/lib/audio/bridge-audio-legs.ts:175-181` | M | `getUserMedia` không ràng buộc `sampleRate`/`channelCount`; BlackHole 16ch ở 48 kHz/16 kênh dễ downmix sai, chỉ lấy kênh 0-1 [U]. Windows có handler format-mismatch, mac không | Ràng buộc `channelCount:2`, kiểm tra format sau khi mở; test với 16ch |
| B10 | `desktop/index.ts:1157` | M [U] | `alwaysOnTop:true` đơn thuần: trên macOS popup **không nổi trên Chrome fullscreen/Space khác** | `setAlwaysOnTop(true,"screen-saver")` + `setVisibleOnAllWorkspaces(true,{visibleOnFullScreen:true})`; hoặc NSPanel native |
| B11 | `ai/stt_worker/far_side_dedupe.py` (`min_chars` 8, `min_ratio` 0.8, `window` 15 s), `far_side_gate` off mặc định | M | Lặp câu ở nhiều ngôn ngữ (`same_language=True`) không bị dedupe; câu ngắn ("ok", "yes") lặp 2 lần không loại; gate frame off ⇒ STT/TTS tốn gấp đôi khi có WarpTalk user cùng Meet | Bật gate sau khi đo bằng `scripts/far_side_gate_eval`; Rust port: giữ fail-open (xem 4.2) |
| B12 | BE `MeetConferenceEndWorker.cs:29-30,92-106` | M | Mỗi tick (30 s) **tuần tự** gọi Google/ room (1 gRPC → ≥1 HTTP, nhiều trang) trong lease 2 phút; N room lớn ⇒ tick vượt lease, chạy chồng, dễ 429 | Song song có semaphore; backoff theo room; lease gia hạn; cache `conferenceRecords` 30 s theo meet code; ưu tiên Meet Media/Calendar push nếu có |
| B13 | `MeetConferenceEndPolicy.cs:46-58` | M | Không có grace: mọi người rời Meet 10 s rồi vào lại (cùng code) mà record mới chưa xuất hiện ⇒ `End` ngay, room bị kết thúc | `End` chỉ khi `latestEnd` cũ hơn `end_grace` (vd 2 phút) |
| B14 | `MeetConferenceEndWorker.cs:106` vs `:133` | L | Tra Google bằng `EffectiveHostId` nhưng end bằng `room.HostId` (khi host chuyển quyền, grant của host mới có thể thiếu scope ⇒ skip vĩnh viễn, log 1 lần) | Thử lần lượt các host/capturer có grant; fallback idle reaper đã có |
| B15 | `GoogleMeetRestClient.cs:40-46,55` | M | Regex `MeetCode` `^[a-z]{3,4}-[a-z]{3,4}-[a-z]{3,4}$` loại mã nickname/`lookup/…`, link Workspace custom; `space.meeting_code` chỉ thấy record nếu user là organizer/có quyền [U] ⇒ người tham gia thường nhận rỗng ⇒ `NoConference` mãi | Resolve qua `spaces.get` (name) khi có; hiển thị trạng thái "không theo dõi được Meet" cho user |
| B16 | `web/lib/audio/bridge-audio-legs.ts:58,120` | M | Yêu cầu `AudioContext.setSinkId`/`setSinkId` (Chromium); Safari không có ⇒ ném lỗi; Electron ổn nhưng mọi nhánh trình duyệt khác chết | App native (mục 5) dùng CoreAudio route trực tiếp |
| B17 | `desktop/index.ts:1669,1671-1677` | L | Allow `media` cho toàn origin trusted: renderer web được mic không prompt thêm; trên mac phụ thuộc TCC nhưng không có UI hướng dẫn khi bị từ chối (chỉ log, `:1587-1594` cho screen) | Màn Permissions có kiểm tra `getMediaAccessStatus` cho mic/screen/audio |
| B-R1 | Rust `meeting/src/application/service.rs:192-215` | H | `bridge_token` chỉ kiểm tra host (`host_session`), **không kiểm tra room là EXTERNAL_BRIDGE** (spec 11 §3.1 yêu cầu). Host bất kỳ tạo được seat publish-only ở room thường | Thêm `room_kind == EXTERNAL_BRIDGE` + `403/INVALID_STATE` |
| B-R2 | Rust cùng chỗ | M | Identity cố định `external-bridge` + mỗi lần gọi mint token mới: hai capturer/hai tab mint ⇒ LiveKit đá lẫn nhau (cùng identity) [U] | Chỉ cấp token cho capturer đang giữ lease; identity `ext:{provider}:{room}:{epoch}` hoặc `DeleteParticipant` rõ ràng |
| B-R3 | Rust (thiếu) | M | Không có `ai_service_suspended`/credit check riêng cho seat stand-in: seat không phải user nên usage gắn vào ai? (spec 10 §5.4) | Gắn usage vào capturer/host; test billing |

### Top 5 (đã chọn theo tác động lên demo thứ 2)

B-R1 (bảo mật, sửa 10 phút), B4 (mac không có text-only), B5 (mac không có far-speaker hint), B8 (lease không phản ánh audio), B3 (driver ad-hoc sign).

---

## 3. Phương án tích hợp theo nền tảng

### 3.1 Ba hướng

| Hướng | Mô tả | Ưu | Nhược |
|---|---|---|---|
| **A. Desktop capture** (bản gốc) | App trên máy user bắt âm thanh app họp, đưa tiếng dub vào virtual mic | Không cần admin Meet/Teams/Zoom; giống nhau cho mọi nền tảng; user chủ động; không thêm "participant" vào cuộc họp | Cần máy luôn bật; virtual mic bắt buộc để dub; far side chỉ là **mixed** audio; mỗi người một máy; echo/loop |
| **B. Bot tham gia** | Bot join như participant, nhận raw audio, gửi audio vào cuộc họp | Chạy server; nhận audio **riêng từng participant** (diarization miễn phí); không phụ thuộc máy user | Hiện diện bot (consent/lobby); Teams phải Windows/.NET; hạ tầng + chi phí; phê duyệt app |
| **C. Native API** | Meet Media API / Zoom RTMS / Graph | Chính thức, không bot lậu | Meet Media API sắp tắt, Teams không có RTMS tương đương, Zoom RTMS cần app approval + host bật |

### 3.2 Google Meet

| Hạng mục | Chi tiết | Trạng thái |
|---|---|---|
| Meet REST v2 (`conferenceRecords`, `participants`, `transcripts`/`entries`) | Dùng sẵn trong upstream để phát hiện kết thúc, roster, relabel sau họp; transcript chỉ có **sau** họp khi host bật transcript (Workspace cao cấp) | Đang dùng; scope `meetings.space.readonly` [V qua code]; hạn mức/quota [U] |
| Meet Media API | WebRTC nhận audio/video thời gian thực; scope audio `meetings.conference.media.audio.readonly` (restricted); admin Workspace phải bật; Cloud project, OAuth principal **và mọi participant** phải enrol Developer Preview; **Google thông báo tắt endpoint 2027-01-15** | [V] qua release notes/Recall blog (search snippet). **Không dùng** |
| Meet add-ons SDK | Side panel / main stage trong Meet; dùng để hiển thị caption/điều khiển, **không** cấp audio thô (kiến thức chung) | [U] |
| Calendar API | Tạo Meet (`conferenceData.createRequest`, `hangoutLink`) — đã có trong WarpBot tool (spec 04 §8) | Có |
| Bot (Recall / tự xây Chromium headless) | Tự xây = giả lập trình duyệt, vi phạm ToS rủi ro, dễ gãy; nên mua | Khuyến nghị mua nếu cần bot |
| Desktop capture | Như bản gốc; mac dùng process tap | Chọn cho demo |

Độ trễ thêm (desktop): Meet → Opus/jitter (≈100-300 ms, ngoài tầm kiểm soát) + capture 20-40 ms + WebRTC 100-150 ms; xem spec 08 §3.1 (tổng dub ≤ 2.0 s mục tiêu, thực tế 2.7 s).

### 3.3 Microsoft Teams

| Hạng mục | Chi tiết |
|---|---|
| Application-hosted media bot (Graph Communications + Real-time Media Platform) | Raw audio 16 kHz PCM theo frame, **unmixed audio theo speaker** [U chi tiết]; bot **C#/.NET, production trên Windows Server (Azure VM/VMSS, IP công cộng mỗi instance)** [V qua snippet Learn]; quyền Graph kiểu `Calls.AccessMedia.All` [U], cần tenant admin consent + application access policy |
| Graph online meetings + transcripts | `onlineMeetings/{id}/transcripts` sau họp; cần quyền `OnlineMeetingTranscript.Read.All`; real-time transcript qua change notification (preview) [U] |
| Teams app / side panel (meeting extension) | Hiển thị caption trong meeting; không lấy audio |
| Desktop capture | Process tap bắt app "Microsoft Teams" (mac: bundle `com.microsoft.teams2`); đưa dub qua virtual mic chọn trong Teams |
| Kết thúc cuộc họp | Graph `callRecords` / change notifications (cần bot hoặc subscription); desktop: theo dõi process/cửa sổ call + silence |
| Đánh giá | Bot Teams = dịch vụ riêng **Windows/.NET** nằm ngoài workspace Rust (sidecar gửi PCM qua gRPC/WebSocket). Không làm được trong 2 ngày |

### 3.4 Zoom

| Hạng mục | Chi tiết |
|---|---|
| Meeting SDK (Linux/Windows/macOS) bot | Raw data audio (PCM) per-participant; từ **2026-03-02** bot join meeting ngoài account phải có **ZAK hoặc OBF token** (hoặc dùng RTMS) [V]; OBF bot chỉ vào được khi owner đã trong meeting và phải rời khi owner rời [V] |
| RTMS (Realtime Media Streams) | **GA từ 2025-06-25** [V]; WebSocket hoặc RTMS SDK (Node/Python); audio per-participant hoặc mixed, mặc định L16 PCM 16 kHz mono [V]; không cần bot trong cuộc họp (thay thế bot) [V]. Yêu cầu app approval, host/admin bật, gói áp dụng [U] |
| Zoom Apps | Panel trong client Zoom (hiển thị caption); không audio thô [U] |
| Desktop capture | Process tap bắt `zoom.us`; virtual mic chọn trong Zoom |
| Đánh giá | **RTMS là lựa chọn bot-free tốt nhất** cho Zoom (chỉ nhận); chiều dub vào Zoom vẫn cần virtual mic hoặc bot gửi audio. Để sau demo |

### 3.5 Build vs buy (bot)

| Tiêu chí | Tự xây | Recall.ai (hoặc tương tự) |
|---|---|---|
| Phủ nền tảng | từng nền tảng một: Meet (headless browser), Teams (.NET Windows), Zoom (SDK/RTMS) | Zoom, Meet, Teams, Webex… một API [V] |
| Audio real-time | tự làm | WebSocket audio **riêng từng participant** [V] |
| Chi phí | hạ tầng + nhân lực lớn | pay-as-you-go **$0.50/giờ ghi hình**, +$0.15/giờ transcript [V snippet 2026]; latency real-time [U], phải tự đo |
| Gửi audio ngược vào cuộc họp | tự làm | có output audio/“speak” API [U] |
| Pháp lý | tự chịu | bot hiện diện trong participant list; DPA, lưu trữ dữ liệu bên thứ ba |
| Khuyến nghị | không | **dùng làm provider `bot` sau demo** (Phase 2), đặt sau trait `ExternalSource` |

### 3.6 Pháp lý / consent (mọi hướng)

| Vấn đề | Yêu cầu |
|---|---|
| Ghi âm/dịch người khác | Thông báo cho người tham gia (banner/chat message "cuộc họp đang được dịch bởi WarpTalk"); tuân luật hai bên đồng ý tại một số vùng |
| Voice clone người ở phía Meet (far speaker) | Spec 13 §6.2: consent theo cờ nền tảng; **audit trail chưa tìm thấy** (spec 13 mục 267) ⇒ mặc định **tắt clone far speaker** cho demo, chỉ giọng catalog |
| Dữ liệu | Transcript/lưu trữ theo workspace policy; gửi LLM ngoài theo `ai_policy` (spec 13 mục 274 proposal) |

### 3.7 Tóm tắt chọn lựa

| Nền tảng | Demo thứ 2 | Phase 2 | Phase 3 |
|---|---|---|---|
| Meet | Desktop capture (mac process tap + BlackHole cho dub) | Meet REST roster/end; add-on caption | bot (Recall) |
| Teams | Desktop capture cùng code path (đổi app target) | Graph transcripts sau họp | bot media Windows/.NET hoặc Recall |
| Zoom | Desktop capture cùng code path | RTMS provider (recv-only) | Meeting SDK bot / Recall |

---

## 4. Kiến trúc đề xuất: `ExternalSource` trong backend Rust

### 4.1 Vị trí & nguyên tắc

- Module mới `crates/modules/external` (schema `external`), tuân CLAUDE.md: `domain/` thuần, `application/` có port, `infrastructure/` adapter, `http/`. Không phụ thuộc module khác; `room` và `meeting` gọi qua port; `apps/api` nối adapter.
- Provider adapters nằm ở `crates/providers/{google_meet,msteams,zoom,recall}` (hoặc mở rộng `providers/google`). `ExternalSource` là **trait trong `application/ports.rs`** của module `external`.
- Room kind `EXTERNAL_BRIDGE` giữ trong `room`; `external` sở hữu session bridge (capturer, provider, meeting ref).

### 4.2 Trait (chữ ký ý niệm, không phải code cuối)

| Method | Trách nhiệm | Meet | Teams | Zoom |
|---|---|---|---|---|
| `provider()` | `meet`/`teams`/`zoom` | | | |
| `parse_meeting_ref(url) -> MeetingRef` | chuẩn hoá URL → khóa idempotency `(workspace, provider, native_id)` (một room mở/khóa, partial unique index như `translation_rooms_open_bridge_meet_code_key`) | meeting code (+ resolve nickname) | `threadId`/`joinWebUrl` + meeting id | meeting number + passcode |
| `bridge_identity(room, capturer, epoch)` | identity LiveKit của seat stand-in, publish-only, **một identity mỗi (room, epoch)** để token mới thay token cũ | `ext:meet:{room}:{epoch}` | | |
| `roster(ref) -> Vec<FarParticipant>` | danh sách người far side, `join_time` | `participants` REST | Graph participants / bot | Zoom participants / RTMS |
| `speaker_hints(ref) -> Stream<Hint{name,t_ms,source}>` | nguồn attribution (xem 4.3) | caption AX từ desktop; REST sau họp | bot unmixed (độ tin cậy 1.0) | RTMS per-participant |
| `conference_state(ref) -> {NotStarted, Live, Ended{at}}` | phát hiện kết thúc (có grace) | `conferenceRecords` | `callRecords`/bot state | webhook `meeting.ended` / RTMS stop |
| `post_meeting_transcript(ref, window)` | đối chiếu relabel (`FarSpeakerRelabelWorker`) | `transcripts/entries` | Graph transcripts | Zoom cloud transcript |
| `capabilities()` | `audio_in: mixed/per_speaker`, `audio_out: virtual_mic/bot/none`, `native_captions` | mixed/virtual_mic | per_speaker/bot | per_speaker/bot |

Mọi lỗi provider trả **in-band** (`unknown` ≠ `ended`), giống `MeetConferenceErrorCodes`; `thiserror` trong lib.

### 4.3 Bốn trách nhiệm cốt lõi

| Vấn đề | Thiết kế |
|---|---|
| **Bridge participant identity** | Hai loại source: (1) `mixed` (desktop capture): một stand-in `ext:{provider}:{room}:{epoch}`; (2) `per_speaker` (bot/RTMS): một identity mỗi far participant (`ext:{provider}:{room}:{participant_key}`), bỏ hẳn nhu cầu đoán người nói. Token chỉ cấp cho capturer giữ lease; `epoch` tăng khi takeover ⇒ token cũ bị vô hiệu |
| **Far-speaker attribution** | Pipeline giữ chuỗi ưu tiên: `per_speaker` (conf 1.0) > `caption_hint` (cửa sổ + lag, conf theo `far_speaker.py`) > `diarization` (sau) > "Participants". Giữ hằng số upstream: lag shift ~1 s, `NEAREST_MAX_CONF` 0.5, sole-near 0.45–0.85, ngưỡng hiển thị 0.6; **bắt buộc hiển thị nguồn** (`far_speaker_source`). Late-name stream sau ~1 s; relabel sau họp từ `post_meeting_transcript` (quyền host thắng). Đề xuất: lưu confidence và source ở mọi segment |
| **Duplicate suppression** | 3 lớp, mặc định **fail-open**: (1) *chặn tại nguồn*: nếu WarpTalk user cũng trong Meet và đã publish mic native ⇒ nhắn desktop "mute-in-Meet" hoặc dùng AEC tham chiếu; (2) *frame gate* (port `far_side_gate`: spectral envelope 16 band, lag tracking, second-voice hangover) — chỉ bật sau khi eval ≥ ngưỡng, cờ runtime theo workspace; (3) *text dedupe* (port `far_side_dedupe`: so khớp theo audio-time, ratio 0.8, min 8 ký tự, cả chiều FORWARD và LEAK, **ambiguous ⇒ không xoá**). Metric `bridge_dup_dropped_total{layer}` + `bridge_dup_ambiguous_total` |
| **Conference-end detection** | Máy trạng thái trong `external`: tín hiệu `provider_state`, `capturer_lease`, `stand_in_silence`, `participants_left`. Quy tắc: `End` khi (provider báo Ended **và** quá `end_grace`) **hoặc** (không có tín hiệu provider **và** stand-in silent + capturer mất > `idle_end`). Worker song song có semaphore, backoff theo room, lease gia hạn, cache theo meeting ref. Mọi "unknown" ⇒ không kết thúc. Hành động kết thúc đi qua use case `room::end` hiện có |

### 4.4 Capturer lease & API (giữ tương thích web hiện tại)

| Endpoint (relative, mount `/api/v1`) | Hành vi |
|---|---|
| `POST /translation-rooms/bridge/claim` | find-or-create theo `(workspace, provider, native_id)`; trả `bridgeRole`, `created`, `heartbeatIntervalSeconds`, `leaseSeconds` |
| `POST /{id}/bridge/capturer/heartbeat` | body mới (đề xuất): `{audio:{rms_dbfs, track_state, rtt_ms}}`; 409 khi mất lease |
| `POST /{id}/bridge/capturer/takeover` | ok khi lease stale **hoặc** stand-in silent quá ngưỡng (sửa B8) |
| `PUT /{id}/bridge/audio-mode` | `voice`/`text`, giữ `BRIDGE_AUDIO_MODE_LOCKED` |
| `POST /meetings/rooms/{id}/bridge-token` | thêm kiểm tra `EXTERNAL_BRIDGE` + chỉ capturer (B-R1/R2) |

Dữ liệu: bảng `external.bridge_sessions(id, room_id, workspace_id, provider, native_id, capturer_user_id, epoch, lease_at, state, ended_reason, created_at)` + unique partial index `(workspace_id, provider, native_id) WHERE state <> 'ended'`; theo `05a` khi có; chưa có trong data dictionary ⇒ cần cập nhật `05a` trước khi viết migration.

---

## 5. Yêu cầu app desktop macOS cho external meeting

### 5.1 Kiến trúc đề xuất

| Lớp | Lựa chọn | Lý do |
|---|---|---|
| Shell | **Swift/SwiftUI native** cho audio + permissions + overlay; UI còn lại (đăng nhập, danh sách room) có thể nhúng WebView của `warptalk-web` | CoreAudio tap/SCK/NSPanel là API native; Electron không gọi trực tiếp, và bản gốc đã phải thêm native addon cho Windows |
| LiveKit | LiveKit Swift SDK (publish track từ PCM tự cấp; subscribe track dub) | Bỏ phụ thuộc `getUserMedia` + virtual speaker cho chiều vào |
| Logic dùng chung | contract REST/WS đã mô tả ở 4.4 | Web/Electron cũ tiếp tục chạy |

### 5.2 Capture far side (chiều vào)

| Cách | Yêu cầu | Ưu | Nhược |
|---|---|---|---|
| **Core Audio process tap** (`AudioHardwareCreateProcessTap` + `CATapDescription` + aggregate device private) | macOS **14.2+** [U, kiến thức chung; trang Apple chỉ lấy được tiêu đề]; `NSAudioCaptureUsageDescription`; TCC "Audio Recording"/System Audio | Chọn **theo process** (Chrome, `zoom.us`, Teams) → không lẫn tab khác ngoài browser đó; không cần quyền Screen Recording; độ trễ thấp (IO proc) | Cần 14.2+; Chrome render audio qua helper process ⇒ phải tap cả process tree (lấy `AudioObjectID` của các process audio); mute behavior phải để `unmuted` |
| **ScreenCaptureKit** (`SCStream`, `capturesAudio`, `excludesCurrentProcessAudio`) | macOS **13+**; quyền **Screen Recording** | Hỗ trợ macOS 13; lọc theo application (`SCContentFilter`) | Quyền Screen Recording nặng nề (popup hằng tháng trên Sequoia [U]); độ trễ/buffer cao hơn tap [U] |
| Virtual device (bản gốc) | HAL plug-in, admin, `killall coreaudiod` | chạy mọi macOS | Rủi ro B1-B3, UX cài đặt, user phải đổi device trong Meet |

Khuyến nghị: **tap là mặc định (14.2+), SCK là fallback (13.x), virtual device chỉ dành cho chiều ra**. Hạn chế chung: Chrome gộp mọi tab chung process audio ⇒ tab khác ồn lẫn vào (giống R9 của Windows). Giải pháp: hướng dẫn dùng profile Chrome riêng cho cuộc họp, hoặc chỉ tap khi detect đúng tab Meet đang active và cảnh báo khi có audio ngoài Meet.

### 5.3 Phát tiếng dub vào cuộc họp (chiều ra)

| Cách | Mô tả | Ghi chú |
|---|---|---|
| **Virtual mic** (BlackHole 2ch hoặc "WarpTalk Microphone") | App phát track dub vào output của device; user chọn device làm Microphone trong Meet/Teams/Zoom | Duy nhất khả thi cho desktop capture. Demo thứ 2: dùng BlackHole upstream đã ký; **không** dùng bản ad-hoc (B3) |
| Mix mic thật + dub | Giữ ducking nhưng ngưỡng thích nghi (B6); có nút push-to-talk | Giữ `MEET_AUDIO_*` làm tham số mặc định |
| Không cần virtual mic | Chế độ **text-only** (caption overlay, user nghe tiếng gốc) | Chạy ngay với tap, không cài gì ⇒ **đường demo an toàn nhất** |
| Bot | Bot phát audio vào meeting | Phase 3 |

Chống echo: dub phát từ process của WarpTalk, tap loại trừ process WarpTalk (`excludes`), đảm bảo dub không quay lại stand-in (R1 của bản gốc).

### 5.4 Caption overlay

| Yêu cầu | Chi tiết |
|---|---|
| Cửa sổ | `NSPanel` nonactivating, level `.floating`/`.statusBar`, `collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]` để hiện trên Chrome fullscreen (sửa B10); click-through tuỳ chọn; resize/opacity |
| Nội dung | caption nguồn + dịch, tên speaker + dấu "?" khi conf < 0.6, độ trễ hiển thị; subscribe `TranscriptInterimReceived`/segment như web (spec 08 §2.5) |
| Hiệu năng | cập nhật diff, không re-render cả danh sách; ≤ 1 frame trễ so với sự kiện |
| Chia sẻ màn hình | `NSWindow.sharingType = .none` tuỳ chọn để overlay không lọt vào screen share |

### 5.5 Permissions UX (TCC)

| Quyền | Khi nào cần | Cách xin | Khi bị từ chối |
|---|---|---|---|
| Microphone | mic thật của host (ghép vào Meet) | `AVCaptureDevice.requestAccess` + `NSMicrophoneUsageDescription` | màn hướng dẫn mở System Settings > Privacy > Microphone |
| Audio capture (process tap) | far side | `NSAudioCaptureUsageDescription`, prompt xuất hiện lần start đầu | text-only không chạy; hiển thị trạng thái "chưa được cấp" |
| Screen Recording | chỉ khi dùng SCK fallback | `CGRequestScreenCaptureAccess` | thử tap hoặc hướng dẫn |
| Automation (Apple Events → browser) | phát hiện URL Meet (bản gốc) | prompt hệ thống lần đầu; -1743 = từ chối (`meet-url-sensor-mac.ts`) | cho phép dán link thủ công |
| Accessibility | đọc caption Meet (hint far-speaker) | `AXIsProcessTrustedWithOptions` | không có tên speaker, vẫn hoạt động |

Nguyên tắc: **màn Permissions một trang**, mỗi quyền có trạng thái (granted/denied/not-determined), nút "Cấp", deep-link System Settings; kiểm tra lại khi app active. Entitlements: hardened runtime + `com.apple.security.device.audio-input`; ký Developer ID + notarize.

### 5.6 Phát hiện cuộc họp & vòng đời

| Tín hiệu | Cách |
|---|---|
| Meet đang mở | Apple Events (như bản gốc) hoặc dán link; Teams/Zoom: kiểm tra process `zoom.us`/`MSTeams` có audio output (từ `kAudioProcessPropertyIsRunningOutput`) [U] |
| Bắt đầu | claim room (`bridge/claim`) → mở LiveKit #2 → publish PCM từ tap |
| Kết thúc | process audio im > `idle_end`, user bấm Stop, hoặc backend `ended` ⇒ dừng tap, rời LiveKit, đóng overlay |
| Phục hồi | reconnect LiveKit có backoff + token mới (B7); heartbeat kèm `rms_dbfs` (B8) |

---

## 6. Kế hoạch test

Mục tiêu NFR (spec 02): PF-01 STT partial 0.8–1.0 s; PF-02 STT final 1.5–2.0 s; PF-03 REST P95 < 1.0 s, median < 300 ms; PF-04 transcript/dịch text < 2.0 s từ hết câu; PF-05 speech-to-speech TTS chuẩn < 2.0–2.5 s; PF-06 clone < 3.0–3.5 s; PF-08 ≥ 1200 user đồng thời (chưa đạt). Ngân sách chi tiết spec 08 §3.1 (tổng 1710 ms, thực tế tác giả ~2.7 s).

### 6.1 Functional

| # | Case | Kỳ vọng | Tầng |
|---|---|---|---|
| F1 | Claim lần đầu cùng (workspace, provider, code) | tạo room, `created:true`, role capturer | API (Postgres) |
| F2 | Claim lần 2 từ user khác | cùng room, `created:false`, role member | API |
| F3 | Hai claim đồng thời | đúng 1 room (unique index), không 500 | API race |
| F4 | `bridge-token` bởi non-host / room thường / room đã ended | 403 / 409 INVALID_STATE (B-R1) | API |
| F5 | Heartbeat: capturer ok, non-capturer 409, sau 45 s stale takeover ok, trước đó 409 | theo bảng | API + fake clock |
| F6 | audio-mode voice→text ok; text→voice khi đang dịch | 409 `BRIDGE_AUDIO_MODE_LOCKED` | API |
| F7 | Meet code parse: `abc-defg-hij`, URL đầy đủ, có query, nickname, mã sai | đúng/ lỗi rõ ràng (B15) | unit |
| F8 | `ConferencePolicy`: NoConference, Live, EndedBeforeRoom, End, rejoin trong grace (B13) | bảng quyết định | unit property test |
| F9 | Attribution: hint trong cửa sổ, gần cửa sổ, hand-over A→B, sole-near, lag 0.5–1.5 s | tên + conf + source đúng; conf < 0.6 ⇒ ẩn tên | unit (port bảng từ `tests/test_far_speaker_hints.py`) |
| F10 | Dedupe FORWARD, LEAK, ambiguous, câu < 8 ký tự, khác ngôn ngữ | chỉ xoá khi chắc chắn, ambiguous giữ | unit (port `test_far_side_*`) |
| F11 | Text-only route: không TTS nhưng vẫn phát `final_chunk_processed` | theo spec 08 §2 | pipeline |
| F12 | Voice-clone consent far speaker mặc định tắt; rút consent ⇒ xoá clone | theo spec 13 §6.2 | API + pipeline |
| F13 | Room kết thúc ⇒ relabel job tạo, host thắng `google_transcript` | spec 12 | transcript |
| F14 (desktop) | Cấp/từ chối từng quyền TCC, đổi quyền lúc đang chạy | UI trạng thái đúng, không crash | thủ công + XCUITest |
| F15 (desktop) | Tap Chrome vs `zoom.us` vs Teams; tab ồn khác | chỉ audio process đích; cảnh báo tab ngoài | thủ công |

### 6.2 Failure injection

| Lỗi | Cách tiêm | Kỳ vọng |
|---|---|---|
| LiveKit #2 rớt / token hết hạn | `DeleteParticipant` / TTL 30 s | tự reconnect < 10 s, không mất lease, gap caption ghi nhận (B7) |
| Audio capture chết nhưng app sống | đóng tap giữa chừng | heartbeat báo `track_state=dead`, takeover mở (B8) |
| Meet REST 403/429/5xx/timeout | wiremock | trạng thái `unknown`, room không bị end; backoff; log 1 lần/lý do |
| Redis mất 10 s | dừng container | consumer reconnect, không mất segment (XAUTOCLAIM), không double-bill |
| STT/MT/TTS provider lỗi | fake provider trả lỗi/chậm | degrade theo ladder (text-only), spec 10 §3.7 |
| Hai capturer cùng lúc | hai tab claim + mint token | một thắng; cái kia bị kick sạch (B-R2) |
| Mạng desktop 5 % mất gói / 300 ms RTT | `dnctl`/Network Link Conditioner | dub vẫn phát, catch-up ≤ 1.25x |
| Sleep/wake máy, đổi output device, rút tai nghe | thủ công | tap/aggregate tạo lại, không im lặng vĩnh viễn |
| Clock skew desktop ↔ server | chỉnh giờ ±2 s | far-speaker hint vẫn đúng (dùng offset đo được) |

### 6.3 Performance & latency

| Chỉ số | Cách đo | Đạt |
|---|---|---|
| Stage latency (nội bộ) | histogram `warptalk_stage_latency_ms{stage}` đã có trong `shared/telemetry`; thêm stage `external_capture`, `external_publish`, `dedupe`, `attribution` | p50/p95 theo ngân sách spec 08 §3.1 |
| **Mouth-to-ear (thực)** | *Clap test*: máy A phát tiếng vỗ tay/tick (file WAV có impulse) vào virtual mic Meet; máy B ghi lại âm thanh dub bằng loopback; tìm offset giữa impulse nguồn và impulse bản dub (cross-correlation) | PF-05 < 2.0–2.5 s; PF-06 < 3.0–3.5 s; báo cáo p50/p95 qua ≥ 30 lần |
| *Loopback test một máy* | phát WAV (câu cố định) qua loa A, mic B ghi; đồng hồ chung (cùng máy ghi hai kênh: tín hiệu gốc từ line-out + dub từ capture) | sai số < 20 ms |
| Segment-level | `anchor_ms + start_ms` (audio-time) vs thời điểm TTS first-frame (`tts` publish stamp) | khớp thang spec 08 |
| Capture overhead (desktop) | Instruments (Time Profiler, System Trace): CPU %, wakeups; ngân sách CPU < 8 % một core ở 48 kHz | |
| **Load: N room tổng hợp** | `apps/pipeline` + harness: N room, mỗi room 1 stand-in + 1-3 speaker, audio WAV phát bằng LiveKit publisher giả (`livekit` rust SDK) thay cho Meet; provider STT/MT/TTS **fake** với độ trễ tuỳ chỉnh (phân phối log-normal theo spec 08) | N = 10 → 100 → 400 → 1200 (PF-08); theo dõi p95 stage, queue depth, CPU/RAM, lag consumer < 2 phút (PF-10) |
| Soak | 8 giờ, 50 room | không rò RAM/handle, không drift latency (B-R6 kiểu "latency hội tụ" từng gặp ở R6 bản gốc) |
| Control-plane | `k6`/`oha` vào claim/heartbeat: 1000 capturer × heartbeat 15 s ≈ 67 req/s | PF-03 |
| Meet-end worker | 500 room giả với wiremock Google | tick < 30 s, không 429 |

Công cụ: `cargo test` (unit/integration, `tower::ServiceExt::oneshot`), `criterion` cho dedupe/gate (băng thông frame), `tokio-test` fake clock, `wiremock` cho Google/Zoom/Graph, `k6`/`oha`, `tc netem`/Network Link Conditioner, Instruments, BlackHole + `sox`/`ffmpeg` để phát/ghi tuần tự, `ffmpeg` + script Python cross-correlation cho clap test. Integration test bỏ qua khi thiếu `DATABASE_URL`/`REDIS_URL` (CLAUDE.md).

### 6.4 Tiêu chí chấp nhận cho demo

| Tiêu chí | Ngưỡng |
|---|---|
| Caption nguồn xuất hiện sau khi nói xong | ≤ 2.0 s (PF-04) |
| Dub (voice catalog) nghe thấy trong Meet | ≤ 3.0 s mouth-to-ear (nới so với PF-05 vì đi qua Meet) |
| Không echo/loop (far side không nghe dịch lời mình) | 0 trường hợp trong 10 phút |
| Kết thúc Meet ⇒ room end | ≤ 2 phút sau khi Meet kết thúc |

---

## 7. Backlog ưu tiên (deadline thứ 2 2026-11-23, hôm nay thứ 7 2026-11-21)

### 7.1 Demo được trong 2 ngày

| # | Việc | Ước lượng | Ghi chú |
|---|---|---|---|
| D1 | **Sửa B-R1/B-R2**: `bridge_token` kiểm tra room bridge + capturer; unit test | 1-2 giờ | `modules/meeting` |
| D2 | `room`: loại `EXTERNAL_BRIDGE`, `bridge/claim`, heartbeat, takeover, audio-mode, unique index | 6-8 giờ | schema `translation_room`, theo spec 10 §3.8 |
| D3 | Contract: Redis stream `far_speaker_hints`, field `far_speaker_*` đã có; consumer đọc hint đưa vào STT (attribution phiên bản đơn giản: cửa sổ + lag 1 s) | 4-6 giờ | `pipeline/stt`, `shared/contracts` |
| D4 | Text dedupe (port `far_side_dedupe`, fail-open) | 3-4 giờ | pure fn, test port từ upstream |
| D5 | Conference-end: policy thuần + grace + worker (Meet REST qua `providers/google`), có thể **tắt** nếu chưa có OAuth | 4-6 giờ | `MeetConferencePolicy` có test |
| D6 | **Demo text-only trên mac**: app tối thiểu (Swift CLI hoặc SwiftUI nhỏ): process tap Chrome → resample 16 kHz → publish LiveKit stand-in; overlay caption (NSPanel) lấy từ WebSocket | 10-14 giờ (rủi ro: tap Chrome process tree, ký/notarize) | **Đường demo chính** |
| D7 | Demo voice (nếu còn thời gian): BlackHole 2ch (brew), phát dub qua `AVAudioEngine` vào BlackHole, hướng dẫn chọn mic trong Meet | 3-4 giờ | chỉ dùng BlackHole upstream đã ký |
| D8 | Bộ test: F1-F8 + F10 + clap test thủ công 1 lần, ghi số đo | 4-6 giờ | |
| D9 | Màn Permissions tối thiểu (mic + audio capture) | 2-3 giờ | |

Cắt nếu thiếu thời gian (theo thứ tự): D7, D5, D3 (hint) → vẫn demo được text-only với "Google Meet participants".

### 7.2 Sau demo (Phase 2, 1-3 tuần)

| # | Việc |
|---|---|
| P2-1 | Port `far_side_gate` (frame spectral) + eval harness; bật theo workspace |
| P2-2 | Accessibility-based caption reader trên mac (B5) |
| P2-3 | `ExternalSource` đầy đủ + adapter Meet REST (roster/transcript relabel) |
| P2-4 | Heartbeat có `audio liveness`, auto-reconnect, takeover khi silent (B7, B8) |
| P2-5 | Load test N room với provider giả; dashboard Grafana theo stage |
| P2-6 | Teams/Zoom qua desktop capture (đổi app target, test từng app) |
| P2-7 | Ký Developer ID + notarize app; driver "WarpTalk Microphone" nếu quyết định giữ |
| P2-8 | Voice-clone far-speaker với consent + audit trail (khoảng trống spec 13) |

### 7.3 Sau đó (Phase 3)

| # | Việc |
|---|---|
| P3-1 | Zoom RTMS provider (recv-only, per-speaker) |
| P3-2 | Provider `bot` qua Recall.ai (Meet/Teams/Zoom), per-speaker identity, gửi audio ngược |
| P3-3 | Teams media bot tự xây (Windows/.NET sidecar) chỉ khi không dùng Recall |
| P3-4 | Meet add-on side panel hiển thị caption trong Meet |

### 7.4 Quyết định cần chốt ngay

| # | Câu hỏi | Mặc định đề xuất |
|---|---|---|
| Q1 | App mac mới native Swift hay tiếp tục Electron + native module? | Swift cho audio/overlay (D6), WebView cho phần còn lại |
| Q2 | Demo voice hay text-only? | Text-only làm chính, voice nếu còn giờ (D7) |
| Q3 | macOS tối thiểu | 14.2 (tap); 13.x chỉ qua SCK, làm sau |
| Q4 | Clone far speaker | Tắt |
| Q5 | Google OAuth `meetings.space.readonly` đã được duyệt cho project chưa? | Nếu chưa: dùng idle reaper, bỏ D5 khỏi demo |
