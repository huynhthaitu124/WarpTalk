import AppKit
import AVFoundation
import Foundation
import WebKit

/// Native side of `window.warptalkNative` (see apps/macos/BRIDGE.md).
/// Uses `WKScriptMessageHandlerWithReply`, so JS `postMessage` returns a Promise.
@MainActor
final class NativeBridge: NSObject, WKScriptMessageHandlerWithReply {
    static let handlerName = "warptalk"
    weak var webView: WKWebView?

    private let capture: NativeCaptureController
    private let overlay: OverlayController
    private let overlayModel: OverlayModel
    private let permissions: PermissionsManager
    private let settings: SettingsStore

    init(capture: NativeCaptureController, overlay: OverlayController, overlayModel: OverlayModel,
         permissions: PermissionsManager, settings: SettingsStore) {
        self.capture = capture; self.overlay = overlay; self.overlayModel = overlayModel
        self.permissions = permissions; self.settings = settings
        super.init()
        capture.emit = { [weak self] type, payload in self?.dispatch(type, payload) }
        // Opt-in (`nativeOverlayCaptions`): the native listener's own `captions` packets drive the overlay.
        capture.onCaption = { [overlayModel] u in
            overlayModel.push(speaker: u.speaker, text: u.text, translation: u.translation, segmentId: u.segmentId, isFinal: u.isFinal)
        }
    }

    // MARK: WKScriptMessageHandlerWithReply

    func userContentController(_ ucc: WKUserContentController, didReceive message: WKScriptMessage,
                               replyHandler: @escaping (Any?, String?) -> Void) {
        // Signature: WKScriptMessageHandlerWithReply.h:90 (macOS 11+): replyHandler(Any?, String?), at most once; a non-nil
        // second argument rejects the JS promise, so errors travel inside the `{ok:false,error}` payload instead.
        let origin = message.frameInfo.securityOrigin
        guard message.frameInfo.isMainFrame,
              OriginPolicy.isTrusted(scheme: origin.protocol, host: origin.host, port: origin.port, allowed: settings.activeWebURL) else {
            replyHandler(BridgeReply.fail("FORBIDDEN_ORIGIN", "Origin không được phép dùng native bridge."), nil)
            return
        }
        guard let req = BridgeRequest(message.body) else {
            replyHandler(BridgeReply.fail("INVALID_ARGUMENT", "Yêu cầu bridge không hợp lệ."), nil)
            return
        }
        Task { @MainActor in
            do { replyHandler(BridgeReply.ok(try await self.handle(req)), nil) }
            catch {
                let e = NativeCaptureController.map(error)
                replyHandler(BridgeReply.fail(e.code, e.message), nil)
            }
        }
    }

    // MARK: Methods

