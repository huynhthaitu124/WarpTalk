import SwiftUI

struct PermissionsView: View {
    @EnvironmentObject var perms: PermissionsManager

    var body: some View {
        Form {
            Section("Quyền hệ thống") {
                row("Micro", "Cần cho họp WarpTalk (gửi giọng của bạn).", perms.microphone,
                    request: { Task { await perms.requestMicrophone() } }, pane: .microphone)
                row("Ghi âm thanh hệ thống (Audio Capture)", "Cần để lấy âm thanh Meet/Zoom/Teams bằng Process Tap. Hộp thoại hiện ra lần đầu bấm Bắt đầu ở tab Họp bên ngoài.",
                    perms.audioCapture, request: nil, pane: .audioCapture)
                row("Ghi màn hình (dự phòng)", "Chỉ cần nếu dùng ScreenCaptureKit thay cho Process Tap. Sau khi cấp phải khởi động lại app.",
                    perms.screenRecording, request: { perms.requestScreenRecording() }, pane: .screenRecording)
            }
            Section {
                Text("Nếu quyền hiện 'Bị từ chối' nhưng bạn đã bật: thoát hẳn WarpTalk rồi mở lại. Nếu vẫn lỗi, chạy `tccutil reset All com.example.warptalk` trong Terminal rồi cấp lại (xem README).")
                    .font(.callout).foregroundStyle(.secondary)
                Button("Kiểm tra lại") { perms.refresh() }
            }
        }
        .formStyle(.grouped)
        .navigationTitle("Quyền")
    }

    private func row(_ title: String, _ detail: String, _ status: PermissionStatus,
                     request: (() -> Void)?, pane: PermissionsManager.Pane) -> some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.headline)
                Text(detail).font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Text(status.rawValue)
                .font(.caption.bold())
                .padding(.horizontal, 8).padding(.vertical, 3)
                .background(color(status).opacity(0.2), in: Capsule())
                .foregroundStyle(color(status))
            if let request { Button("Cấp") { request() } }
            Button("Mở Cài đặt") { perms.open(pane) }
        }
    }

    private func color(_ s: PermissionStatus) -> Color {
        switch s { case .granted: return .green; case .denied: return .red; case .notDetermined: return .orange; case .unknown: return .gray }
    }
}
