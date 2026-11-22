import AppKit
import AVFoundation
import CoreGraphics
import Foundation

enum PermissionStatus: String {
    case granted = "Đã cấp"
    case denied = "Bị từ chối"
    case notDetermined = "Chưa hỏi"
    case unknown = "Không kiểm tra được"
}

@MainActor
final class PermissionsManager: ObservableObject {
    @Published var microphone: PermissionStatus = .unknown
    @Published var screenRecording: PermissionStatus = .unknown
    /// Audio capture (process tap) has NO public status API: the prompt appears on first tap creation.
    /// We remember the result of the last attempt.
    @Published var audioCapture: PermissionStatus = .notDetermined

    init() {
        refresh()
        NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.refresh() }
        }
    }

    func refresh() {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: microphone = .granted
        case .denied, .restricted: microphone = .denied
        case .notDetermined: microphone = .notDetermined
        @unknown default: microphone = .unknown
        }
        screenRecording = CGPreflightScreenCaptureAccess() ? .granted : .denied   // cannot distinguish "never asked"
    }

    func requestMicrophone() async {
        _ = await AVCaptureDevice.requestAccess(for: .audio)
        refresh()
    }

    func requestScreenRecording() {
        _ = CGRequestScreenCaptureAccess()
        refresh()
    }

    /// Triggers the system "audio recording" prompt by briefly creating a tap on the first available meeting
    /// app (or reports what happened). Real status is only known after an actual start.
    /// JSON-ready snapshot for the web bridge. Values: granted | denied | notDetermined | unknown.
    func snapshot() -> [String: String] {
        refresh()
        return ["microphone": Self.wire(microphone), "screenRecording": Self.wire(screenRecording), "audioCapture": Self.wire(audioCapture)]
    }

    static func wire(_ s: PermissionStatus) -> String {
        switch s { case .granted: return "granted"; case .denied: return "denied"; case .notDetermined: return "notDetermined"; case .unknown: return "unknown" }
    }

    func recordAudioCaptureResult(success: Bool) { audioCapture = success ? .granted : .denied }

    // Deep links (System Settings, macOS 13+/14). Pane ids: bvanpeski/SystemPreferences macos_preferencepanes-Ventura.md
    // ("Privacy & Security": Privacy_Microphone, Privacy_ScreenCapture). On macOS 14.2+ system-audio recording lives in the
    // same pane, now titled "Screen & System Audio Recording" (section "System Audio Recording Only"), so `audioCapture`
    // opens `Privacy_ScreenCapture`. No separate `Privacy_AudioCapture` id could be confirmed.
    enum Pane {
        case microphone, screenRecording, audioCapture, privacy

        var query: String {
            switch self {
            case .microphone: return "Privacy_Microphone"
            case .screenRecording, .audioCapture: return "Privacy_ScreenCapture"
            case .privacy: return "Privacy"
            }
        }

        /// Wire names used by the JS bridge: microphone | screenRecording | audioCapture | privacy.
        static func from(wire: String) -> Pane? {
            switch wire {
            case "microphone": return .microphone
            case "screenRecording": return .screenRecording
            case "audioCapture": return .audioCapture
            case "privacy": return .privacy
            default: return nil
            }
        }
    }

    func open(_ pane: Pane) {
        let urls = ["x-apple.systempreferences:com.apple.preference.security?\(pane.query)",
                    "x-apple.systempreferences:com.apple.preference.security"]
        for u in urls { if let url = URL(string: u), NSWorkspace.shared.open(url) { return } }
    }
}
