import SwiftUI

struct ExternalMeetingView: View {
    @EnvironmentObject var app: AppState
    var body: some View { ExternalMeetingInner(session: ExternalSessionHolder.shared(app)) }
}

/// Keeps one ExternalSession alive across sidebar navigation.
@MainActor
enum ExternalSessionHolder {
    private static var instance: ExternalSession?
    static func shared(_ app: AppState) -> ExternalSession {
        if let i = instance { return i }
        let s = ExternalSession(app: app)
        instance = s
        return s
    }
}

private struct ExternalMeetingInner: View {
    @ObservedObject var session: ExternalSession
    @EnvironmentObject var app: AppState
    @EnvironmentObject var settings: SettingsStore
    @EnvironmentObject var captions: CaptionStore

    var body: some View {
        HStack(spacing: 0) {
            config.frame(width: 360)
            Divider()
            VStack(spacing: 0) {
                status
                Divider()
                CaptionsPanel()
            }
        }
        .navigationTitle("Họp bên ngoài")
    }

    private var config: some View {
        Form {
            Section("Cuộc họp") {
                TextField("Link hoặc mã (Meet/Zoom/Teams)", text: $session.linkText)
                    .onChange(of: session.linkText) { _, _ in session.linkChanged() }
                Picker("Nền tảng", selection: $session.provider) { ForEach(BridgeProviderKind.allCases) { Text($0.label).tag($0) } }
                Picker("Họ nói", selection: $session.farLanguage) { ForEach(Language.allCases) { Text($0.label).tag($0) } }
                Picker("Tôi đọc/nghe", selection: $settings.listenLanguage) { ForEach(Language.allCases) { Text($0.label).tag($0) } }
            }
            Section("Ứng dụng cần bắt âm thanh") {
                Picker("Ứng dụng", selection: $session.selectedTargetID) {
                    if session.targets.isEmpty { Text("Không thấy Chrome/Zoom/Teams/Safari đang chạy").tag(String?.none) }
                    ForEach(session.targets) { Text("\($0.name) (pid \($0.pid))").tag(Optional($0.id)) }
                }
                Button("Quét lại") { session.refreshTargets() }
                Picker("Cách bắt", selection: $session.backend) { ForEach(CaptureBackend.allCases) { Text($0.label).tag($0) } }
                Toggle("Tự chuyển sang ScreenCaptureKit nếu Tap lỗi", isOn: $session.autoFallbackToSCK)
            }
            Section("Chế độ âm thanh") {
                Picker("", selection: $session.audioMode) { ForEach(ExternalAudioMode.allCases) { Text($0.label).tag($0) } }
                    .pickerStyle(.radioGroup).labelsHidden()
                if session.audioMode == .voice {
                    Text("Tiếng dub sẽ phát vào: \(deviceLabel). Trong Meet/Zoom/Teams hãy chọn đúng thiết bị này làm Micro.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            Section {
                HStack {
                    if session.isActive {
                        Button("Dừng") { Task { await session.stop() } }
                        Button("Kết thúc phòng", role: .destructive) { Task { await session.endRoom() } }
                    } else {
                        Button("Bắt đầu") { Task { await session.start() } }.buttonStyle(.borderedProminent)
                        if session.phase == .needsTakeover {
                            Button("Giành quyền capturer") { Task { await session.start(takeOver: true) } }
                        }
                    }
                    Spacer()
                    Button("Overlay") { app.overlay.toggle() }
                }
                if case .failed(let m) = session.phase { Text(m).foregroundStyle(.red).font(.callout) }
                if session.phase == .needsTakeover { Text("Phòng đang có capturer khác (hoặc bạn vừa mất quyền).").font(.callout).foregroundStyle(.orange) }
            }
        }.formStyle(.grouped)
    }

    private var deviceLabel: String {
        settings.dubOutputDeviceUID.isEmpty ? "loa mặc định (chưa chọn BlackHole trong Cài đặt!)"
            : (DeviceManager.device(uid: settings.dubOutputDeviceUID)?.name ?? settings.dubOutputDeviceUID)
    }

    private var status: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(phaseText).font(.headline)
                Spacer()
                if let b = captions.latencyBadge { Label(b, systemImage: "speedometer").font(.caption.monospacedDigit()) }
            }
            HStack {
                Text("Mức âm").font(.caption)
                ProgressView(value: max(0, min(1, (session.levelDb + 80) / 80))).frame(width: 160)
                Text(String(format: "%.0f dBFS", session.levelDb)).font(.caption.monospacedDigit())
                Spacer()
                Text("Capture: \(session.captureName)").font(.caption).foregroundStyle(.secondary)
                if let hb = session.lastHeartbeat { Text("HB \(hb.formatted(date: .omitted, time: .standard))").font(.caption).foregroundStyle(.secondary) }
            }
            if let r = session.room { Text("Phòng \(r.roomCode) · \(r.title)").font(.caption).foregroundStyle(.secondary) }
            ScrollView { VStack(alignment: .leading) { ForEach(Array(session.statusLines.enumerated()), id: \.offset) { item in Text(item.element).font(.caption2.monospaced()) } }.frame(maxWidth: .infinity, alignment: .leading) }
                .frame(height: 90).background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 6))
        }.padding(12)
    }

    private var phaseText: String {
        switch session.phase {
        case .idle: return "Chưa chạy"
        case .starting: return "Đang khởi động…"
        case .running: return "Đang chạy · \(session.audioMode == .text ? "text-only" : "voice")"
        case .needsTakeover: return "Cần giành quyền capturer"
        case .failed: return "Lỗi"
        }
    }
}
