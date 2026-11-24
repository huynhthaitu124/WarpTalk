import Foundation

/// Native preferences (UserDefaults). Minimal on purpose: the web app owns product settings.
@MainActor
final class SettingsStore: ObservableObject {
    /// Next.js dev server (`apps/web`: `npm run dev`).
    static let defaultWebURL = "http://localhost:3000"
    /// Tried after the configured URL when it is unreachable (the previous Vite dev server).
    static let defaultWebFallbackURLs = ""
    static let defaultBackendURL = "http://localhost:8080"      // used when the web app does not pass backendUrl
    static let defaultMeetingAppBundleId = "com.google.Chrome"
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

    // Copilot (ADR 0001)
    /// Meeting app whose audio becomes the `others` channel when `startCopilotSession` gets no `appBundleId`.
    @Published var meetingAppBundleId: String { didSet { d.set(meetingAppBundleId, forKey: "meetingAppBundleId") } }
    /// Microphone (input device UID) for the `me` channel. "" = system default input.
    @Published var micDeviceUID: String { didSet { d.set(micDeviceUID, forKey: "micDeviceUID") } }
    @Published var overlayPosition: OverlayPosition { didSet { d.set(overlayPosition.rawValue, forKey: "overlayPosition") } }
    /// Show the overlay automatically while a copilot session runs.
    @Published var copilotAutoShowOverlay: Bool { didSet { d.set(copilotAutoShowOverlay, forKey: "copilotAutoShowOverlay") } }
    /// HIDDEN (no UI): use the core's fake STT for demos/offline. `defaults write <bundle id> copilotFakeStt -bool YES`.
    @Published var copilotFakeStt: Bool { didSet { d.set(copilotFakeStt, forKey: "copilotFakeStt") } }

    private let d = UserDefaults.standard

    init() {
        let d = UserDefaults.standard
        var web = d.string(forKey: "webURL") ?? Self.defaultWebURL
        // One-time migration: the old default pointed at Vite; the web app moved to Next.js (port 3000).
        if !d.bool(forKey: "webURLMigratedToNext") {
            if web == Self.legacyViteURL { web = Self.defaultWebURL; d.set(web, forKey: "webURL") }
            d.set(true, forKey: "webURLMigratedToNext")
        }
        webURL = web
        webFallbackURLs = d.string(forKey: "webFallbackURLs") ?? Self.defaultWebFallbackURLs
        backendURL = d.string(forKey: "backendURL") ?? Self.defaultBackendURL
        activeWebURL = web
        dubOutputDeviceUID = d.string(forKey: "dubOutputDeviceUID") ?? ""
        overlayClickThrough = d.bool(forKey: "overlayClickThrough")
        // Product decision (ADR 0001): transparent meeting assistant, so the overlay is NOT hidden from screen share by default.
        overlayHiddenFromScreenShare = d.bool(forKey: "overlayHiddenFromScreenShare")
        meetingAppBundleId = d.string(forKey: "meetingAppBundleId") ?? Self.defaultMeetingAppBundleId
        micDeviceUID = d.string(forKey: "micDeviceUID") ?? ""
        overlayPosition = d.string(forKey: "overlayPosition").flatMap(OverlayPosition.init(rawValue:)) ?? .bottomCenter
        copilotAutoShowOverlay = d.object(forKey: "copilotAutoShowOverlay") as? Bool ?? true
        copilotFakeStt = d.bool(forKey: "copilotFakeStt")
    }

    func setActiveWebURL(_ url: String) { activeWebURL = url }
}

/// Where the floating overlay sits on the main screen.
enum OverlayPosition: String, CaseIterable, Identifiable {
    case bottomCenter, bottomLeft, bottomRight, topCenter, topLeft, topRight
    var id: String { rawValue }

    var label: String {
        switch self {
        case .bottomCenter: return "Dưới, giữa"
        case .bottomLeft: return "Dưới, trái"
        case .bottomRight: return "Dưới, phải"
        case .topCenter: return "Trên, giữa"
        case .topLeft: return "Trên, trái"
        case .topRight: return "Trên, phải"
        }
    }

    /// Window origin (AppKit coordinates: y grows upwards) for a window of `size` inside `frame` (the screen's visibleFrame).
    func origin(in frame: CGRect, size: CGSize, margin: CGFloat = 40) -> CGPoint {
        let x: CGFloat
        switch self {
        case .bottomCenter, .topCenter: x = frame.midX - size.width / 2
        case .bottomLeft, .topLeft: x = frame.minX + margin
        case .bottomRight, .topRight: x = frame.maxX - size.width - margin
        }
        let y: CGFloat
        switch self {
        case .bottomCenter, .bottomLeft, .bottomRight: y = frame.minY + margin
        case .topCenter, .topLeft, .topRight: y = frame.maxY - size.height - margin
        }
        return CGPoint(x: x, y: y)
    }
}
