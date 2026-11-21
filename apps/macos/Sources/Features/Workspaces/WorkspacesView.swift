import SwiftUI

/// Sidebar picker + "create workspace" sheet.
struct WorkspacePicker: View {
    @EnvironmentObject var app: AppState
    @EnvironmentObject var settings: SettingsStore
    @State private var showCreate = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if app.workspaces.isEmpty {
                Text("Chưa có workspace").font(.caption).foregroundStyle(.secondary)
            } else {
                Picker("", selection: $settings.selectedWorkspaceId) {
                    ForEach(app.workspaces) { Text($0.name).tag($0.id) }
                }.labelsHidden()
            }
            HStack {
                Button("Tạo mới") { showCreate = true }.buttonStyle(.link)
                Button("Tải lại") { Task { await app.reloadWorkspaces() } }.buttonStyle(.link)
            }.font(.caption)
        }
        .sheet(isPresented: $showCreate) { CreateWorkspaceSheet() }
    }
}

struct CreateWorkspaceSheet: View {
    @EnvironmentObject var app: AppState
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var desc = ""
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Tạo workspace").font(.headline)
            TextField("Tên", text: $name).textFieldStyle(.roundedBorder)
            TextField("Mô tả (tuỳ chọn)", text: $desc).textFieldStyle(.roundedBorder)
            if let error { Text(error).foregroundStyle(.red).font(.callout) }
            HStack {
                Spacer()
                Button("Huỷ") { dismiss() }
                Button("Tạo") {
                    busy = true
                    Task {
                        defer { busy = false }
                        do {
                            let ws = try await app.api.createWorkspace(name: name, description: desc.isEmpty ? nil : desc)
                            await app.reloadWorkspaces()
                            app.settings.selectedWorkspaceId = ws.id
                            dismiss()
                        } catch { self.error = (error as? LocalizedError)?.errorDescription ?? "\(error)" }
                    }
                }.keyboardShortcut(.defaultAction).disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || busy)
            }
        }.padding(20).frame(width: 360)
    }
}
