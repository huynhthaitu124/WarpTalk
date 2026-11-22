# WarpTalk cho macOS (SwiftUI)

App macOS 14.2+ (Swift, language mode 5) cho WarpTalk. **Giao diện sản phẩm là web app (`apps/web`) chạy trong WKWebView**; app native chỉ lo phần web không làm được: bắt âm thanh app họp (Process Tap / ScreenCaptureKit), publish lên LiveKit qua bridge token, phát tiếng dub vào thiết bị ảo (BlackHole), overlay phụ đề nổi và quyền macOS. Hợp đồng JS<->native: **`BRIDGE.md`**.
Thiết kế: `docs/architecture/external-meeting.md` (§5, §6, §7.1 D6/D7/D9).

> Code được viết trên Linux, **chưa được biên dịch lần nào**. Tên API của LiveKit Swift SDK 2.19.0 và của Apple (Core Audio tap, ScreenCaptureKit, WebKit) đã được đối chiếu với source/header thật (mục 8), nhưng lần build đầu vẫn có thể còn vài lỗi nhỏ. Các điểm chưa kiểm chứng được đánh dấu `// VERIFY:` — chạy `grep -rn "VERIFY" Sources`.

## 1. Checklist build

Yêu cầu:
- macOS 14.5 trở lên để chạy Xcode 16.3+ (Xcode 26.x cũng được). App chạy trên macOS **14.2+**.
- **Xcode 16.3 trở lên** (toolchain Swift 6.1). Lý do: `Package.swift` của LiveKit **2.19.0** (bản đã pin, đã kiểm API) khai báo `swift-tools-version:6.1`. Code app vẫn biên dịch ở Swift language mode 5 (`SWIFT_VERSION = 5.0`).
- `brew install xcodegen` (một lần).
- Kết nối mạng lần đầu (Xcode tải LiveKit 2.19.0 và WebRTC xcframework, vài trăm MB).

```bash
cd apps/macos
make gen && make build         # sinh WarpTalk.xcodeproj rồi build Debug (web lấy từ URL trong Settings)
make run                       # mở app đã build
make test                      # XCTest
make open                      # hoặc mở Xcode: scheme WarpTalk > My Mac > Run (Cmd+R)
```
Bản nhúng sẵn web (không cần dev server): `make bundle` (chạy `npm run build` ở `apps/web`, yêu cầu Next.js `output: 'export'` -> `apps/web/out`, rồi build với `REQUIRE_WEB_BUNDLE=YES`) hoặc `make run-bundled`. Chi tiết mục 3.

## 2. Ký (signing)
- Mặc định ký ad-hoc (`CODE_SIGN_IDENTITY=-`): chạy được, nhưng **macOS có thể hỏi lại quyền Micro/Audio sau mỗi lần build** vì danh tính code đổi.
- Khuyến nghị: `cp Config/Local.xcconfig.example Config/Local.xcconfig`, điền `DEVELOPMENT_TEAM` (Apple ID miễn phí cũng được), chạy lại `make gen`. File này không commit.
- Bundle id là placeholder `com.example.warptalk`: đổi trong `project.yml` (`PRODUCT_BUNDLE_IDENTIFIER`) nếu cần (nhớ đổi lệnh `tccutil reset` bên dưới).
- **App Sandbox đang TẮT (chỉ cho demo)** trong `Resources/WarpTalk.entitlements` để Process Tap và liệt kê thiết bị hoạt động không cần thêm entitlement. Info.plist cũng cho phép http/ws thường (`NSAllowsArbitraryLoads`) để nối backend dev. Cả hai phải sửa lại trước khi phát hành; ký Developer ID + notarize là việc sau demo.
- `ENABLE_USER_SCRIPT_SANDBOXING = NO` (project.yml) vì build phase copy web đọc `../web/out` và ghi vào app bundle.

## 3. Web app: URL cấu hình hoặc bản nhúng
App tải web theo thứ tự, URL đầu tiên trả lời sẽ là origin duy nhất được dùng native bridge:
1. **URL trong Settings** (mặc định `http://localhost:3000`: Next.js dev, `cd apps/web && npm run dev`).
2. **Danh sách URL dự phòng** (mặc định `http://localhost:5173`, Vite cũ; nhiều URL cách nhau dấu phẩy). Chỉ chuyển sang URL kế tiếp khi lỗi kết nối (server chưa chạy), không chuyển khi người dùng huỷ điều hướng.
3. **Bản nhúng `warptalk://app/`** nếu app được build kèm web export.

