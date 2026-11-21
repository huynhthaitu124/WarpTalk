import SwiftUI

struct RootView: View {
    @EnvironmentObject var app: AppState

    var body: some View {
        Group {
            if app.session == nil {
                AuthView()
            } else {
                MainView()
            }
        }
        .task { if app.session != nil { await app.reloadWorkspaces() } }
        .alert("Lỗi", isPresented: Binding(get: { app.errorMessage != nil }, set: { if !$0 { app.errorMessage = nil } })) {
            Button("OK", role: .cancel) {}
        } message: { Text(app.errorMessage ?? "") }
    }
}

struct MainView: View {
    @EnvironmentObject var app: AppState
    @EnvironmentObject var settings: SettingsStore

    var body: some View {
        NavigationSplitView {
            List(selection: $app.sidebar) {
                Section("Workspace") { WorkspacePicker() }
                Section {
                    ForEach(SidebarItem.allCases) { item in
                        Label(item.rawValue, systemImage: item.icon).tag(item)
                    }
                }
            }
            .navigationSplitViewColumnWidth(min: 200, ideal: 230)
            .safeAreaInset(edge: .bottom) {
                HStack {
                    Text(app.session?.email ?? "").font(.caption).lineLimit(1)
                    Spacer()
                    Button("Đăng xuất") { Task { await app.signOut() } }.buttonStyle(.link)
                }.padding(8)
            }
        } detail: {
            switch app.sidebar {
            case .rooms: RoomsView()
            case .external: ExternalMeetingView()
            case .permissions: PermissionsView()
            case .settings: SettingsView()
            }
        }
    }
}
