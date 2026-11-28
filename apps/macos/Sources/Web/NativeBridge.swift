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
    private let copilot: CopilotSessionController
    private let overlay: OverlayController
    private let overlayModel: OverlayModel
    private let overlayWeb: OverlayWebController
    private let permissions: PermissionsManager
    private let settings: SettingsStore

    init(capture: NativeCaptureController, copilot: CopilotSessionController, overlay: OverlayController, overlayModel: OverlayModel,
         overlayWeb: OverlayWebController, permissions: PermissionsManager, settings: SettingsStore) {
        self.capture = capture; self.copilot = copilot; self.overlay = overlay; self.overlayModel = overlayModel; self.overlayWeb = overlayWeb
        self.permissions = permissions; self.settings = settings
        super.init()
        overlayWeb.onClosed = { [weak self] in self?.dispatch("overlay-command", ["command": "closed"]) }
        capture.emit = { [weak self] type, payload in self?.dispatch(type, payload) }
        copilot.emit = { [weak self] type, payload in self?.dispatch(type, payload) }
        // Copilot mode of the overlay follows the session; the web app drives suggestions (pushOverlaySuggestion).
        copilot.onOverlayEvent = { [overlayModel] ev in overlayModel.applyCopilot(ev) }
        copilot.onSessionActive = { [overlay, overlayModel, settings] active, req in
            if active {
                overlayModel.enterCopilotMode(meName: req?.meName, othersName: req?.othersName)
                if settings.copilotAutoShowOverlay { overlay.show() }
            } else {
                overlayModel.exitCopilotMode()
            }
        }
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
            let token = try r.string("accessToken")
            capture.updateToken(token)
            copilot.updateBearer(token)
            return ["updated": true]
        case "startCopilotSession":
            return try await copilot.startCopilotSession(try CopilotStartRequest.parse(r))
        case "stopCopilotSession": return await copilot.stopCopilotSession()
        case "copilotStatus": return copilot.copilotStatus()
        case "pushOverlaySuggestion":
            overlayModel.upsertSuggestion(try OverlaySuggestion.parse(r))
            return NSNull()
        case "clearOverlaySuggestion":
            overlayModel.removeSuggestion(id: try r.string("id"))
            return NSNull()
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
        case "openOverlayWindow":
            overlayWeb.open(bridge: self)
            return ["visible": overlayWeb.isVisible]
        case "closeOverlayWindow":
            overlayWeb.close()
            return ["visible": false]
        case "pushOverlayState":
            guard let state = r.params["state"] as? [String: Any], JSONSerialization.isValidJSONObject(state),
                  let data = try? JSONSerialization.data(withJSONObject: state),
                  let json = String(data: data, encoding: .utf8) else {
                throw BridgeError(code: "INVALID_ARGUMENT", message: "Thiếu 'state' (object).")
            }
            overlayWeb.push(json: json)
            return NSNull()
        case "overlayCommand":
            let command = try r.string("command")
            if command == "expand" || command == "collapse" {     // window layout only, handled here
                overlayWeb.setExpanded(command == "expand")
                return NSNull()
            }
            if command == "hide" { overlayWeb.hide(); return NSNull() }
            if command == "glass" { overlayWeb.setGlass(r.optString("value") ?? ""); return NSNull() }
            if command == "drag" { overlayWeb.drag(r.optString("value") ?? ""); return NSNull() }
            if command == "hitrect" { overlayWeb.setHitRect(r.optString("value") ?? ""); return NSNull() }
            if command == "resize" {                                     // value "WxH" in points
                let parts = (r.optString("value") ?? "").split(separator: "x").compactMap { Double($0) }
                if parts.count == 2 { overlayWeb.resizeCard(width: parts[0], height: parts[1]) }
                return NSNull()
            }
            var payload: [String: Any] = ["command": command]
            if let v = r.optString("value") { payload["value"] = v }
            dispatch("overlay-command", payload)
            return NSNull()
        case "setOverlayOptions":
            if let b = r.optBool("clickThrough") { settings.overlayClickThrough = b }
            if let b = r.optBool("hideFromScreenShare") { settings.overlayHiddenFromScreenShare = b; overlayWeb.applySharing() }
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
            "overlayWindow": true,
            "copilot": CopilotEngines.isAvailable,   // false: built without `make core`, startCopilotSession fails ENGINE_UNAVAILABLE
            "inputDevices": DeviceManager.inputDevices().map { ["uid": $0.uid, "name": $0.name] },
            "supportedApps": MeetingAppSpec.all.map { ["name": $0.name, "bundleIds": $0.bundleIDs] },
            "outputDevices": outs.map { ["uid": $0.uid, "name": $0.name, "isVirtual": DeviceManager.isVirtual($0)] },
            "blackHoleInstalled": outs.contains { $0.name.lowercased().contains("blackhole") },
            // Every language in System Settings > Language & Region (the web view only sees the
            // first), plus the region: people often run macOS in English but speak another language.
            "preferredLanguages": Locale.preferredLanguages,
            "region": Locale.current.region?.identifier ?? "",
        ]
    }

    private func listAudioApps() -> [[String: Any]] {
        let processes = DeviceManager.audioProcesses()
        return RunningMeetingApps.current().map { t in
            let ids = Set(DeviceManager.processObjects(for: t))
            let producing = processes.contains { ids.contains($0.objectID) && $0.isRunningOutput }
            var row: [String: Any] = ["bundleId": t.bundleID, "name": t.name, "pid": Int(t.pid), "producingAudio": producing]
            if let open = CallWindows.isCallOpen(pid: t.pid, bundleID: t.bundleID) { row["callOpen"] = open }
            return row
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

/// Whether a meeting app shows a call window, from window titles ("Meet - abc-defg-hij",
/// "Zoom Meeting", Teams' "Meeting ..."). `nil` when titles are unreadable (no Screen Recording
/// permission) or the app is not one we know: the caller must then not infer anything.
enum CallWindows {
    static func isCallOpen(pid: pid_t, bundleID: String) -> Bool? {
        guard let info = CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] else { return nil }
        let titles = info.compactMap { w -> String? in
            guard (w[kCGWindowOwnerPID as String] as? Int).map(pid_t.init) == pid,
                  (w[kCGWindowLayer as String] as? Int) == 0 else { return nil }
            return w[kCGWindowName as String] as? String
        }
        guard titles.contains(where: { !$0.isEmpty }) else { return nil }
        let id = bundleID.lowercased()
        let matches: (String) -> Bool
        if id.contains("zoom") {
            matches = { $0.range(of: "zoom (meeting|webinar)", options: [.regularExpression, .caseInsensitive]) != nil }
        } else if id.contains("teams") {
            matches = { $0.range(of: "meeting|call|cuộc họp", options: [.regularExpression, .caseInsensitive]) != nil }
        } else {
            // Browsers: Google Meet titles its call tab "Meet - <code>" (only the active tab of each
            // window is visible, so a switched tab reads as "no call": callers debounce).
            matches = { $0.range(of: "^Meet [-–] [a-z]{3}-[a-z]{4}-[a-z]{3}", options: .regularExpression) != nil }
        }
        return titles.contains(where: matches)
    }
}
