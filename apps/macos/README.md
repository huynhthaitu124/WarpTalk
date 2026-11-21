# WarpTalk cho macOS (SwiftUI)

App native macOS 14.2+ (Swift 5.10, SwiftUI) cho WarpTalk: họp dịch trực tiếp (LiveKit) và **Họp bên ngoài** (Meet/Zoom/Teams) bằng cách bắt âm thanh app họp.
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

## 3. Trỏ app tới backend
Màn đăng nhập có mục "Server", hoặc **Cài đặt > Backend**. Mặc định `http://localhost:8080` (không gồm `/api/v1`, app tự thêm). Máy thứ hai dùng IP LAN của máy chạy backend, ví dụ `http://192.168.1.20:8080`. URL LiveKit lấy từ phản hồi `join`/`bridge-token` nên không cấu hình ở app.

## 4. Quyền (tab "Quyền")
| Quyền | Dùng cho | Ghi chú |
|---|---|---|
| Micro | họp WarpTalk; LiveKit publisher của bridge cũng mở micro (âm lượng mic bị đặt 0) | |
| Ghi âm thanh hệ thống (Audio Capture) | Process Tap | Không có API hỏi trạng thái; hộp thoại hiện lần đầu bấm Bắt đầu |
| Ghi màn hình | chỉ khi dùng ScreenCaptureKit dự phòng | cấp xong phải mở lại app |

Xử lý sự cố:
1. Thoát hẳn app (Cmd+Q) và mở lại sau khi đổi quyền.
2. Đặt lại: `tccutil reset Microphone com.example.warptalk`, tương tự `ScreenCapture`, `All`; rồi cấp lại.
3. Kiểm tra System Settings > Privacy & Security > Microphone / Screen & System Audio Recording.
4. Process Tap báo `noAudioProcess`: Chrome/Zoom/Teams chưa phát tiếng. Vào cuộc họp, bật loa rồi Bắt đầu lại.
5. Đổi bản build ad-hoc làm mất quyền: dùng `Local.xcconfig` như mục 2.

## 5. Họp bên ngoài (Meet text-only, đường demo chính)
1. Đăng nhập, chọn workspace, mở Chrome vào cuộc họp Meet.
2. Tab **Họp bên ngoài**: dán link Meet; "Họ nói" = ngôn ngữ phía Meet; "Tôi đọc" = ngôn ngữ của bạn; chọn Google Chrome; chế độ **Chỉ phụ đề**; bấm **Bắt đầu**.
3. Overlay phụ đề nổi lên (Cmd+Shift+O để bật/tắt). Mức âm phải nhảy khi Meet có người nói.

### Voice (D7) với BlackHole
```bash
brew install --cask blackhole-2ch    # cần khởi động lại coreaudiod/đăng xuất, dùng bản đã ký
```
1. Cài đặt > "Đầu ra tiếng dub" = BlackHole 2ch.
2. Trong Meet: Microphone = BlackHole 2ch (Loa giữ nguyên để nghe người khác).
3. Họp bên ngoài > chế độ Voice > Bắt đầu. Tiếng `dub-{lang}` phát vào BlackHole, Meet gửi đi.
Chống echo: Process Tap chỉ lấy tiến trình của app họp nên tiếng dub của WarpTalk không quay lại.

### Plan B nếu `mixer.capture(appAudio:)` không có trong bản LiveKit đã pin
Xem `Features/External/BridgePublisher.swift`: lặp audio capture vào **BlackHole 16ch** bằng AVAudioEngine rồi chọn thiết bị đó làm input của LiveKit (`AudioManager.shared.inputDevice`). Chưa cài đặt.

## 6. Test
`make test` (XCTest): giải mã JSON theo DTO Rust, parse caption + latency tracker, kích thước buffer của Resampler.

## 7. Cấu trúc
`Sources/App` (app, state) · `API` (client, Keychain, DTO) · `Audio` (`AudioCaptureSource`, `ProcessTapCapture`, `SCKAudioCapture`, `Resampler`, `DeviceManager`) · `Features/{Auth,Workspaces,Rooms,Meeting,External,Overlay,Permissions,Settings}` · `Tests`.
