import Foundation

/// Minimal REST client for what the NATIVE side still needs. The web app owns auth, rooms and meeting UI and passes
/// its access token in `startExternalCapture` / `updateAuth`; this client never refreshes tokens. On 401 the caller
/// emits `auth-required` so the web app can push a fresh token via `updateAuth`.
struct MeetingGrant: Decodable, Equatable {
    let roomName: String
    let serverUrl: String
    let token: String
    let participantIdentity: String
}

struct BridgeHeartbeatRequest: Encodable {
    let audioFlowing: Bool
    let lastAudioAt: Date?
    let rmsDbfs: Double?
}

/// `{ "code": "...", "message": "..." }` from warptalk_kernel::AppError.
struct ErrorBody: Decodable { let code: String; let message: String }

final class NativeAPI: @unchecked Sendable {
    private let baseURL: String
    private let lock = NSLock()
    private var token: String
    private let session: URLSession

    init(baseURL: String, accessToken: String, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.token = accessToken
        self.session = session
    }

    func updateToken(_ t: String) { lock.lock(); token = t; lock.unlock() }
    private var currentToken: String { lock.lock(); defer { lock.unlock() }; return token }

    static func httpError(_ status: Int, _ data: Data) -> APIError {
        if let b = try? JSONDecoder().decode(ErrorBody.self, from: data) { return .http(status: status, code: b.code, message: b.message) }
        let text = String(data: data, encoding: .utf8)?.prefix(200) ?? ""
        return .http(status: status, code: "HTTP_\(status)", message: text.isEmpty ? "Lỗi server" : String(text))
    }

    func makeURL(_ path: String) throws -> URL {
        var base = baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        while base.hasSuffix("/") { base.removeLast() }
        guard let u = URL(string: base + "/api/v1" + path) else { throw APIError.invalidURL(base + path) }
        return u
    }

    private func send(_ method: String, _ path: String, body: Data? = nil) async throws -> Data {
        var req = URLRequest(url: try makeURL(path))
        req.httpMethod = method
        req.timeoutInterval = 15
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        req.setValue("Bearer \(currentToken)", forHTTPHeaderField: "Authorization")
        if let body { req.httpBody = body; req.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        let data: Data, resp: URLResponse
        do { (data, resp) = try await session.data(for: req) } catch { throw APIError.transport(error) }
        guard let http = resp as? HTTPURLResponse else { throw APIError.transport(URLError(.badServerResponse)) }
        guard (200..<300).contains(http.statusCode) else { throw Self.httpError(http.statusCode, data) }
        return data
    }

    func bridgeToken(roomId: String) async throws -> MeetingGrant {
        try decode(try await send("POST", "/meetings/\(roomId)/bridge-token"))
    }

    /// Subscribe-only grant (used only for voice mode, to receive the dub track). `clientTag` makes the server mint
    /// identity `{userId}:{clientTag}`, so this connection never evicts the web client of the same user (contract B-09).
    /// VERIFY: backend rejects unknown tags with 400 INVALID_CLIENT_TAG; "bridge-listener" is the agreed value.
    func meetingJoin(roomId: String, clientTag: String = "bridge-listener") async throws -> MeetingGrant {
        let body = try JSONSerialization.data(withJSONObject: ["clientTag": clientTag])
        return try decode(try await send("POST", "/meetings/\(roomId)/join", body: body))
    }

    /// `POST /translation-rooms/{id}/bridge/capturer/heartbeat` (spec shape; server side being added).
    func heartbeat(roomId: String, _ req: BridgeHeartbeatRequest) async throws {
        let body = try APIDates.encoder().encode(req)
        _ = try await send("POST", "/translation-rooms/\(roomId)/bridge/capturer/heartbeat", body: body)
    }

    private func decode<T: Decodable>(_ data: Data) throws -> T {
        do { return try APIDates.decoder().decode(T.self, from: data) }
        catch { throw APIError.decoding(error, body: String(data: data, encoding: .utf8) ?? "") }
    }
}
