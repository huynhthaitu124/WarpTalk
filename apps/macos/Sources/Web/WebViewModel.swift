import AppKit
import SwiftUI
import WebKit

/// Owns the WKWebView that hosts the web app, and the bridge wiring.
@MainActor
final class WebViewModel: NSObject, ObservableObject, WKNavigationDelegate, WKUIDelegate {
    let webView: WKWebView
    @Published var loadError: String?
    @Published var isLoading = false
    private let settings: SettingsStore

    init(settings: SettingsStore, bridge: NativeBridge) {
        self.settings = settings
        let cfg = WKWebViewConfiguration()
        let script = WKUserScript(source: BridgeScript.source, injectionTime: .atDocumentStart, forMainFrameOnly: true)
        cfg.userContentController.addUserScript(script)
        cfg.userContentController.addScriptMessageHandler(bridge, contentWorld: .page, name: NativeBridge.handlerName)
        cfg.mediaTypesRequiringUserActionForPlayback = []     // remote audio (LiveKit) must autoplay
        let wv = WKWebView(frame: .zero, configuration: cfg)
        if #available(macOS 13.3, *) { wv.isInspectable = true }   // Safari > Develop to inspect (dev)
        webView = wv
        super.init()
        bridge.webView = webView
        webView.navigationDelegate = self
        webView.uiDelegate = self
        load()
    }

    func load() {
        guard let url = URL(string: settings.webURL.trimmingCharacters(in: .whitespaces)), url.scheme != nil else {
            loadError = "URL web không hợp lệ: \(settings.webURL)"; return
        }
        loadError = nil
        webView.load(URLRequest(url: url))
    }

    func reload() { webView.url == nil ? load() : webView.reload() }

    // MARK: WKNavigationDelegate
    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) { isLoading = true }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { isLoading = false; loadError = nil }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { fail(error) }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { fail(error) }
    private func fail(_ error: Error) {
        isLoading = false
        loadError = "Không tải được \(settings.webURL): \(error.localizedDescription). Hãy chạy web app (npm run dev) rồi bấm Tải lại."
    }

    // MARK: WKUIDelegate

    /// getUserMedia from the web app (native meetings publish the mic from the web view). Trusted origin only.
    /// VERIFY: macOS 12+ delegate signature; the system mic prompt still applies (NSMicrophoneUsageDescription).
    func webView(_ webView: WKWebView, requestMediaCapturePermissionFor origin: WKSecurityOrigin,
                 initiatedByFrame frame: WKFrameInfo, type: WKMediaCaptureType,
                 decisionHandler: @escaping (WKPermissionDecision) -> Void) {
        let ok = OriginPolicy.isTrusted(scheme: origin.protocol, host: origin.host, port: origin.port, allowed: settings.webURL)
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
