import Foundation

/// Native preferences (UserDefaults). Minimal on purpose: the web app owns product settings.
@MainActor
final class SettingsStore: ObservableObject {
    /// Next.js dev server (`apps/web`: `npm run dev`).
    static let defaultWebURL = "http://localhost:3000"
    /// Tried after the configured URL when it is unreachable (the previous Vite dev server).
    static let defaultWebFallbackURLs = ""
    static let defaultBackendURL = "http://localhost:8080"      // used when the web app does not pass backendUrl
    private static let legacyViteURL = "http://localhost:5173"

    @Published var webURL: String { didSet { d.set(webURL, forKey: "webURL") } }
    /// Newline/comma separated fallback URLs, tried in order when `webURL` does not answer.
    @Published var webFallbackURLs: String { didSet { d.set(webFallbackURLs, forKey: "webFallbackURLs") } }
    @Published var backendURL: String { didSet { d.set(backendURL, forKey: "backendURL") } }
    /// The URL actually loaded in the web view (one of the candidates). The native bridge is only available to this origin.
    @Published private(set) var activeWebURL: String
    /// Default output device UID for dub playback (voice mode). "" = system default.
    @Published var dubOutputDeviceUID: String { didSet { d.set(dubOutputDeviceUID, forKey: "dubOutputDeviceUID") } }
    @Published var overlayClickThrough: Bool { didSet { d.set(overlayClickThrough, forKey: "overlayClickThrough") } }
    @Published var overlayHiddenFromScreenShare: Bool { didSet { d.set(overlayHiddenFromScreenShare, forKey: "overlayHiddenFromScreenShare") } }

    private let d = UserDefaults.standard

    init() {
        let d = UserDefaults.standard
        var web = d.string(forKey: "webURL") ?? Self.defaultWebURL
        // The old default pointed at Vite (5173), which no longer serves anything; the web app moved
        // to Next.js (port 3000). Always replace it, even if an earlier build already set the flag.
        if web == Self.legacyViteURL { web = Self.defaultWebURL; d.set(web, forKey: "webURL") }
        d.set(true, forKey: "webURLMigratedToNext")
        webURL = web
        webFallbackURLs = d.string(forKey: "webFallbackURLs") ?? Self.defaultWebFallbackURLs
        backendURL = d.string(forKey: "backendURL") ?? Self.defaultBackendURL
        activeWebURL = web
        dubOutputDeviceUID = d.string(forKey: "dubOutputDeviceUID") ?? ""
        overlayClickThrough = d.bool(forKey: "overlayClickThrough")
        overlayHiddenFromScreenShare = d.bool(forKey: "overlayHiddenFromScreenShare")
    }

    func setActiveWebURL(_ url: String) { activeWebURL = url }
}
