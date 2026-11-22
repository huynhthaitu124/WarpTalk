import Foundation

/// Which URL(s) the web view tries, in order. Pure (unit-tested).
///   1. the configured URL (default `http://localhost:3000`, the Next.js dev server),
///   2. the fallback list (default `http://localhost:5173`, the old Vite dev server),
///   3. the web export bundled inside the app (`warptalk://app/`), when the build included one.
enum WebURLPlan {
    static let bundledScheme = "warptalk"
    static let bundledHost = "app"
    static let bundledURL = "warptalk://app/"

    /// Splits a user-typed list (newline, comma or whitespace separated).
    static func parseList(_ raw: String) -> [String] {
        raw.split(whereSeparator: { $0 == "\n" || $0 == "," || $0 == " " || $0 == "\t" })
            .map { String($0).trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    /// Valid, de-duplicated candidates (http, https or the bundled scheme only), primary first.
    static func candidates(primary: String, fallbacks: [String], bundledAvailable: Bool) -> [URL] {
        var seen = Set<String>()
        var out: [URL] = []
        var all = [primary] + fallbacks
        if bundledAvailable { all.append(bundledURL) }
        for raw in all {
            let t = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            guard let u = URL(string: t), let scheme = u.scheme?.lowercased(), u.host != nil,
                  ["http", "https", bundledScheme].contains(scheme) else { continue }
            if seen.insert(u.absoluteString).inserted { out.append(u) }
        }
        return out
    }

    static func isBundled(_ url: URL) -> Bool {
        url.scheme?.lowercased() == bundledScheme && url.host?.lowercased() == bundledHost
    }

    /// Navigation errors worth trying the next candidate for (server not running / unreachable).
    /// `NSURLErrorCancelled` (-999) is a superseded navigation, never a failure.
    static func shouldTryNext(errorDomain: String, code: Int) -> Bool {
        guard errorDomain == NSURLErrorDomain else { return false }
        return [NSURLErrorCannotConnectToHost, NSURLErrorCannotFindHost, NSURLErrorTimedOut,
                NSURLErrorNetworkConnectionLost, NSURLErrorDNSLookupFailed].contains(code)
    }

    static func isCancellation(errorDomain: String, code: Int) -> Bool {
        errorDomain == NSURLErrorDomain && code == NSURLErrorCancelled
    }
}

/// Values the web app reads from `window.warptalkNative.config` (see BRIDGE.md).
struct WebRuntimeConfig: Equatable {
    /// Base of the backend API WITHOUT `/api/v1`. Bundled mode: `warptalk://app` (same-origin proxy to the backend URL
    /// from Settings, so no CORS); URL mode: the backend URL from Settings.
    var apiBaseUrl: String
    var bundled: Bool

    static func make(activeWebURL: URL?, backendURL: String) -> WebRuntimeConfig {
        var backend = backendURL.trimmingCharacters(in: .whitespacesAndNewlines)
        while backend.hasSuffix("/") { backend.removeLast() }
        if let u = activeWebURL, WebURLPlan.isBundled(u) {
            return WebRuntimeConfig(apiBaseUrl: "warptalk://app", bundled: true)
        }
        return WebRuntimeConfig(apiBaseUrl: backend, bundled: false)
    }

    var json: String {
        let d: [String: Any] = ["version": 1, "apiBaseUrl": apiBaseUrl, "bundled": bundled]
        guard let data = try? JSONSerialization.data(withJSONObject: d, options: [.sortedKeys]),
              let s = String(data: data, encoding: .utf8) else { return "{}" }
        return s
    }
}
