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
    /// Where the user dragged the pill to, relative to its home (top centre). Zero = home.
    private var dragOffset = NSPoint.zero
    private var dragStartOffset = NSPoint.zero
    /// Released this close to home, the window snaps back (magnet).
    private static let snapRadius: CGFloat = 90
    private var pointerTimer: Timer?
    /// Native glass shapes under the transparent page, keyed by the page's shape id ("p" pill, "c" card).
    private var glassViews: [String: NSView] = [:]
    private var container: NSView?
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
            panel.setFrame(placed(Self.frame(for: cardSize, on: panel.screen ?? NSScreen.main)), display: true, animate: false)
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

    /// Real (refracting) glass under the page: `value` is "id:x,y,w,h,r,alpha|..." in page points.
    /// The page cannot render it itself: its backdrop-filter sees nothing through a transparent window.
    func setGlass(_ value: String) {
        guard let container, let webView else { return }
        var seen = Set<String>()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for part in value.split(separator: "|") {
            let kv = part.split(separator: ":", maxSplits: 1)
            guard kv.count == 2 else { continue }
            let id = String(kv[0])
            let n = kv[1].split(separator: ",").compactMap { Double($0) }
            guard n.count == 6, n.allSatisfy(\.isFinite), n[2] > 0, n[3] > 0 else { continue }
            seen.insert(id)
            let v = glassViews[id] ?? {
                let v = Self.makeGlass(pill: id == "p")
                container.addSubview(v, positioned: .below, relativeTo: webView)
                glassViews[id] = v
                return v
            }()
            v.frame = NSRect(x: n[0], y: n[1], width: n[2], height: n[3])
            Self.setCorner(v, radius: n[4])
            v.alphaValue = min(max(n[5], 0), 1)
            v.isHidden = false
        }
        for (id, v) in glassViews where !seen.contains(id) { v.isHidden = true }
        CATransaction.commit()
    }

    private static func makeGlass(pill: Bool) -> NSView {
        if #available(macOS 26.0, *) {
            let g = NSGlassEffectView()
            // Both use .regular (frosted: a light blur so the card is not see-through) with a
            // light smoke tint; a heavier tint hides the edge refraction, so keep it low.
            g.style = .regular
            g.tintColor = NSColor(white: 0, alpha: pill ? 0.38 : 0.26)
            if #available(macOS 27.0, *) { g.effectIsInteractive = pill }
            return g
        }
        let v = NSVisualEffectView()
        v.material = .hudWindow
        v.blendingMode = .behindWindow
        v.state = .active
        v.wantsLayer = true
        v.layer?.masksToBounds = true
        return v
    }

    private static func setCorner(_ v: NSView, radius r: Double) {
        let radius = CGFloat(r)
        if #available(macOS 26.0, *), let g = v as? NSGlassEffectView {
            if g.cornerRadius != radius { g.cornerRadius = radius }
        } else if v.layer?.cornerRadius != radius {
            v.layer?.cornerRadius = radius
        }
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
        panel.setFrame(placed(Self.frame(for: cardSize, on: panel.screen ?? NSScreen.main)), display: true, animate: false)
    }

    private func placed(_ home: NSRect) -> NSRect { home.offsetBy(dx: dragOffset.x, dy: dragOffset.y) }

    /// Pill drag from the page: "begin", "dx,dy" (screen points since begin, y down), "end".
    func drag(_ value: String) {
        guard let panel else { return }
        let home = Self.frame(for: cardSize, on: panel.screen ?? NSScreen.main)
        switch value {
        case "begin":
            dragStartOffset = dragOffset
        case "end":
            if hypot(dragOffset.x, dragOffset.y) < Self.snapRadius {
                dragOffset = .zero
                NSAnimationContext.runAnimationGroup { ctx in
                    ctx.duration = 0.28
                    ctx.timingFunction = CAMediaTimingFunction(controlPoints: 0.34, 1.3, 0.64, 1)
                    panel.animator().setFrame(home, display: true)
                }
            }
        default:
            let p = value.split(separator: ",").compactMap { Double($0) }
            guard p.count == 2, p[0].isFinite, p[1].isFinite else { return }
            var o = NSPoint(x: dragStartOffset.x + p[0], y: dragStartOffset.y - p[1])
            // Light magnet while dragging: very close to home sticks to it.
            if hypot(o.x, o.y) < 24 { o = .zero }
            // Keep the pill (top of the window) on screen.
            let vf = (panel.screen ?? NSScreen.main)?.visibleFrame ?? home
            o.x = min(max(o.x, vf.minX - home.minX - home.width / 2 + 120), vf.maxX - home.maxX + home.width / 2 - 120)
            o.y = min(max(o.y, vf.minY - home.maxY + 60), 0)
            dragOffset = o
            panel.setFrameOrigin(NSPoint(x: home.minX + o.x, y: home.minY + o.y))
        }
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
        // Glass views sit under the transparent page (see setGlass); flipped so frames use page coordinates.
        let root = FlippedView()
        wv.frame = root.bounds
        wv.autoresizingMask = [.width, .height]
        root.addSubview(wv)
        container = root
        p.contentView = root
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

private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

/// Borderless panels refuse key status by default; the card needs it for its language pickers.
private final class CopilotPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    // Glass and vibrancy draw their dimmed "inactive" look once the window resigns key/main (a
    // click anywhere else). The overlay floats over other apps, so it always reports itself active.
    override var isKeyWindow: Bool { true }
    override var isMainWindow: Bool { true }
    // Faking key status above made AppKit offer ⌘V/⌘C/⌘A to this panel's web view even while the user
    // typed in the main window, so paste landed nowhere. Track the real key state and only take
    // key equivalents when the panel truly holds the keyboard.
    private var reallyKey = false
    override func becomeKey() { reallyKey = true; super.becomeKey() }
    override func resignKey() { reallyKey = false; super.resignKey() }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        reallyKey ? super.performKeyEquivalent(with: event) : false
    }
    // Key/main is not enough: AppKit greys glass whenever the APP is inactive (another app in
    // front), which is the normal state for this overlay. These AppKit-internal hooks decide the
    // "active appearance"; overriding them keeps the glass lit. Private selectors: if a future
    // macOS drops them, nothing calls these and the glass simply dims again (no crash).
    @objc func _hasActiveAppearance() -> Bool { true }
    @objc func _hasActiveAppearanceIgnoringKeyFocus() -> Bool { true }
    @objc func _hasKeyAppearance() -> Bool { true }
}
