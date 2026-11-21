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
    func recordAudioCaptureResult(success: Bool) { audioCapture = success ? .granted : .denied }

    // Deep links (System Settings, macOS 13+/14).
    enum Pane: String {
        case microphone = "Privacy_Microphone"
        case screenRecording = "Privacy_ScreenCapture"
        // VERIFY: pane id for "Audio Recording"/system audio is Privacy_AudioCapture on 14.2+; falls back to Privacy root.
        case audioCapture = "Privacy_AudioCapture"
        case privacy = "Privacy"
    }

    func open(_ pane: Pane) {
        let urls = ["x-apple.systempreferences:com.apple.preference.security?\(pane.rawValue)",
                    "x-apple.systempreferences:com.apple.preference.security"]
        for u in urls { if let url = URL(string: u), NSWorkspace.shared.open(url) { return } }
    }
}