    private func handle(_ r: BridgeRequest) async throws -> Any {
        switch r.method {
        case "getCapabilities": return capabilities()
        case "listAudioApps": return listAudioApps()
        case "startExternalCapture":
            let mode = r.optString("mode") ?? "text"
            guard mode == "text" || mode == "voice" else { throw BridgeError(code: "INVALID_ARGUMENT", message: "mode phải là 'text' hoặc 'voice'.") }
            let backend = r.optString("captureBackend") ?? "auto"
            guard ["auto", "processTap", "screenCaptureKit"].contains(backend) else {
                throw BridgeError(code: "INVALID_ARGUMENT", message: "captureBackend không hợp lệ.")
            }
            try await capture.start(StartCaptureParams(
                appBundleId: try r.string("appBundleId"), roomId: try r.string("roomId"), mode: mode,
                accessToken: try r.string("accessToken"), backendUrl: r.optString("backendUrl"),
                heartbeatIntervalSeconds: r.optDouble("heartbeatIntervalSeconds") ?? 15,
                listenLanguage: r.optString("listenLanguage"), dubOutputDeviceUid: r.optString("dubOutputDeviceUid"),
                captureBackend: backend, nativeOverlayCaptions: r.optBool("nativeOverlayCaptions") ?? false))
            return capture.status()
        case "stopExternalCapture":
            await capture.stop()
            return capture.status()
        case "captureStatus": return capture.status()
        case "updateAuth":
            capture.updateToken(try r.string("accessToken"))
            return ["updated": true]
        case "showOverlay":
            guard let v = r.optBool("visible") else { throw BridgeError(code: "INVALID_ARGUMENT", message: "Thiếu 'visible' (bool).") }
            overlay.setVisible(v)
            return ["visible": overlay.isVisible]
        case "pushOverlayCaption":
            overlayModel.push(speaker: r.optString("speaker") ?? "", text: try r.string("text"),
                              translation: r.optString("translation"), segmentId: r.optString("segmentId"),
                              isFinal: r.optBool("isFinal") ?? true)
            return NSNull()
        case "clearOverlay": overlayModel.clear(); return NSNull()
        case "setOverlayOptions":
            if let b = r.optBool("clickThrough") { settings.overlayClickThrough = b }
            if let b = r.optBool("hideFromScreenShare") { settings.overlayHiddenFromScreenShare = b }
            return ["clickThrough": settings.overlayClickThrough, "hideFromScreenShare": settings.overlayHiddenFromScreenShare]
        case "permissions": return permissions.snapshot()
        case "requestPermission":
            switch try r.string("kind") {
            case "microphone": await permissions.requestMicrophone()
            case "screenRecording": permissions.requestScreenRecording()
            case "audioCapture":
                throw BridgeError(code: "NOT_REQUESTABLE", message: "Quyền audioCapture chỉ hiện hộp thoại khi bắt đầu capture lần đầu; dùng openSystemSettings('audioCapture') để chỉnh.")
            default: throw BridgeError(code: "INVALID_ARGUMENT", message: "kind không hợp lệ.")
            }
            return permissions.snapshot()
        case "openSystemSettings":
            guard let pane = PermissionsManager.Pane.from(wire: try r.string("pane")) else {
                throw BridgeError(code: "INVALID_ARGUMENT", message: "pane không hợp lệ.")
            }
            permissions.open(pane)
            return NSNull()
        default:
            throw BridgeError(code: "UNKNOWN_METHOD", message: "Không có method '\(r.method)'.")
        }
    }

    private func capabilities() -> [String: Any] {
        let os = ProcessInfo.processInfo.operatingSystemVersion
        let outs = DeviceManager.outputDevices()
        return [
            "version": 1,
            "platform": "macos",
            "osVersion": "\(os.majorVersion).\(os.minorVersion).\(os.patchVersion)",
            "processTap": ProcessInfo.processInfo.isOperatingSystemAtLeast(OperatingSystemVersion(majorVersion: 14, minorVersion: 2, patchVersion: 0)),
            "screenCaptureKit": true,
            "voiceMode": true,
            "overlay": true,
            "supportedApps": MeetingAppSpec.all.map { ["name": $0.name, "bundleIds": $0.bundleIDs] },
            "outputDevices": outs.map { ["uid": $0.uid, "name": $0.name, "isVirtual": DeviceManager.isVirtual($0)] },
            "blackHoleInstalled": outs.contains { $0.name.lowercased().contains("blackhole") },
        ]
    }

    private func listAudioApps() -> [[String: Any]] {
        let processes = DeviceManager.audioProcesses()
        return RunningMeetingApps.current().map { t in
            let ids = Set(DeviceManager.processObjects(for: t))
            let producing = processes.contains { ids.contains($0.objectID) && $0.isRunningOutput }
            return ["bundleId": t.bundleID, "name": t.name, "pid": Int(t.pid), "producingAudio": producing]
        }
    }

    // MARK: Events -> window 'warptalk-native'

    func dispatch(_ type: String, _ payload: [String: Any]) {
        var detail = payload
        detail["type"] = type
        detail["ts"] = APIDates.string(Date())
        guard JSONSerialization.isValidJSONObject(detail),
              let data = try? JSONSerialization.data(withJSONObject: detail),
              let json = String(data: data, encoding: .utf8) else { return }
        webView?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('warptalk-native',{detail:\(json)}))", completionHandler: nil)
    }
}
