import Foundation
import SwiftUI

enum SidebarItem: String, CaseIterable, Identifiable {
    case rooms = "Phòng họp"
    case external = "Họp bên ngoài"
    case permissions = "Quyền"
    case settings = "Cài đặt"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .rooms: return "person.3"
        case .external: return "rectangle.on.rectangle"
        case .permissions: return "lock.shield"
        case .settings: return "gearshape"
        }
    }
}

/// Root observable state: session, workspaces, shared services.
@MainActor
final class AppState: ObservableObject {
    let settings = SettingsStore()
    let api: APIClient
    let tokenStore: TokenStore
    let captions = CaptionStore()
    let overlay = OverlayController()
    let permissions = PermissionsManager()

    @Published var session: StoredSession?
    @Published var workspaces: [Workspace] = []
    @Published var sidebar: SidebarItem = .rooms
    @Published var errorMessage: String?

    var selectedWorkspace: Workspace? {
        workspaces.first { $0.id == settings.selectedWorkspaceId } ?? workspaces.first
    }

    init() {
        let store = KeychainTokenStore()
        tokenStore = store
        // The closure reads the current URL each request, so changing it in Settings applies immediately.
        // UserDefaults is thread-safe; avoids hopping to MainActor from the actor.
        api = APIClient(baseURL: { UserDefaults.standard.string(forKey: "backendURL") ?? SettingsStore.defaultBackendURL }, store: store)
        session = store.load()
        Task { await api.setOnSessionExpired { [weak self] in
            Task { @MainActor in self?.session = nil; self?.workspaces = [] }
        } }
        overlay.bind(settings: settings, captions: captions)
    }

    func signedIn(_ s: StoredSession) {
        session = s
        Task { await reloadWorkspaces() }
    }

    func signOut() async {
        await api.logout()
        session = nil
        workspaces = []
    }

    func reloadWorkspaces() async {
        do {
            let page = try await api.listWorkspaces()
            workspaces = page.items
            if settings.selectedWorkspaceId.isEmpty || !page.items.contains(where: { $0.id == settings.selectedWorkspaceId }) {
                settings.selectedWorkspaceId = page.items.first?.id ?? ""
            }
        } catch {
            report(error)
        }
    }

    func report(_ error: Error) {
        errorMessage = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }
}
