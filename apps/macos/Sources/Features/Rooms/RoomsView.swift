import SwiftUI

@MainActor
final class RoomsViewModel: ObservableObject {
    @Published var rooms: [TranslationRoom] = []
    @Published var loading = false
    @Published var error: String?

    func load(app: AppState) async {
        guard let ws = app.selectedWorkspace else { rooms = []; return }
        loading = true; defer { loading = false }
        do { rooms = try await app.api.listRooms(workspaceId: ws.id).items; error = nil }
        catch { self.error = (error as? LocalizedError)?.errorDescription ?? "\(error)" }
    }
}

struct RoomsView: View {
    @EnvironmentObject var app: AppState
    @EnvironmentObject var settings: SettingsStore
    @StateObject private var vm = RoomsViewModel()
    @State private var showCreate = false
    @State private var showJoinCode = false
    @State private var active: MeetingViewModel?

    var body: some View {
        Group {
            if let active {
                MeetingView(vm: active) { self.active = nil; Task { await vm.load(app: app) } }
            } else {
                list
            }
        }
        .task(id: settings.selectedWorkspaceId) { await vm.load(app: app) }
    }

    private var list: some View {
        VStack(alignment: .leading) {
            HStack {
                Text(app.selectedWorkspace.map { "Phòng trong \($0.name)" } ?? "Chọn hoặc tạo workspace").font(.title2.bold())
                Spacer()
                Button("Vào bằng mã") { showJoinCode = true }
                Button("Tạo phòng") { showCreate = true }.disabled(app.selectedWorkspace == nil)
                Button { Task { await vm.load(app: app) } } label: { Image(systemName: "arrow.clockwise") }
            }
            if let e = vm.error { Text(e).foregroundStyle(.red).font(.callout) }
            if vm.loading { ProgressView() }
            List(vm.rooms) { room in
                RoomRow(room: room) { active = MeetingViewModel(room: room, app: app) }
            }
        }
        .padding()
        .sheet(isPresented: $showCreate) { CreateRoomSheet { Task { await vm.load(app: app) } } }
        .sheet(isPresented: $showJoinCode) { JoinByCodeSheet { room in active = MeetingViewModel(room: room, app: app) } }
    }
}

struct RoomRow: View {
    let room: TranslationRoom
    let open: () -> Void
    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(room.title).font(.headline)
                Text("Mã: \(room.roomCode) · \(room.sourceLanguage) → \(room.targetLanguages.joined(separator: ", ")) · \(room.status)")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Button("Mở") { open() }.disabled(room.statusEnum.isClosed)
        }
    }
}

struct CreateRoomSheet: View {
    @EnvironmentObject var app: AppState
    @EnvironmentObject var settings: SettingsStore
    @Environment(\.dismiss) private var dismiss
    var onCreated: () -> Void
    @State private var title = ""
    @State private var source: Language = .vi
    @State private var targets: Set<Language> = [.en]
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Tạo phòng dịch").font(.headline)
            TextField("Tiêu đề", text: $title).textFieldStyle(.roundedBorder)
            Picker("Ngôn ngữ nói", selection: $source) { ForEach(Language.allCases) { Text($0.label).tag($0) } }
            Text("Ngôn ngữ dịch sang").font(.subheadline)
            HStack {
                ForEach(Language.allCases) { l in
                    Toggle(l.label, isOn: Binding(get: { targets.contains(l) },
                                                  set: { if $0 { targets.insert(l) } else { targets.remove(l) } }))
                }
            }
            if let error { Text(error).foregroundStyle(.red).font(.callout) }
            HStack {
                Spacer()
                Button("Huỷ") { dismiss() }
                Button("Tạo") { create() }.keyboardShortcut(.defaultAction)
                    .disabled(busy || title.trimmingCharacters(in: .whitespaces).isEmpty || targets.isEmpty)
            }
        }
        .padding(20).frame(width: 440)
        .onAppear { source = settings.speakLanguage; targets = [settings.listenLanguage] }
    }

    private func create() {
        guard let ws = app.selectedWorkspace else { return }
        busy = true
        Task {
            defer { busy = false }
            do {
                _ = try await app.api.createRoom(CreateRoomRequest(
                    workspaceId: ws.id, title: title, displayName: nil, sourceLanguage: source.rawValue,
                    targetLanguages: targets.map(\.rawValue).sorted(), requiresApproval: false))
                onCreated(); dismiss()
            } catch { self.error = (error as? LocalizedError)?.errorDescription ?? "\(error)" }
        }
    }
}

struct JoinByCodeSheet: View {
    @EnvironmentObject var app: AppState
    @EnvironmentObject var settings: SettingsStore
    @Environment(\.dismiss) private var dismiss
    var onJoined: (TranslationRoom) -> Void
    @State private var code = ""
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Vào phòng bằng mã (12 ký tự)").font(.headline)
            TextField("abcdefghijkl", text: $code).textFieldStyle(.roundedBorder).font(.system(.body, design: .monospaced))
            Text("Nói: \(settings.speakLanguage.label) · Nghe: \(settings.listenLanguage.label) (đổi trong Cài đặt)")
                .font(.caption).foregroundStyle(.secondary)
            if let error { Text(error).foregroundStyle(.red).font(.callout) }
            HStack {
                Spacer()
                Button("Huỷ") { dismiss() }
                Button("Vào") { join() }.keyboardShortcut(.defaultAction).disabled(busy || code.trimmingCharacters(in: .whitespaces).count != 12)
            }
        }.padding(20).frame(width: 420)
    }

    private func join() {
        busy = true
        Task {
            defer { busy = false }
            do {
                let p = try await app.api.joinRoom(byCode: JoinByCodeRequest(
                    roomCode: code.trimmingCharacters(in: .whitespaces), displayName: nil,
                    speakLanguage: settings.speakLanguage.rawValue, listenLanguage: settings.listenLanguage.rawValue))
                _ = p
                // The join response is the participant; fetch the room via the list by code match.
                if let ws = app.selectedWorkspace,
                   let room = try await app.api.listRooms(workspaceId: ws.id).items.first(where: { $0.roomCode.lowercased() == code.trimmingCharacters(in: .whitespaces).lowercased() }) {
                    dismiss(); onJoined(room)
                } else {
                    error = "Đã vào phòng nhưng không tìm thấy phòng trong workspace đang chọn. Chọn đúng workspace rồi mở từ danh sách."
                }
            } catch { self.error = (error as? LocalizedError)?.errorDescription ?? "\(error)" }
        }
    }
}
