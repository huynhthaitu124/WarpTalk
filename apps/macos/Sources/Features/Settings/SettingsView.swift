import SwiftUI

/// Minimal native settings window (Cmd+,).
struct SettingsView: View {
    @EnvironmentObject var settings: SettingsStore
    @EnvironmentObject var web: WebViewModel
    @State private var outputs: [AudioDeviceInfo] = []

    var body: some View {
        Form {
            Section("Web app") {
                TextField("URL giao diện web", text: $settings.webURL)
                HStack {
                    Button("Tải lại") { web.load() }
                    Button("Mặc định (dev)") { settings.webURL = SettingsStore.defaultWebURL }
                }
            }
            Section("Backend") {
                TextField("URL API (không gồm /api/v1)", text: $settings.backendURL)
                Text("Chỉ dùng nếu web app không truyền backendUrl khi bắt đầu capture.").font(.caption).foregroundStyle(.secondary)
            }
            Section("Âm thanh") {
                Picker("Đầu ra tiếng dub (voice mode, chọn BlackHole 2ch)", selection: $settings.dubOutputDeviceUID) {
                    Text("Mặc định hệ thống").tag("")
                    ForEach(outputs) { d in Text(DeviceManager.isVirtual(d) ? "\(d.name) (ảo)" : d.name).tag(d.uid) }
                }
                Button("Quét lại thiết bị") { outputs = DeviceManager.outputDevices() }
            }
            Section("Overlay phụ đề") {
                Toggle("Click xuyên qua", isOn: $settings.overlayClickThrough)
                Toggle("Ẩn khỏi chia sẻ màn hình", isOn: $settings.overlayHiddenFromScreenShare)
            }
        }
        .formStyle(.grouped)
        .frame(width: 520, height: 460)
        .onAppear { outputs = DeviceManager.outputDevices() }
    }
}
