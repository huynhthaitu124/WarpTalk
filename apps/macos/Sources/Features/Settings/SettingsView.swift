import SwiftUI

struct SettingsView: View {
    @EnvironmentObject var settings: SettingsStore
    @State private var outputs: [AudioDeviceInfo] = []
    @State private var inputs: [AudioDeviceInfo] = []

    var body: some View {
        Form {
            Section("Backend") {
                TextField("URL (không gồm /api/v1)", text: $settings.backendURL)
                Button("Đặt lại mặc định") { settings.backendURL = SettingsStore.defaultBackendURL }
            }
            Section("Ngôn ngữ") {
                Picker("Tôi nói", selection: $settings.speakLanguage) { ForEach(Language.allCases) { Text($0.label).tag($0) } }
                Picker("Tôi nghe (dịch sang)", selection: $settings.listenLanguage) { ForEach(Language.allCases) { Text($0.label).tag($0) } }
                Toggle("Nghe cả giọng gốc (mặc định chỉ nghe track dub-\(settings.listenLanguage.rawValue))", isOn: $settings.hearOriginals)
            }
            Section("Thiết bị âm thanh") {
                devicePicker("Micro (họp WarpTalk)", selection: $settings.micDeviceUID, devices: inputs)
                devicePicker("Loa (họp WarpTalk)", selection: $settings.playbackDeviceUID, devices: outputs)
                devicePicker("Đầu ra tiếng dub (họp bên ngoài, chế độ voice) — chọn BlackHole 2ch", selection: $settings.dubOutputDeviceUID, devices: outputs)
                Button("Quét lại thiết bị") { reload() }
            }
            Section("Overlay phụ đề") {
                Toggle("Click xuyên qua (không bấm được overlay)", isOn: $settings.overlayClickThrough)
                Toggle("Ẩn overlay khỏi chia sẻ màn hình", isOn: $settings.overlayHiddenFromScreenShare)
            }
        }
        .formStyle(.grouped)
        .navigationTitle("Cài đặt")
        .onAppear { reload() }
    }

    private func reload() { outputs = DeviceManager.outputDevices(); inputs = DeviceManager.inputDevices() }

    private func devicePicker(_ title: String, selection: Binding<String>, devices: [AudioDeviceInfo]) -> some View {
        Picker(title, selection: selection) {
            Text("Mặc định hệ thống").tag("")
            ForEach(devices) { d in Text(DeviceManager.isVirtual(d) ? "\(d.name) (ảo)" : d.name).tag(d.uid) }
        }
    }
}