Settings (Cmd+,) hiển thị URL đang tải; nút "Dùng bản đóng gói" chọn thẳng bản nhúng. Nếu trang trắng/lỗi: Cmd+R (Tải lại). Bản cũ lưu `http://localhost:5173` sẽ được tự đổi sang 3000 một lần.

**Bản nhúng (static export):**
- `Scripts/copy-web.sh` (post-build phase của target WarpTalk) rsync `apps/web/out` vào `WarpTalk.app/Contents/Resources/web`. Không có `out/index.html` thì chỉ ghi chú và bỏ qua (trừ khi `REQUIRE_WEB_BUNDLE=YES`). Đổi nguồn: `WEB_OUT_DIR=/duong/dan make build`.
- `WKURLSchemeHandler` (`Sources/Web/BundledWeb.swift`) phục vụ `warptalk://app/...`: file đúng tên; thư mục -> `index.html`; đường dẫn không đuôi -> `<path>.html`; còn lại không đuôi -> `index.html` (route phía client); có đuôi mà không thấy -> 404. Chặn `..`.
- **API base cấu hình được**: native inject `window.warptalkNative.config = { version, apiBaseUrl, bundled }` (xem `BRIDGE.md`). Bản nhúng: `apiBaseUrl = "warptalk://app"` và native **proxy** `warptalk://app/api/**` tới "URL API" trong Settings (không cần CORS cho scheme tuỳ biến; không hỗ trợ SSE/streaming vì response được gom đủ rồi mới trả). Chạy từ dev server: `apiBaseUrl` = "URL API" trong Settings. Web nên đọc giá trị này thay vì hard-code.
- Next export cần `output: 'export'`, `images.unoptimized: true` và không dùng route handler/middleware (việc của `apps/web`).
- Nếu `navigator.mediaDevices` là `undefined` trong `warptalk://app` (WebKit không coi scheme là secure context — chưa kiểm chứng), dùng URL `http://localhost:3000` (hoặc `npx serve apps/web/out -l 3000`) thay cho bản nhúng.
- Xem web trong Safari > Develop > (tên máy) để debug.

URL LiveKit lấy từ phản hồi `join`/`bridge-token`.

## 4. Quyền (TCC)
Web app hiển thị và yêu cầu quyền qua bridge (`permissions()`, `requestPermission()`, `openSystemSettings()`).
| Quyền | Dùng cho | Ghi chú |
|---|---|---|
| Micro | họp WarpTalk trong web view (getUserMedia) | Publisher của bridge **không** cần micro: LiveKit chạy ở chế độ manual rendering, PCM bắt được đưa thẳng vào mixer (`LiveKitAudio.swift`) |
| Ghi âm thanh hệ thống (Audio Capture) | Process Tap | Không có API công khai hỏi trạng thái; hộp thoại hiện lần đầu bắt đầu capture. Mở System Settings tại pane "Screen & System Audio Recording" (mục "System Audio Recording Only") |
| Ghi màn hình | chỉ khi dùng ScreenCaptureKit dự phòng | cấp xong phải mở lại app |

Sự cố:
1. Thoát hẳn app (Cmd+Q) và mở lại sau khi đổi quyền.
2. Đặt lại: `tccutil reset Microphone com.example.warptalk` (tương tự `ScreenCapture`, `All`), rồi cấp lại.
3. System Settings > Privacy & Security > Microphone / Screen & System Audio Recording.
4. `NO_AUDIO_PROCESS`: Chrome/Zoom/Teams chưa phát tiếng. Vào cuộc họp, bật loa rồi thử lại.
5. Build ad-hoc làm mất quyền mỗi lần: dùng `Local.xcconfig` (mục 2).

