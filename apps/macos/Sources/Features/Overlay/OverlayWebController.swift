import AppKit
import WebKit

/// Floating copilot for external meetings (Teams / Google Meet): a small pill pinned to the top centre of the screen
/// that expands into a card (transcript, suggestions, follow-ups) when clicked.
///
/// The window never changes size for pill <-> card (resizing a WKWebView window shows a frame of stale content,
/// which read as a stutter): it is always card-sized and transparent, and only the area the page reports with
/// `overlayCommand("hitrect", "x,y,w,h")` (pill, plus the card when open) takes the mouse. Everywhere else
/// `ignoresMouseEvents` lets clicks fall through to the app underneath.
///
/// It hosts a second WKWebView on the same web origin at `/overlay`. That page never authenticates (the refresh
/// token rotates, so two pages refreshing would race): it renders only the state the main page relays through
/// `pushOverlayState`, and sends user actions back with `overlayCommand` (forwarded to the main page as a
/// `warptalk-native` event of type `overlay-command`).
@MainActor
final class OverlayWebController: NSObject, WKNavigationDelegate, NSWindowDelegate {
    private let settings: SettingsStore
    private var panel: NSPanel?
    private var webView: WKWebView?
    private var lastState: String?
    private(set) var expanded = false
    // Includes a transparent margin so the web content's CSS shadow is not clipped.
    static let defaultCardSize = NSSize(width: 900, height: 380)
    static let minCardSize = NSSize(width: 560, height: 220)
    /// Card size; the user drags the grip on the card to change it (kept for the session).
    private(set) var cardSize = OverlayWebController.defaultCardSize
    /// Interactive area in page coordinates (top-left origin, points). Default: roughly the wide pill.
    private var hitRect = NSRect(x: (OverlayWebController.defaultCardSize.width - 400) / 2, y: 0, width: 400, height: 60)
    private var pointerInside = false
    private var pointerTimer: Timer?
    /// Called when the user closes the window with the title-bar button.
    var onClosed: (() -> Void)?

    init(settings: SettingsStore) {
        self.settings = settings
        super.init()
    }

    var isVisible: Bool { panel?.isVisible ?? false }

    func open(bridge: NativeBridge) {
        if panel == nil { build(bridge: bridge) }
        if let panel, !panel.isVisible {
            panel.setFrame(Self.frame(for: cardSize, on: panel.screen ?? NSScreen.main), display: true, animate: false)
        }
        applySharing()
        load()
        panel?.orderFrontRegardless()
        startPointerTracking()
    }

    /// Hidden by the user from the card (keeps the state; the main page can reopen it).
    func hide() {
        stopPointerTracking()
        panel?.orderOut(nil)
        onClosed?()
    }

    func close() {
        stopPointerTracking()
        panel?.orderOut(nil)
        lastState = nil
    }

    /// `json` is the serialized state object; kept so a reload of the overlay page gets it back.
    func push(json: String) {
        lastState = json
        deliver()
    }

    /// Pill <-> card. Only bookkeeping and key status: the page draws both shapes in the fixed-size window.
    func setExpanded(_ on: Bool) {
        expanded = on
        if on { panel?.makeKey() }
    }

    /// The page's interactive area ("x,y,w,h" in page points).
    func setHitRect(_ value: String) {
        let n = value.split(separator: ",").compactMap { Double($0) }
        guard n.count == 4, n.allSatisfy(\.isFinite) else { return }
        hitRect = NSRect(x: n[0], y: n[1], width: n[2], height: n[3])
        updatePointer()
    }

    // MARK: Click-through outside the pill / card

