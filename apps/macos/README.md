# WarpTalk cho macOS (SwiftUI)

App macOS 14.2+ (Swift 5.10) cho WarpTalk. **Giao diện sản phẩm là web app (`apps/web`) chạy trong WKWebView**; app native chỉ lo phần web không làm được: bắt âm thanh app họp (Process Tap / ScreenCaptureKit), publish lên LiveKit qua bridge token, phát tiếng dub vào thiết bị ảo (BlackHole), overlay phụ đề nổi và quyền macOS. Hợp đồng JS<->native: **`BRIDGE.md`**.
Thiết kế: `docs/architecture/external-meeting.md` (§5, §6, §7.1 D6/D7/D9).

> Code được viết trên Linux, **chưa được biên dịch lần nào**. Lần build đầu có thể có vài lỗi tên API (LiveKit SDK, Core Audio). Mọi chỗ không chắc đều có `// VERIFY:` — chạy `grep -rn "VERIFY" Sources` để xem danh sách.

## 1. Cài đặt và build

```bash
brew install xcodegen          # một lần
cd apps/macos
make gen                       # sinh WarpTalk.xcodeproj từ project.yml
open WarpTalk.xcodeproj        # hoặc: make open
```
Trong Xcode: chọn scheme **WarpTalk** > My Mac > Run (Cmd+R). Lần đầu Xcode sẽ tải Swift Package LiveKit (cần mạng).
Dòng lệnh: `make build` rồi `make run`; test: `make test`.

Yêu cầu: macOS 14.2+, Xcode 15.3+ (Swift 5.10).

## 2. Ký (signing)
- Mặc định ký ad-hoc (`CODE_SIGN_IDENTITY=-`): chạy được, nhưng **macOS có thể hỏi lại quyền Micro/Audio sau mỗi lần build** vì danh tính code đổi.
- Khuyến nghị: `cp Config/Local.xcconfig.example Config/Local.xcconfig`, điền `DEVELOPMENT_TEAM` (Apple ID miễn phí cũng được), chạy lại `make gen`. File này không commit.
- Bundle id là placeholder `com.example.warptalk`: đổi trong `project.yml` (`PRODUCT_BUNDLE_IDENTIFIER`) nếu cần.
- **App Sandbox đang TẮT (chỉ cho demo)** trong `Resources/WarpTalk.entitlements` để Process Tap và liệt kê thiết bị hoạt động không cần thêm entitlement. Info.plist cũng cho phép http/ws thường (`NSAllowsArbitraryLoads`) để nối backend dev. Cả hai phải sửa lại trước khi phát hành; ký Developer ID + notarize là việc sau demo.

## 3. Trỏ app tới web và backend
- Chạy web app trước (`cd apps/web && npm run dev`, mặc định http://localhost:5173), rồi mở WarpTalk.app. Nếu trang trắng/lỗi, bấm "Tải lại" (Cmd+R).
- Đổi URL: **WarpTalk > Settings (Cmd+,)**: "URL giao diện web" và "URL API" (không gồm `/api/v1`; chỉ dùng khi web không truyền `backendUrl`). Native bridge chỉ hoạt động với đúng origin này.
- URL LiveKit lấy từ phản hồi `join`/`bridge-token`.
- Xem web trong Safari > Develop > (tên máy) để debug.

## 4. Quyền
Web app hiển thị và yêu cầu quyền qua bridge (`permissions()`, `requestPermission()`, `openSystemSettings()`).
| Quyền | Dùng cho | Ghi chú |
|---|---|---|
| Micro | họp WarpTalk trong web view (getUserMedia); LiveKit publisher của bridge cũng mở micro (âm lượng mic bị đặt 0) | |
| Ghi âm thanh hệ thống (Audio Capture) | Process Tap | Không có API hỏi trạng thái; hộp thoại hiện lần đầu bắt đầu capture |
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
- **Voice (D7)** với BlackHole: `brew install --cask blackhole-2ch` (bản đã ký). Settings > "Đầu ra tiếng dub" = BlackHole 2ch (hoặc web truyền `dubOutputDeviceUid`). Trong Meet/Zoom/Teams đặt Microphone = BlackHole 2ch. Process Tap chỉ lấy tiến trình của app họp nên tiếng dub không quay lại.
- **Plan B** nếu `mixer.capture(appAudio:)` không có trong bản LiveKit đã pin: xem `Features/External/BridgePublisher.swift` (lặp audio vào BlackHole 16ch rồi chọn làm input của LiveKit). Chưa cài đặt.

## 6. Test
`make test` (XCTest): bridge protocol (parse, reply, origin policy, script), NativeAPI (JSON theo DTO Rust, ngày chrono), overlay model, AudioActivity, kích thước buffer Resampler. Phần web test bằng Playwright theo mục stub trong `BRIDGE.md`.

## 7. Cấu trúc
`Sources/App` · `Web` (WKWebView, `NativeBridge`, `BridgeProtocol`) · `API` (`NativeAPI`) · `Audio` (`AudioCaptureSource`, `ProcessTapCapture`, `SCKAudioCapture`, `Resampler`, `DeviceManager`) · `Features/External` (`NativeCaptureController`, `BridgePublisher`, `DubListener`, `VoiceOutputPlayer`) · `Features/Overlay` · `Features/Permissions` · `Features/Settings` · `Tests`.