## 5. Họp bên ngoài
Web app gọi bridge: `listAudioApps` -> claim phòng (API) -> `startExternalCapture` -> mirror phụ đề bằng `pushOverlayCaption`. Chi tiết luồng trong `BRIDGE.md`.
- **Text-only**: mặc định. Overlay hiện phụ đề (Cmd+Shift+O bật/tắt).
- **Voice (D7)** với BlackHole: `brew install --cask blackhole-2ch` (bản đã ký). Settings > "Đầu ra tiếng dub" = BlackHole 2ch (hoặc web truyền `dubOutputDeviceUid`). Trong Meet/Zoom/Teams đặt Microphone = BlackHole 2ch. Process Tap chỉ lấy tiến trình của app họp nên tiếng dub không quay lại. Native nghe dub bằng kết nối riêng `clientTag: "bridge-listener"` (identity `{userId}:bridge-listener`, chỉ subscribe) và chỉ nhận track `dub-{lang}`.
- Dừng capture: native gọi `POST /translation-rooms/{id}/bridge/capturer/release` (trừ khi đã mất lease).
- **Publish PCM ngoài vào LiveKit**: đã xác minh trên SDK 2.19.0: `AudioManager.shared.setManualRenderingMode(true)` + `setMicrophone(enabled: true)` + `AudioManager.shared.mixer.capture(appAudio:)`. Không còn "Plan B" BlackHole 16ch.

## 6. Test
`make test` (XCTest, hầu hết là test thuần Swift): bridge protocol (parse, reply, origin policy kể cả `warptalk://app`, script + config), `NativeAPI` (request: Bearer, `clientTag`, heartbeat body, release; round-trip qua URLProtocol stub), lease/`HeartbeatFailure` theo `code`, `CaptionPacket` (các field `speakerName`, `speakerId`, `translations` cộng dồn, `sentenceIndex`, `isLastSentence`, `is_flash`), `WebURLPlan`, `BundledWebResolver` (thư mục tạm, chặn traversal), `APIProxy`, overlay model, `AudioActivity`, `Resampler` (kể cả downmix stereo). Phần web test bằng Playwright theo mục stub trong `BRIDGE.md`.

## 7. Checklist chạy lần đầu
1. `make gen && make build`; nếu lỗi biên dịch, sửa theo mục 8 (thường là tên API còn thiếu chắc chắn).
2. `cd apps/web && npm run dev` (cổng 3000), `make run`. Settings phải hiện "Đang tải: http://localhost:3000".
3. Trong DevTools (Safari > Develop): `window.warptalkNative` tồn tại, `warptalkNative.config.apiBaseUrl` đúng URL backend.
4. Chạy `getCapabilities()` + `listAudioApps()` (mở Chrome đang phát tiếng): thấy `com.google.Chrome`, `producingAudio: true`.
5. Claim phòng bridge ở web, `startExternalCapture`: hộp thoại "Audio Recording" hiện lần đầu; cấp quyền, bắt đầu lại nếu cần. Kỳ vọng: `state: "running"`, `audioFlowing: true`, `heartbeat` event có `leaseExpiresAt`, LiveKit có participant bridge đang publish `MICROPHONE`.
6. Voice mode: cài BlackHole, chọn thiết bị, `listenLanguage` đặt; log không có `set output device failed`; track `dub-{lang}` hiện ở `status.dubTrack`; tiếng dub nghe được qua thiết bị đã chọn (kiểm tra mục VERIFY "render ở manual rendering").
7. `stopExternalCapture`: status về `idle`, backend ghi nhận release (phòng không còn capturer).
8. `make bundle`, mở app, Settings > "Dùng bản đóng gói": trang tải từ `warptalk://app/`, gọi API thành công qua proxy.

## 8. Đối chiếu API và các mục còn lại

