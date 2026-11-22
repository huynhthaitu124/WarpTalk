import Foundation
import WebKit

/// Maps a `warptalk://app/<path>` request onto the bundled Next.js static export (`Contents/Resources/web`). Pure
/// apart from `FileManager` reads, so it is unit-tested against a temp directory.
///
/// Next `output: 'export'` emits `index.html`, `<route>.html` (or `<route>/index.html` with `trailingSlash`), `404.html`
/// and `_next/static/**`. Rules, in order: exact file; directory -> its `index.html`; extensionless -> `<path>.html`;
/// extensionless and still unknown -> root `index.html` (client-side routes); otherwise 404.
struct BundledWebResolver {
    let root: URL
    var fileManager: FileManager = .default

    enum Resolution: Equatable {
        case file(URL, mime: String)
        case notFound
    }

    /// `path` is `URL.path` of the request (percent-decoded, starts with "/").
    func resolve(path rawPath: String) -> Resolution {
        let decoded = rawPath.removingPercentEncoding ?? rawPath
        var comps: [String] = []
        for part in decoded.split(separator: "/", omittingEmptySubsequences: true) {
            if part == "." { continue }
            if part == ".." { return .notFound }          // no traversal, not even "normalised" traversal
            comps.append(String(part))
        }
        let rel = comps.joined(separator: "/")

        func isFile(_ rel: String) -> URL? {
            let u = rel.isEmpty ? root : root.appendingPathComponent(rel)
            var isDir: ObjCBool = false
            guard fileManager.fileExists(atPath: u.path, isDirectory: &isDir) else { return nil }
            if isDir.boolValue {
                let idx = u.appendingPathComponent("index.html")
                return fileManager.fileExists(atPath: idx.path) ? idx : nil
            }
            return u
        }

        if let f = isFile(rel) { return .file(f, mime: Self.mime(for: f.pathExtension)) }
        let last = comps.last ?? ""
        if !last.contains(".") {
            if let f = isFile(rel + ".html") { return .file(f, mime: Self.mime(for: "html")) }
            if let f = isFile("index.html") { return .file(f, mime: Self.mime(for: "html")) }
        }
        return .notFound
    }

    static func mime(for ext: String) -> String {
        switch ext.lowercased() {
        case "html", "htm": return "text/html; charset=utf-8"
        case "js", "mjs": return "text/javascript; charset=utf-8"
        case "css": return "text/css; charset=utf-8"
        case "json", "map", "webmanifest": return "application/json; charset=utf-8"
        case "txt": return "text/plain; charset=utf-8"
        case "svg": return "image/svg+xml"
        case "png": return "image/png"
        case "jpg", "jpeg": return "image/jpeg"
        case "gif": return "image/gif"
        case "webp": return "image/webp"
        case "avif": return "image/avif"
        case "ico": return "image/x-icon"
        case "woff": return "font/woff"
        case "woff2": return "font/woff2"
        case "ttf": return "font/ttf"
        case "otf": return "font/otf"
        case "wasm": return "application/wasm"
        case "mp3": return "audio/mpeg"
        case "wav": return "audio/wav"
        default: return "application/octet-stream"
        }
    }

    /// `Contents/Resources/web` when the build bundled an export (needs `index.html`).
    static func bundledRoot(bundle: Bundle = .main) -> URL? {
        guard let res = bundle.resourceURL else { return nil }
        let web = res.appendingPathComponent("web", isDirectory: true)
        return FileManager.default.fileExists(atPath: web.appendingPathComponent("index.html").path) ? web : nil
    }
}

/// `warptalk://app/api/...` -> `{backendURL}/api/...` (same-origin proxy for the bundled web app, so the backend needs no
/// CORS entry for the custom scheme). Pure request mapping, unit-tested.
enum APIProxy {
    static let prefix = "/api/"

    static func isAPIPath(_ path: String) -> Bool { path.hasPrefix(prefix) }

    static func upstreamRequest(for request: URLRequest, backendURL: String, body: Data?) -> URLRequest? {
        guard let url = request.url, isAPIPath(url.path) else { return nil }
        var base = backendURL.trimmingCharacters(in: .whitespacesAndNewlines)
        while base.hasSuffix("/") { base.removeLast() }
        guard !base.isEmpty, var comps = URLComponents(string: base) else { return nil }
        let basePath = comps.path
        comps.path = (basePath.hasSuffix("/") ? String(basePath.dropLast()) : basePath) + url.path
        comps.percentEncodedQuery = URLComponents(url: url, resolvingAgainstBaseURL: false)?.percentEncodedQuery
        guard let upstream = comps.url else { return nil }
        var out = URLRequest(url: upstream)
        out.httpMethod = request.httpMethod
        out.timeoutInterval = 30
        for (k, v) in request.allHTTPHeaderFields ?? [:] {
            let lk = k.lowercased()
            if lk == "origin" || lk == "referer" || lk == "host" || lk == "content-length" { continue }
            out.setValue(v, forHTTPHeaderField: k)
        }
        out.httpBody = body
        return out
    }
}

