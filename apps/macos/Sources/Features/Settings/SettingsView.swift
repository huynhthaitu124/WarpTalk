import SwiftUI

/// Minimal native settings window (Cmd+,).
struct SettingsView: View {
    @EnvironmentObject var settings: SettingsStore
    @EnvironmentObject var web: WebViewModel
    @State private var outputs: [AudioDeviceInfo] = []
    @State private var inputs: [AudioDeviceInfo] = []

    var body: some View {
        Form {
            Section("Web app") {
                TextField("URL giao diện web", text: $settings.webURL)
                TextField("URL dự phòng (cách nhau bằng dấu phẩy)", text: $settings.webFallbackURLs)
                Text("Đang tải: \(settings.activeWebURL)").font(.caption).foregroundStyle(.secondary)
                HStack {
                    Button("Tải lại") { web.load() }
                    Button("Mặc định (Next.js dev)") {
                        settings.webURL = SettingsStore.defaultWebURL
                        settings.webFallbackURLs = SettingsStore.defaultWebFallbackURLs
                    }
                    Button("Dùng bản đóng gói") { settings.webURL = WebURLPlan.bundledURL; web.load() }
                        .disabled(!web.hasBundledWeb)
                        .help(web.hasBundledWeb ? "Web export nằm trong app (warptalk://app/)" : "App chưa bundle web export: chạy `make bundle`")
                }
            }
            Section("Backend") {
                TextField("URL API (không gồm /api/v1)", text: $settings.backendURL)
                Text("Bản đóng gói gọi API qua warptalk://app/api/... rồi chuyển tiếp tới URL này. Web gốc (http) dùng URL này khi không truyền backendUrl.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Âm thanh") {
                Picker("Đầu ra tiếng dub (voice mode, chọn BlackHole 2ch)", selection: $settings.dubOutputDeviceUID) {
                    Text("Mặc định hệ thống").tag("")
                    ForEach(outputs) { d in Text(DeviceManager.isVirtual(d) ? "\(d.name) (ảo)" : d.name).tag(d.uid) }
                }
                Button("Quét lại thiết bị") { outputs = DeviceManager.outputDevices(); inputs = DeviceManager.inputDevices() }
            }
            Section("Copilot (phiên họp)") {
                Picker("Ứng dụng họp mặc định (kênh \"Người khác\")", selection: $settings.meetingAppBundleId) {
                    ForEach(MeetingAppSpec.all, id: \.bundleIDs[0]) { spec in Text(spec.name).tag(spec.bundleIDs[0]) }
                }
                Picker("Micro (kênh \"Tôi\")", selection: $settings.micDeviceUID) {
                    Text("Mặc định hệ thống").tag("")
                    ForEach(inputs) { d in Text(d.name).tag(d.uid) }
                }
                Picker("Vị trí overlay", selection: $settings.overlayPosition) {
                    ForEach(OverlayPosition.allCases) { p in Text(p.label).tag(p) }
                }
                Toggle("Tự hiện overlay khi bắt đầu phiên", isOn: $settings.copilotAutoShowOverlay)
                Text("Web app có thể ghi đè ứng dụng họp và micro khi gọi startCopilotSession.").font(.caption).foregroundStyle(.secondary)
            }
            Section("Overlay phụ đề") {
                Toggle("Click xuyên qua", isOn: $settings.overlayClickThrough)
                Toggle("Ẩn khỏi chia sẻ màn hình", isOn: $settings.overlayHiddenFromScreenShare)
            }
        }
        .formStyle(.grouped)
        .frame(width: 560, height: 720)
        .onAppear { outputs = DeviceManager.outputDevices(); inputs = DeviceManager.inputDevices() }
    }
}