**Đã xác minh** (nguồn trong comment của code):
- LiveKit Swift **2.19.0** (`client-sdk-swift`, clone read-only): `Room(delegate:connectOptions:roomOptions:)` (`Core/Room.swift:277`), `connect(url:token:...)` (`:385`), `ConnectOptions(autoSubscribe:)`, `RoomOptions()`, `RoomDelegate` (mọi method `@objc optional`; `didSubscribeTrack`, `didPublishTrack`, `didReceiveData(... forTopic:encryptionType:)`, `didDisconnectWithError`), `remoteParticipants`, `Participant.audioTracks`, `RemoteTrackPublication.set(subscribed:)`/`isSubscribed`, `RemoteAudioTrack.add(audioRenderer:)`, `AudioRenderer.render(pcmBuffer:)`, `AudioManager.setManualRenderingMode`, `mixer.capture(appAudio:)`, `mixer.appVolume`. Pin trong `project.yml`: `exactVersion: 2.19.0`.
- Core Audio (header macOS 14.5 SDK + `insidegui/AudioCap`): `CATapDescription(stereoMixdownOfProcesses: [AudioObjectID])` (đã sửa: trước đây bọc `NSNumber`), `AudioHardwareCreateProcessTap` (macOS 14.2), `kAudioAggregateDeviceMainSubDeviceKey`, `kAudioHardwarePropertyProcessObjectList`, `kAudioProcessProperty{PID,BundleID,IsRunningOutput}`, `kAudioTapPropertyFormat`.
- ScreenCaptureKit (header 14.5): `capturesAudio`, `excludesCurrentProcessAudio`, `sampleRate`, `channelCount` (macOS 13), `SCContentFilter(display:including:exceptingWindows:)`, `SCStreamOutput`/`SCStreamDelegate`.
- WebKit (header 14.5): `WKScriptMessageHandlerWithReply` (reply `(Any?, String?)`), **đăng ký phải dùng `addScriptMessageHandlerWithReply:contentWorld:name:`** (code cũ gọi nhầm hàm của protocol khác nên sẽ không biên dịch; nay gọi selector qua ObjC runtime vì header không cho biết tên Swift), `WKUIDelegate requestMediaCapturePermissionFor` (macOS 12), `WKURLSchemeHandler`, `isInspectable` (13.3).
- AVFAudio: `AVAudioConverter.downmix` mặc định NO (đã sửa: stereo→mono nay trộn kênh thay vì chỉ lấy kênh trái); `AVAudioIONode.audioUnit` + `kAudioOutputUnitProperty_CurrentDevice` cho thiết bị đầu ra dub.
- System Settings: `Privacy_Microphone`, `Privacy_ScreenCapture` (`bvanpeski/SystemPreferences`); quyền system-audio nằm trong pane Screen & System Audio Recording nên `audioCapture` mở `Privacy_ScreenCapture`.
- Backend: `clientTag` `bridge-listener`/`INVALID_CLIENT_TAG`, heartbeat/release trả `BridgeStateResponse` (`leaseExpiresAt`, `serverTime`), 409 `CAPTURER_LEASE_LOST` khác `NOT_BRIDGE_ROOM`/`INVALID_STATE` (đọc từ `crates/modules/{meeting,room}`).

**Còn lại (`// VERIFY:`, cần máy Mac):**
1. `Sources/Features/External/DubListener.swift`: `AudioRenderer.render(pcmBuffer:)` có được gọi cho track remote khi bật manual rendering không (SDK ghi là có, không có test). Nếu không, voice mode im lặng: cần chuyển sang chế độ thiết bị + `RemoteAudioTrack.volume = 0`.
2. `Sources/Web/WebViewModel.swift`: `warptalk://` có là secure context cho `getUserMedia`/`crypto.subtle` không. Nếu không: dùng URL `http://localhost`.
3. `Sources/Web/BundledWeb.swift`: body của POST tới `warptalk://app/api/**` có tới được scheme handler không (đọc `httpBody` rồi `httpBodyStream`). Nếu không, web gọi thẳng backend (cần CORS cho origin `warptalk://app`).
Ngoài ra chưa kiểm được trên máy: build thực tế với Xcode, hành vi TCC của Process Tap, và `Privacy_ScreenCapture` có đưa thẳng tới mục System Audio Recording trên macOS 15.

## 9. Cấu trúc
`Sources/App` · `Web` (WKWebView, `NativeBridge`, `BridgeProtocol`, `WebURLPlan`, `BundledWeb` = scheme handler + proxy) · `API` (`NativeAPI`, `CaptionPacket`) · `Audio` (`AudioCaptureSource`, `ProcessTapCapture`, `SCKAudioCapture`, `Resampler`, `DeviceManager`) · `Features/External` (`NativeCaptureController`, `BridgePublisher`, `DubListener`, `LiveKitAudio`, `VoiceOutputPlayer`) · `Features/Overlay` · `Features/Permissions` · `Features/Settings` · `Scripts/copy-web.sh` · `Tests`.