    private func startPointerTracking() {
        guard pointerTimer == nil else { return }
        let t = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.updatePointer() }
        }
        RunLoop.main.add(t, forMode: .common)
        pointerTimer = t
        updatePointer()
    }

    private func stopPointerTracking() {
        pointerTimer?.invalidate()
        pointerTimer = nil
        pointerInside = false
    }

    private func updatePointer() {
        guard let panel, panel.isVisible else { return }
        // Keep the current state while a button is held (a resize drag may leave the card).
        if NSEvent.pressedMouseButtons != 0 { return }
        let m = NSEvent.mouseLocation
        let f = panel.frame
        let p = NSPoint(x: m.x - f.minX, y: f.maxY - m.y)            // page coordinates, top-left origin
        let inside = f.contains(m) && hitRect.insetBy(dx: -4, dy: -4).contains(p)
        if panel.ignoresMouseEvents == inside { panel.ignoresMouseEvents = !inside }
        if inside != pointerInside {
            pointerInside = inside
            // Once the window ignores the mouse, WebKit never sees it leave: tell the page.
            webView?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('warptalk-overlay-pointer',{detail:{inside:\(inside)}}))",
                                        completionHandler: nil)
        }
    }

    /// Live resize from the page's drag grip. Stays centred and top-anchored; clamped to the screen.
    func resizeCard(width: Double, height: Double) {
        guard width.isFinite, height.isFinite else { return }
        let vf = (panel?.screen ?? NSScreen.main)?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        cardSize = NSSize(width: min(max(width, Self.minCardSize.width), vf.width - 16),
                          height: min(max(height, Self.minCardSize.height), vf.height - 16))
        guard let panel else { return }
        panel.setFrame(Self.frame(for: cardSize, on: panel.screen ?? NSScreen.main), display: true, animate: false)
    }

    private static func frame(for size: NSSize, on screen: NSScreen?) -> NSRect {
        let vf = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let h = min(size.height, vf.height - 16)
        return NSRect(x: vf.midX - size.width / 2, y: vf.maxY - h, width: size.width, height: h)
    }

    func applySharing() {
        // .none keeps the copilot out of screen shares (best effort; some capture paths ignore it).
        panel?.sharingType = settings.overlayHiddenFromScreenShare ? .none : .readOnly
    }

    // MARK: Build

    private func build(bridge: NativeBridge) {
        let cfg = WKWebViewConfiguration()
        cfg.setURLSchemeHandler(WarpTalkSchemeHandler(backendURL: { [weak settings] in
            MainActor.assumeIsolated { settings?.backendURL ?? "" }
        }), forURLScheme: WebURLPlan.bundledScheme)
        WebViewModel.register(replyHandler: bridge, on: cfg.userContentController)
        let wv = WKWebView(frame: .zero, configuration: cfg)
        if #available(macOS 13.3, *) { wv.isInspectable = true }
        wv.navigationDelegate = self
        wv.setValue(false, forKey: "drawsBackground")      // no white flash before the dark page paints
        webView = wv

        let p = CopilotPanel(contentRect: Self.frame(for: cardSize, on: NSScreen.main),
                             styleMask: [.borderless, .nonactivatingPanel],
                             backing: .buffered, defer: false)
        p.title = "WarpTalk"
        p.level = .floating
        p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        p.isFloatingPanel = true
        p.hidesOnDeactivate = false
        p.becomesKeyOnlyIfNeeded = true
        p.isOpaque = false
        p.hasShadow = false                                 // the page draws its own shadow around the pill / card
        p.backgroundColor = .clear
        p.appearance = NSAppearance(named: .darkAqua)
        p.isReleasedWhenClosed = false
        p.delegate = self
        p.contentView = wv
        p.ignoresMouseEvents = true                         // until the pointer is over the pill (see updatePointer)
        panel = p
    }

    private func load() {
        guard let webView, var c = URLComponents(string: settings.activeWebURL) else { return }
        c.path = "/overlay"
        c.query = nil
        c.fragment = nil
        guard let url = c.url else { return }
        if webView.url?.path == "/overlay", webView.url?.host == url.host { deliver(); return }
        let config = WebRuntimeConfig.make(activeWebURL: url, backendURL: settings.backendURL)
        let ucc = webView.configuration.userContentController
        ucc.removeAllUserScripts()
        ucc.addUserScript(WKUserScript(source: BridgeScript.make(config: config), injectionTime: .atDocumentStart, forMainFrameOnly: true))
        webView.load(URLRequest(url: url))
    }

    private func deliver() {
        guard let webView, let s = lastState else { return }
        webView.evaluateJavaScript("window.__warptalkOverlayState=\(s);window.dispatchEvent(new CustomEvent('warptalk-overlay',{detail:\(s)}))",
                                   completionHandler: nil)
    }

    // MARK: WKNavigationDelegate

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { deliver() }

    /// Links in the overlay open in the default browser.
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        if navigationAction.navigationType == .linkActivated, let url = navigationAction.request.url {
            NSWorkspace.shared.open(url)
            decisionHandler(.cancel)
            return
        }
        decisionHandler(.allow)
    }

    // MARK: NSWindowDelegate

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        sender.orderOut(nil)
        onClosed?()
        return false
    }
}

/// Borderless panels refuse key status by default; the card needs it for its language pickers.
private final class CopilotPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}