/// `WKURLSchemeHandler` for `warptalk://`: static files from the bundle + the API proxy.
/// Header: WKURLSchemeHandler.h (macOS 10.13+). After `stop`, NO callback may be made on the task (WebKit throws), hence `active`.
/// WKURLSchemeTask callbacks are not documented as thread-safe: everything is delivered on the main queue.
final class WarpTalkSchemeHandler: NSObject, WKURLSchemeHandler {
    private let resolver: () -> BundledWebResolver?
    private let backendURL: () -> String
    private let session: URLSession
    /// Tasks WebKit has started and not stopped; main thread only.
    private var active = Set<ObjectIdentifier>()
    private var upstream: [ObjectIdentifier: URLSessionDataTask] = [:]

    init(resolver: @escaping () -> BundledWebResolver? = { BundledWebResolver.bundledRoot().map { BundledWebResolver(root: $0) } },
         backendURL: @escaping () -> String,
         session: URLSession = .shared) {
        self.resolver = resolver
        self.backendURL = backendURL
        self.session = session
    }

    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        let id = ObjectIdentifier(urlSchemeTask)
        active.insert(id)
        let request = urlSchemeTask.request
        guard let url = request.url else { return finish(urlSchemeTask, id, status: 400, body: Data("bad request".utf8)) }

        if APIProxy.isAPIPath(url.path) {
            var body = request.httpBody
            if body == nil, let stream = request.httpBodyStream { body = Self.drain(stream) }   // VERIFY (README): POST bodies via scheme tasks
            guard let up = APIProxy.upstreamRequest(for: request, backendURL: backendURL(), body: body) else {
                return finish(urlSchemeTask, id, status: 502, body: Data("backend URL not configured".utf8))
            }
            let task = session.dataTask(with: up) { [weak self] data, resp, err in
                DispatchQueue.main.async {
                    guard let self, self.active.contains(id) else { return }
                    if let http = resp as? HTTPURLResponse, let data {
                        // Same-origin from the page's point of view: pass status and headers through unchanged.
                        let r = HTTPURLResponse(url: url, statusCode: http.statusCode, httpVersion: "HTTP/1.1",
                                                headerFields: Self.safeHeaders(http.allHeaderFields)) ?? http
                        urlSchemeTask.didReceive(r)
                        urlSchemeTask.didReceive(data)
                        urlSchemeTask.didFinish()
                    } else {
                        urlSchemeTask.didFailWithError(err ?? URLError(.badServerResponse))
                    }
                    self.active.remove(id)
                    self.upstream[id] = nil
                }
            }
            upstream[id] = task
            task.resume()
            return
        }

        guard let resolver = resolver() else {
            return finish(urlSchemeTask, id, status: 404, mime: "text/plain; charset=utf-8",
                          body: Data("Chưa có web bundle trong app. Chạy `make bundle` (apps/macos) hoặc đổi URL trong Settings.".utf8))
        }
        switch resolver.resolve(path: url.path) {
        case .file(let file, let mime):
            if let data = try? Data(contentsOf: file) { finish(urlSchemeTask, id, status: 200, mime: mime, body: data) }
            else { finish(urlSchemeTask, id, status: 404, body: Data("not found".utf8)) }
        case .notFound:
            finish(urlSchemeTask, id, status: 404, body: Data("not found".utf8))
        }
    }

    func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {
        let id = ObjectIdentifier(urlSchemeTask)
        upstream[id]?.cancel()
        upstream[id] = nil
        active.remove(id)
    }

    private func finish(_ task: WKURLSchemeTask, _ id: ObjectIdentifier, status: Int,
                        mime: String = "text/plain; charset=utf-8", body: Data) {
        guard active.contains(id), let url = task.request.url else { return }
        let headers = ["Content-Type": mime, "Content-Length": String(body.count), "Cache-Control": "no-cache"]
        if let r = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: headers) {
            task.didReceive(r)
        }
        task.didReceive(body)
        task.didFinish()
        active.remove(id)
    }

    private static func safeHeaders(_ h: [AnyHashable: Any]) -> [String: String] {
        var out: [String: String] = [:]
        for (k, v) in h {
            guard let k = k as? String, let v = v as? String else { continue }
            let lk = k.lowercased()
            if lk == "content-encoding" || lk == "transfer-encoding" || lk == "content-length" { continue }   // URLSession already decoded the body
            out[k] = v
        }
        return out
    }

    private static func drain(_ stream: InputStream) -> Data {
        stream.open(); defer { stream.close() }
        var data = Data()
        var buf = [UInt8](repeating: 0, count: 16 * 1024)
        while stream.hasBytesAvailable {
            let n = stream.read(&buf, maxLength: buf.count)
            if n <= 0 { break }
            data.append(buf, count: n)
        }
        return data
    }
}
