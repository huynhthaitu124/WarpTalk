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

/// Body of `POST /translation-rooms/{id}/bridge/capturer/heartbeat`: `{ audioFlowing, lastAudioAt, rmsDbfs }`.
/// (crates/modules/room/src/http/dto.rs `HeartbeatRequest` reads the first two; `rmsDbfs` is ignored today.)
struct BridgeHeartbeatRequest: Encodable {
    let audioFlowing: Bool
    let lastAudioAt: Date?
    let rmsDbfs: Double?
}

/// Response of heartbeat / release (`BridgeStateResponse`, room http/dto.rs): `BridgeDto` flattened plus intervals.
/// Every field is optional on purpose: a successful heartbeat must never be reported as failed because of a missing field.
/// The lease deadline is judged against the SERVER clock (`leaseExpiresAt - serverTime`), never the Mac's clock.
struct BridgeStateResponse: Decodable, Equatable {
    let roomId: String?
    let leaseExpiresAt: Date?
    let serverTime: Date?
    let heartbeatIntervalSeconds: Double?
    let leaseSeconds: Double?
    let capturerUserId: String?
    let epoch: Int?

    /// Seconds the lease still has at `serverTime`; nil when the server did not send both fields.
    var leaseRemainingSeconds: Double? {
        guard let e = leaseExpiresAt, let s = serverTime else { return nil }
        return e.timeIntervalSince(s)
    }
}

/// What the capture controller should do with a failed heartbeat (audit R-08: switch on the error `code`, not only the status).
enum HeartbeatFailure: Equatable {
    case authRequired
    /// Another machine took the capturer role (409 `CAPTURER_LEASE_LOST`). The lease is not ours: do not call release.
    case leaseLost(message: String)
    /// Any other 409 (`NOT_BRIDGE_ROOM`, `INVALID_STATE`, e.g. the room ended): stop, pass the backend code through.
    case conflict(code: String, message: String)
    case transient(message: String)

    static func classify(_ error: Error) -> HeartbeatFailure {
        guard let api = error as? APIError else { return .transient(message: error.localizedDescription) }
        switch api.status {
        case 401: return .authRequired
        case 409:
            if api.code == "CAPTURER_LEASE_LOST" { return .leaseLost(message: api.localizedDescription) }
            return .conflict(code: api.code ?? "CONFLICT", message: api.localizedDescription)
        default: return .transient(message: api.localizedDescription)
        }
    }
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

    /// Pure request construction (unit-tested).
    func makeRequest(_ method: String, _ path: String, body: Data? = nil) throws -> URLRequest {
        var req = URLRequest(url: try makeURL(path))
        req.httpMethod = method
        req.timeoutInterval = 15
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        req.setValue("Bearer \(currentToken)", forHTTPHeaderField: "Authorization")
        if let body { req.httpBody = body; req.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        return req
    }

    private func send(_ method: String, _ path: String, body: Data? = nil) async throws -> Data {
        let req = try makeRequest(method, path, body: body)
        let data: Data, resp: URLResponse
        do { (data, resp) = try await session.data(for: req) } catch { throw APIError.transport(error) }
        guard let http = resp as? HTTPURLResponse else { throw APIError.transport(URLError(.badServerResponse)) }
        guard (200..<300).contains(http.statusCode) else { throw Self.httpError(http.statusCode, data) }
        return data
    }

    /// `POST /meetings/{id}/bridge-token`: publish-only grant, identity = the stand-in seat. Requires the capturer role.
    func bridgeToken(roomId: String) async throws -> MeetingGrant {
        try decode(try await send("POST", "/meetings/\(roomId)/bridge-token"))
    }

    /// Body of `POST /meetings/{id}/join` for the native listener.
    static func joinBody(clientTag: String) throws -> Data {
        try JSONSerialization.data(withJSONObject: ["clientTag": clientTag])
    }

    /// Subscribe-only grant (voice mode: receive the dub track). `clientTag: "bridge-listener"` makes the server mint
    /// identity `{userId}:bridge-listener`, so this connection never evicts the web client of the same user (contract B-09).
    /// The server rejects other tags than `listener` | `bridge-listener` with 400 `INVALID_CLIENT_TAG`
    /// (crates/modules/meeting/src/application/service.rs `join`).
    func meetingJoin(roomId: String, clientTag: String = "bridge-listener") async throws -> MeetingGrant {
        try decode(try await send("POST", "/meetings/\(roomId)/join", body: Self.joinBody(clientTag: clientTag)))
    }

    /// `POST /translation-rooms/{id}/bridge/capturer/heartbeat`. Returns the lease state (nil when the body is not decodable).
    @discardableResult
    func heartbeat(roomId: String, _ req: BridgeHeartbeatRequest) async throws -> BridgeStateResponse? {
        let body = try APIDates.encoder().encode(req)
        let data = try await send("POST", "/translation-rooms/\(roomId)/bridge/capturer/heartbeat", body: body)
        return try? APIDates.decoder().decode(BridgeStateResponse.self, from: data)
    }

    /// `POST /translation-rooms/{id}/bridge/capturer/release`: give the capturer role back on stop so another machine
    /// can take it without waiting for the lease to expire. Idempotent enough to call after the web app did.
    @discardableResult
    func releaseCapturer(roomId: String) async throws -> BridgeStateResponse? {
        let data = try await send("POST", "/translation-rooms/\(roomId)/bridge/capturer/release")
        return try? APIDates.decoder().decode(BridgeStateResponse.self, from: data)
    }

    private func decode<T: Decodable>(_ data: Data) throws -> T {
        do { return try APIDates.decoder().decode(T.self, from: data) }
        catch { throw APIError.decoding(error, body: String(data: data, encoding: .utf8) ?? "") }
    }
}
