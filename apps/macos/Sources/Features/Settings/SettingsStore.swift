import Foundation

/// Native preferences (UserDefaults). Minimal on purpose: the web app owns product settings.
@MainActor
final class SettingsStore: ObservableObject {
    static let defaultWebURL = "http://localhost:5173"          // Vite dev server; bundled build later
    static let defaultBackendURL = "http://localhost:8080"      // used when the web app does not pass backendUrl

    @Published var webURL: String { didSet { d.set(webURL, forKey: "webURL") } }
    @Published var backendURL: String { didSet { d.set(backendURL, forKey: "backendURL") } }
    /// Default output device UID for dub playback (voice mode). "" = system default.
    @Published var dubOutputDeviceUID: String { didSet { d.set(dubOutputDeviceUID, forKey: "dubOutputDeviceUID") } }
    @Published var overlayClickThrough: Bool { didSet { d.set(overlayClickThrough, forKey: "overlayClickThrough") } }
    @Published var overlayHiddenFromScreenShare: Bool { didSet { d.set(overlayHiddenFromScreenShare, forKey: "overlayHiddenFromScreenShare") } }

    private let d = UserDefaults.standard

    init() {
        let d = UserDefaults.standard
        webURL = d.string(forKey: "webURL") ?? Self.defaultWebURL
        backendURL = d.string(forKey: "backendURL") ?? Self.defaultBackendURL
        dubOutputDeviceUID = d.string(forKey: "dubOutputDeviceUID") ?? ""
        overlayClickThrough = d.bool(forKey: "overlayClickThrough")
        overlayHiddenFromScreenShare = d.bool(forKey: "overlayHiddenFromScreenShare")
    }
}
