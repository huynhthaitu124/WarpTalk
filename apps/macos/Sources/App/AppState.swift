import Foundation

/// Wires the native services together. The product UI lives in the web app (WKWebView).
@MainActor
final class AppState: ObservableObject {
    let settings: SettingsStore
    let permissions: PermissionsManager
    let overlayModel: OverlayModel
    let overlay: OverlayController
    let capture: NativeCaptureController
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
        let overlayWeb = OverlayWebController(settings: settings)
        let bridge = NativeBridge(capture: capture, overlay: overlay, overlayModel: overlayModel,
                                  overlayWeb: overlayWeb, permissions: permissions, settings: settings)
        self.settings = settings; self.permissions = permissions; self.overlayModel = overlayModel
        self.overlay = overlay; self.capture = capture; self.bridge = bridge
        self.web = WebViewModel(settings: settings, bridge: bridge)
    }
}
