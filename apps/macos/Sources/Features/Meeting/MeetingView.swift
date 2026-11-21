import SwiftUI

struct MeetingView: View {
    @ObservedObject var vm: MeetingViewModel
    @ObservedObject private var live: LiveKitSession
    @EnvironmentObject var app: AppState
    @EnvironmentObject var settings: SettingsStore
    @EnvironmentObject var captions: CaptionStore
    var onClose: () -> Void

    init(vm: MeetingViewModel, onClose: @escaping () -> Void) {
        _vm = ObservedObject(wrappedValue: vm)
        _live = ObservedObject(wrappedValue: vm.live)
        self.onClose = onClose
    }

    var body: some View {
        let room = vm.roomState ?? vm.room
        VStack(spacing: 0) {
            header(room)
            Divider()
            HStack(spacing: 0) {
                participantsPane.frame(width: 240)
                Divider()
                CaptionsPanel()
            }
        }
        .task { await vm.join(displayName: nil) }
        .alert("Lỗi", isPresented: Binding(get: { vm.error != nil }, set: { if !$0 { vm.error = nil } })) {
            Button("OK", role: .cancel) {}
        } message: { Text(vm.error ?? "") }
    }

    private func header(_ room: TranslationRoom) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading) {
                Text(room.title).font(.headline)
                Text("Mã \(room.roomCode) · \(room.status) · \(live.state)").font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if let badge = captions.latencyBadge {
                Label(badge, systemImage: "speedometer").font(.caption.monospacedDigit())
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(.quaternary, in: Capsule())
                    .help("Độ trễ từ lúc thấy câu đến khi có bản dịch cuối (đo tại máy này)")
            }
            Toggle(isOn: Binding(get: { live.micEnabled }, set: { v in Task { await live.setMic(v) } })) {
                Image(systemName: live.micEnabled ? "mic.fill" : "mic.slash.fill")
            }.toggleStyle(.button).help("Bật/tắt micro")
            Toggle("Nghe giọng gốc", isOn: $settings.hearOriginals)
                .onChange(of: settings.hearOriginals) { _, v in live.hearOriginals = v }
            Button("Overlay") { app.overlay.toggle() }
            if vm.isHost {
                if !room.translationActive { Button("Bắt đầu dịch") { Task { await vm.start() } }.buttonStyle(.borderedProminent) }
                Button("Kết thúc", role: .destructive) { Task { await vm.endMeeting(); onClose() } }
            }
            Button("Rời") { Task { await vm.leave(); onClose() } }
        }.padding(12)
    }

    private var participantsPane: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Người tham gia").font(.subheadline.bold())
            List(live.participants) { p in
                HStack {
                    Circle().fill(p.isSpeaking ? Color.green : Color.gray.opacity(0.3)).frame(width: 10, height: 10)
                    Text(p.isLocal ? "\(p.name) (tôi)" : p.name)
                    Spacer()
                    Image(systemName: p.micOn ? "mic.fill" : "mic.slash").foregroundStyle(.secondary).font(.caption)
                }
            }
            Divider()
            Text("Âm thanh dịch").font(.subheadline.bold())
            Text("Đang nghe: \(live.subscribedDub ?? "chưa có track dub-\(settings.listenLanguage.rawValue)")").font(.caption)
            if !live.availableDubTracks.isEmpty {
                Text("Có: \(live.availableDubTracks.joined(separator: ", "))").font(.caption2).foregroundStyle(.secondary)
            }
            if let e = live.lastError { Text(e).font(.caption).foregroundStyle(.red) }
        }.padding(12)
    }
}

/// Scrolling caption list: original + translation. Shared by native and external views.
struct CaptionsPanel: View {
    @EnvironmentObject var captions: CaptionStore
    @EnvironmentObject var settings: SettingsStore

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    ForEach(captions.lines) { line in
                        VStack(alignment: .leading, spacing: 2) {
                            Text("\(line.speaker) · \(line.lang)").font(.caption).foregroundStyle(.secondary)
                            Text(line.original).foregroundStyle(line.isFinal ? .primary : .secondary)
                            if let t = line.translation {
                                Text(t).font(.body.weight(.medium)).foregroundStyle(Color.accentColor)
                            }
                        }.id(line.id)
                    }
                }.padding(12).frame(maxWidth: .infinity, alignment: .leading)
            }
            .onChange(of: captions.lines.last) { _, last in
                if let last { withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo(last.id, anchor: .bottom) } }
            }
        }
    }
}
