import XCTest
@testable import WarpTalk

final class WebURLPlanTests: XCTestCase {
    func testDefaultsPreferNextThenViteThenBundle() {
        let c = WebURLPlan.candidates(primary: SettingsStoreDefaults.web, fallbacks: WebURLPlan.parseList(SettingsStoreDefaults.fallbacks), bundledAvailable: true)
        XCTAssertEqual(c.map(\.absoluteString), ["http://localhost:3000", "http://localhost:5173", "warptalk://app/"])
    }

    func testDedupInvalidAndBundleOptional() {
        let c = WebURLPlan.candidates(primary: " http://localhost:3000 ", fallbacks: ["http://localhost:3000", "ftp://x", "nonsense", "https://app.example.com"], bundledAvailable: false)
        XCTAssertEqual(c.map(\.absoluteString), ["http://localhost:3000", "https://app.example.com"])
        XCTAssertTrue(WebURLPlan.candidates(primary: "", fallbacks: [], bundledAvailable: false).isEmpty)
    }

    func testParseList() {
        XCTAssertEqual(WebURLPlan.parseList("a, b\nc  d,,"), ["a", "b", "c", "d"])
    }

    func testFallbackOnlyForUnreachableNotForCancel() {
        XCTAssertTrue(WebURLPlan.shouldTryNext(errorDomain: NSURLErrorDomain, code: NSURLErrorCannotConnectToHost))
        XCTAssertFalse(WebURLPlan.shouldTryNext(errorDomain: NSURLErrorDomain, code: NSURLErrorCancelled))
        XCTAssertFalse(WebURLPlan.shouldTryNext(errorDomain: "WebKitErrorDomain", code: 102))
        XCTAssertTrue(WebURLPlan.isCancellation(errorDomain: NSURLErrorDomain, code: NSURLErrorCancelled))
    }

    func testRuntimeConfig() {
        XCTAssertEqual(WebRuntimeConfig.make(activeWebURL: URL(string: "warptalk://app/index.html"), backendURL: "http://x:1"), WebRuntimeConfig(apiBaseUrl: "warptalk://app", bundled: true))
        XCTAssertEqual(WebRuntimeConfig.make(activeWebURL: URL(string: "http://localhost:3000"), backendURL: " http://x:1// "), WebRuntimeConfig(apiBaseUrl: "http://x:1", bundled: false))
    }

    /// Same literals as `SettingsStore` (which is main-actor isolated).
    private enum SettingsStoreDefaults {
        static let web = "http://localhost:3000"
        static let fallbacks = "http://localhost:5173"
    }
}

final class BundledWebResolverTests: XCTestCase {
    private var root: URL!

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("warptalk-web-\(UUID().uuidString)")
        let fm = FileManager.default
        try fm.createDirectory(at: root.appendingPathComponent("_next/static/chunks"), withIntermediateDirectories: true)
        try fm.createDirectory(at: root.appendingPathComponent("meetings"), withIntermediateDirectories: true)
        for (path, body) in ["index.html": "<html>home</html>", "login.html": "<html>login</html>", "meetings/index.html": "m",
                             "_next/static/chunks/app.js": "js", "logo.svg": "<svg/>"] {
            try body.write(to: root.appendingPathComponent(path), atomically: true, encoding: .utf8)
        }
    }

    override func tearDownWithError() throws { try? FileManager.default.removeItem(at: root) }

    private func file(_ path: String) -> (String, String)? {
        if case .file(let u, let mime) = BundledWebResolver(root: root).resolve(path: path) { return (u.lastPathComponent, mime) }
        return nil
    }

    func testRootAndRoutes() {
        XCTAssertEqual(file("/")?.0, "index.html")
        XCTAssertEqual(file("")?.0, "index.html")
        XCTAssertEqual(file("/login")?.0, "login.html")                 // Next export: <route>.html
        XCTAssertEqual(file("/meetings")?.0, "index.html")             // directory -> index.html
        XCTAssertEqual(file("/rooms/some-id")?.0, "index.html")        // unknown extensionless -> client-side route
    }

    func testStaticAssetsAndMime() {
        XCTAssertEqual(file("/_next/static/chunks/app.js")?.1, "text/javascript; charset=utf-8")
        XCTAssertEqual(file("/logo.svg")?.1, "image/svg+xml")
        XCTAssertEqual(file("/index.html")?.1, "text/html; charset=utf-8")
        XCTAssertEqual(BundledWebResolver.mime(for: "woff2"), "font/woff2")
        XCTAssertEqual(BundledWebResolver.mime(for: "bin"), "application/octet-stream")
    }

    func testMissingAssetIs404NotIndex() {
        XCTAssertEqual(BundledWebResolver(root: root).resolve(path: "/_next/static/chunks/missing.js"), .notFound)
    }

    func testTraversalIsRejected() {
        XCTAssertEqual(BundledWebResolver(root: root).resolve(path: "/../secret.txt"), .notFound)
        XCTAssertEqual(BundledWebResolver(root: root).resolve(path: "/a/../../etc/passwd"), .notFound)
        XCTAssertEqual(BundledWebResolver(root: root).resolve(path: "/%2e%2e/secret"), .notFound)
    }
}

final class APIProxyTests: XCTestCase {
    private func req(_ url: String, method: String = "GET", headers: [String: String] = [:]) -> URLRequest {
        var r = URLRequest(url: URL(string: url)!)
        r.httpMethod = method
        for (k, v) in headers { r.setValue(v, forHTTPHeaderField: k) }
        return r
    }

    func testMapsPathQueryMethodAndBody() throws {
        let body = Data(#"{"a":1}"#.utf8)
        let r = try XCTUnwrap(APIProxy.upstreamRequest(
            for: req("warptalk://app/api/v1/translation-rooms/r1?x=1&y=a%20b", method: "POST",
                     headers: ["Authorization": "Bearer t", "Content-Type": "application/json", "Origin": "warptalk://app", "Content-Length": "7"]),
            backendURL: "http://localhost:8080/", body: body))
        XCTAssertEqual(r.url?.absoluteString, "http://localhost:8080/api/v1/translation-rooms/r1?x=1&y=a%20b")
        XCTAssertEqual(r.httpMethod, "POST"); XCTAssertEqual(r.httpBody, body)
        XCTAssertEqual(r.value(forHTTPHeaderField: "Authorization"), "Bearer t")
        XCTAssertNil(r.value(forHTTPHeaderField: "Origin")); XCTAssertNil(r.value(forHTTPHeaderField: "Content-Length"))
    }

    func testRejectsNonAPIPathsAndEmptyBackend() {
        XCTAssertNil(APIProxy.upstreamRequest(for: req("warptalk://app/index.html"), backendURL: "http://x", body: nil))
        XCTAssertNil(APIProxy.upstreamRequest(for: req("warptalk://app/api/v1/x"), backendURL: "  ", body: nil))
        XCTAssertTrue(APIProxy.isAPIPath("/api/v1/x")); XCTAssertFalse(APIProxy.isAPIPath("/apix"))
    }

    func testBackendWithBasePath() throws {
        let r = try XCTUnwrap(APIProxy.upstreamRequest(for: req("warptalk://app/api/v1/x"), backendURL: "https://gw.example.com/warptalk", body: nil))
        XCTAssertEqual(r.url?.absoluteString, "https://gw.example.com/warptalk/api/v1/x")
    }
}
