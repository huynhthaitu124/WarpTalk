import AppKit
import SwiftUI
import WebKit

/// Owns the WKWebView that hosts the web app, and the bridge wiring.
///
/// Loads `settings.webURL`, then the fallback URLs, then the bundled export (`warptalk://app/`) when one is in the app.
/// The URL that answered becomes `settings.activeWebURL`, the only origin allowed to use the native bridge.
@MainActor
final class WebViewModel: NSObject, ObservableObject, WKNavigationDelegate, WKUIDelegate {
    let webView: WKWebView
    @Published var loadError: String?
    @Published var isLoading = false
    let hasBundledWeb: Bool
    private let settings: SettingsStore
    private let userContent: WKUserContentController
    private var candidates: [URL] = []
    private var candidateIndex = 0

    init(settings: SettingsStore, bridge: NativeBridge) {
        self.settings = settings
        hasBundledWeb = BundledWebResolver.bundledRoot() != nil
        let cfg = WKWebViewConfiguration()
        cfg.mediaTypesRequiringUserActionForPlayback = []     // remote audio (LiveKit) must autoplay
        // `warptalk://app/...`: bundled static export + same-origin API proxy (WKURLSchemeHandler.h, macOS 10.13+).
        // Must be set before the web view is created.
        // VERIFY: whether WebKit treats pages of this custom scheme as a secure context (needed for getUserMedia / crypto.subtle).
        // If `navigator.mediaDevices` is undefined under warptalk://app, use the http://localhost URL instead (README mục 3).
        cfg.setURLSchemeHandler(WarpTalkSchemeHandler(backendURL: { [weak settings] in
            MainActor.assumeIsolated { settings?.backendURL ?? "" }
        }), forURLScheme: WebURLPlan.bundledScheme)
        userContent = cfg.userContentController
        Self.register(replyHandler: bridge, on: cfg.userContentController)
        let wv = WKWebView(frame: .zero, configuration: cfg)
        if #available(macOS 13.3, *) { wv.isInspectable = true }   // WKWebView.h: isInspectable macOS 13.3+. Safari > Develop to inspect (dev)
        webView = wv
        super.init()
        bridge.webView = webView
        webView.navigationDelegate = self
        webView.uiDelegate = self
        load()
    }

    /// `addScriptMessageHandlerWithReply:contentWorld:name:` (WKUserContentController.h:98, macOS 11+) takes a
    /// `WKScriptMessageHandlerWithReply`; the plain `addScriptMessageHandler:contentWorld:name:` takes a different
    /// protocol and would not accept `NativeBridge`. The header gives no `NS_SWIFT_NAME`, so rather than guess the
    /// Swift spelling we call the selector through the Objective-C runtime (same behaviour, always compiles).
    static func register(replyHandler: NativeBridge, on ucc: WKUserContentController) {
        let sel = NSSelectorFromString("addScriptMessageHandlerWithReply:contentWorld:name:")
        guard ucc.responds(to: sel), let imp = ucc.method(for: sel) else {
            NSLog("[web] addScriptMessageHandlerWithReply unavailable: native bridge disabled")
            return
        }
        typealias Fn = @convention(c) (AnyObject, Selector, AnyObject, AnyObject, NSString) -> Void
        let call = unsafeBitCast(imp, to: Fn.self)
        call(ucc, sel, replyHandler, WKContentWorld.page, NativeBridge.handlerName as NSString)
    }

    /// Re-injected on every load so the page sees the current `warptalkNative.config` (API base).
    private func installScript(for url: URL) {
        let ucc = userContent
        ucc.removeAllUserScripts()
        let config = WebRuntimeConfig.make(activeWebURL: url, backendURL: settings.backendURL)
        ucc.addUserScript(WKUserScript(source: BridgeScript.make(config: config), injectionTime: .atDocumentStart, forMainFrameOnly: true))
    }

    /// Starts again from the configured URL.
    func load() {
        candidates = WebURLPlan.candidates(primary: settings.webURL, fallbacks: WebURLPlan.parseList(settings.webFallbackURLs),
                                           bundledAvailable: hasBundledWeb)
        candidateIndex = 0
        guard !candidates.isEmpty else {
            loadError = "URL web không hợp lệ: \(settings.webURL)"; return
        }
        loadError = nil
        loadCandidate()
    }

    private func loadCandidate() {
        let url = candidates[candidateIndex]
        settings.setActiveWebURL(url.absoluteString)
        installScript(for: url)
        webView.load(URLRequest(url: url))
    }

    func reload() {
        if webView.url == nil { load() } else { webView.reload() }
    }

    // MARK: WKNavigationDelegate
    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) { isLoading = true }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { isLoading = false; loadError = nil }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { fail(error) }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { fail(error) }
    private func fail(_ error: Error) {
        let ns = error as NSError
        if WebURLPlan.isCancellation(errorDomain: ns.domain, code: ns.code) { return }   // superseded navigation
        if candidateIndex + 1 < candidates.count, WebURLPlan.shouldTryNext(errorDomain: ns.domain, code: ns.code) {
            candidateIndex += 1
            loadCandidate()
            return
        }
        isLoading = false
        let tried = candidates.map(\.absoluteString).joined(separator: ", ")
        loadError = "Không tải được \(tried): \(error.localizedDescription). Hãy chạy web app (cd apps/web && npm run dev, cổng 3000) rồi bấm Tải lại."
    }

    // MARK: WKUIDelegate

    /// getUserMedia from the web app (native meetings publish the mic from the web view). Trusted origin only.
    /// Signature: WKUIDelegate.h:160 `webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:`
    /// (macOS 12+). The system mic prompt still applies (NSMicrophoneUsageDescription).
    func webView(_ webView: WKWebView, requestMediaCapturePermissionFor origin: WKSecurityOrigin,
                 initiatedByFrame frame: WKFrameInfo, type: WKMediaCaptureType,
                 decisionHandler: @escaping (WKPermissionDecision) -> Void) {
        let ok = OriginPolicy.isTrusted(scheme: origin.protocol, host: origin.host, port: origin.port, allowed: settings.activeWebURL)
        decisionHandler(ok ? .grant : .deny)
    }

    /// target=_blank / window.open: send to the default browser instead of spawning web views.
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = navigationAction.request.url { NSWorkspace.shared.open(url) }
        return nil
    }
}

struct WebContainerView: View {
    @EnvironmentObject var web: WebViewModel

    var body: some View {
        ZStack {
            WebViewRepresentable(webView: web.webView)
            if let e = web.loadError {
                VStack(spacing: 12) {
                    Image(systemName: "exclamationmark.triangle").font(.largeTitle)
                    Text(e).multilineTextAlignment(.center).frame(maxWidth: 480)
                    HStack {
                        Button("Tải lại") { web.load() }.keyboardShortcut(.defaultAction)
                        SettingsLink { Text("Cài đặt…") }
                    }
                }
                .padding(24)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
            }
        }
    }
}

struct WebViewRepresentable: NSViewRepresentable {
    let webView: WKWebView
    func makeNSView(context: Context) -> WKWebView { webView }
    func updateNSView(_ nsView: WKWebView, context: Context) {}
}
