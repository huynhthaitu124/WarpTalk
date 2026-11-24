import Foundation

/// Wires the native services together. The product UI lives in the web app (WKWebView).
@MainActor
final class AppState: ObservableObject {
    let settings: SettingsStore
    let permissions: PermissionsManager
    let overlayModel: OverlayModel
    let overlay: OverlayController
    let capture: NativeCaptureController
    let copilot: CopilotSessionController
    let bridge: NativeBridge
    let web: WebViewModel

    init() {
        let settings = SettingsStore()
        let permissions = PermissionsManager()
        let overlayModel = OverlayModel()
        let overlay = OverlayController()
        overlay.bind(settings: settings, model: overlayModel)
        let capture = NativeCaptureController(permissions: permissions,
                                              defaultBackendURL: { settings.backendURL },
                                              defaultDubDeviceUID: { settings.dubOutputDeviceUID })
        let copilot = CopilotSessionController(deps: .live(permissions: permissions, settings: settings))
        let bridge = NativeBridge(capture: capture, copilot: copilot, overlay: overlay, overlayModel: overlayModel,
                                  permissions: permissions, settings: settings)
        self.settings = settings; self.permissions = permissions; self.overlayModel = overlayModel
        self.overlay = overlay; self.capture = capture; self.copilot = copilot; self.bridge = bridge
        self.web = WebViewModel(settings: settings, bridge: bridge)
    }
}
