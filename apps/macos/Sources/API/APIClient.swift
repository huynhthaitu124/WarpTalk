import Foundation

/// async/await REST client for the WarpTalk backend (base path `/api/v1`).
///
/// - Bearer access token from `TokenStore`.
/// - On 401 (except auth endpoints) performs ONE refresh (single-flight across concurrent requests)
///   using refresh-token rotation (`POST /auth/refresh` returns a NEW refresh token that replaces the old),
///   then retries the original request once.
/// - If refresh fails the session is cleared and `onSessionExpired` fires.
actor APIClient {
    private let baseURLProvider: @Sendable () -> String
    private let session: URLSession
    private nonisolated let store: TokenStore
    private let decoder = APIDates.decoder()
    private let encoder = APIDates.encoder()
    private var refreshTask: Task<StoredSession, Error>?

    private var onSessionExpired: (@Sendable () -> Void)?
    func setOnSessionExpired(_ f: @escaping @Sendable () -> Void) { onSessionExpired = f }

    init(baseURL: @escaping @Sendable () -> String, store: TokenStore, session: URLSession = .shared) {
        self.baseURLProvider = baseURL
        self.store = store
        self.session = session
    }

    // MARK: Core

    private func url(_ path: String, query: [URLQueryItem] = []) throws -> URL {
        var base = baseURLProvider().trimmingCharacters(in: .whitespacesAndNewlines)
        while base.hasSuffix("/") { base.removeLast() }
        guard var comps = URLComponents(string: base + "/api/v1" + path) else { throw APIError.invalidURL(base + path) }
        if !query.isEmpty { comps.queryItems = query }
        guard let u = comps.url else { throw APIError.invalidURL(base + path) }
        return u
    }

    private func build(_ method: String, _ path: String, query: [URLQueryItem], body: Data?, auth: Bool) throws -> URLRequest {
        var req = URLRequest(url: try url(path, query: query))
        req.httpMethod = method
        req.timeoutInterval = 20
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body { req.httpBody = body; req.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        if auth, let s = store.load() { req.setValue("Bearer \(s.accessToken)", forHTTPHeaderField: "Authorization") }
        return req
    }

    private func perform(_ req: URLRequest) async throws -> (Data, HTTPURLResponse) {
        do {
            let (data, resp) = try await session.data(for: req)
            guard let http = resp as? HTTPURLResponse else { throw APIError.transport(URLError(.badServerResponse)) }
            return (data, http)
        } catch let e as APIError { throw e } catch { throw APIError.transport(error) }
    }

    /// Returns raw data for a 2xx, throws `APIError.http` otherwise.
    private func send(_ method: String, _ path: String, query: [URLQueryItem] = [], body: Data? = nil, auth: Bool = true) async throws -> Data {
        var (data, http) = try await perform(try build(method, path, query: query, body: body, auth: auth))
        if http.statusCode == 401, auth, store.load()?.refreshToken != nil {
            _ = try await refreshSession()
            (data, http) = try await perform(try build(method, path, query: query, body: body, auth: auth))
        }
        guard (200..<300).contains(http.statusCode) else { throw Self.httpError(http.statusCode, data) }
        return data
    }

    static func httpError(_ status: Int, _ data: Data) -> APIError {
        if let b = try? JSONDecoder().decode(ErrorBody.self, from: data) {
            return .http(status: status, code: b.code, message: b.message)
        }
        let text = String(data: data, encoding: .utf8)?.prefix(200) ?? ""
        return .http(status: status, code: "HTTP_\(status)", message: text.isEmpty ? "Lỗi server" : String(text))
    }

    private func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        do { return try decoder.decode(T.self, from: data) }
        catch { throw APIError.decoding(error, body: String(data: data, encoding: .utf8) ?? "") }
    }

    private func call<T: Decodable, B: Encodable>(_ method: String, _ path: String, query: [URLQueryItem] = [], body: B?, auth: Bool = true) async throws -> T {
        let bodyData = try body.map { try encoder.encode($0) }
        return try decode(T.self, from: try await send(method, path, query: query, body: bodyData, auth: auth))
    }

    private func call<T: Decodable>(_ method: String, _ path: String, query: [URLQueryItem] = []) async throws -> T {
        try decode(T.self, from: try await send(method, path, query: query))
    }

    private func callVoid<B: Encodable>(_ method: String, _ path: String, body: B?, auth: Bool = true) async throws {
        let bodyData = try body.map { try encoder.encode($0) }
        _ = try await send(method, path, body: bodyData, auth: auth)
    }

    private struct Empty: Encodable {}

    // MARK: Refresh (single-flight, rotation)

    @discardableResult
    func refreshSession() async throws -> StoredSession {
        if let t = refreshTask { return try await t.value }
        guard let current = store.load() else { throw APIError.notAuthenticated }
        let task = Task<StoredSession, Error> { [self] in
            let body = try encoder.encode(RefreshRequest(refreshToken: current.refreshToken))
            let (data, http) = try await perform(try build("POST", "/auth/refresh", query: [], body: body, auth: false))
            guard (200..<300).contains(http.statusCode) else {
                throw Self.httpError(http.statusCode, data)
            }
            let r = try decode(AuthResponse.self, from: data)
            let s = Self.stored(r)
            store.save(s)   // rotation: the old refresh token is dead from now on
            return s
        }
        refreshTask = task
        defer { refreshTask = nil }
        do { return try await task.value }
        catch {
            // Only a definitive rejection ends the session; network errors keep the tokens.
            if let api = error as? APIError, let st = api.status, st == 400 || st == 401 || st == 403 {
                store.clear()
                onSessionExpired?()
                throw APIError.notAuthenticated
            }
            throw error
        }
    }

    static func stored(_ r: AuthResponse) -> StoredSession {
        StoredSession(userId: r.userId, email: r.email, accessToken: r.accessToken,
                      accessTokenExpiresAt: r.accessTokenExpiresAt, refreshToken: r.refreshToken)
    }

    // MARK: Auth

    func register(email: String, password: String, fullName: String) async throws -> RegisterResponse {
        try await call("POST", "/auth/register", body: RegisterRequest(email: email, password: password, fullName: fullName), auth: false)
    }

    @discardableResult
    func login(email: String, password: String) async throws -> StoredSession {
        let r: AuthResponse = try await call("POST", "/auth/login", body: LoginRequest(email: email, password: password), auth: false)
        let s = Self.stored(r)
        store.save(s)
        return s
    }

    func logout() async {
        if let s = store.load() {
            try? await callVoid("POST", "/auth/logout", body: RefreshRequest(refreshToken: s.refreshToken), auth: false)
        }
        store.clear()
    }

    nonisolated var currentSession: StoredSession? { store.load() }

    // MARK: Workspaces

    func listWorkspaces(page: Int = 1, pageSize: Int = 100) async throws -> Page<Workspace> {
        try await call("GET", "/workspaces", query: [.init(name: "page", value: "\(page)"), .init(name: "pageSize", value: "\(pageSize)")])
    }

    func createWorkspace(name: String, description: String?) async throws -> Workspace {
        try await call("POST", "/workspaces", body: CreateWorkspaceRequest(name: name, description: description))
    }

    // MARK: Rooms

    func listRooms(workspaceId: String, status: String? = nil, page: Int = 1, pageSize: Int = 50) async throws -> Page<TranslationRoom> {
        var q: [URLQueryItem] = [.init(name: "workspaceId", value: workspaceId),
                                 .init(name: "page", value: "\(page)"), .init(name: "pageSize", value: "\(pageSize)")]
        if let status { q.append(.init(name: "status", value: status)) }
        return try await call("GET", "/translation-rooms", query: q)
    }

    func createRoom(_ req: CreateRoomRequest) async throws -> TranslationRoom { try await call("POST", "/translation-rooms", body: req) }
    func getRoom(_ id: String) async throws -> TranslationRoom { try await call("GET", "/translation-rooms/\(id)") }

    func joinRoom(byCode req: JoinByCodeRequest) async throws -> RoomParticipant { try await call("POST", "/translation-rooms/join", body: req) }
    func joinRoom(id: String, _ req: JoinByIdRequest) async throws -> RoomParticipant { try await call("POST", "/translation-rooms/\(id)/join", body: req) }

    func startRoom(_ id: String) async throws -> TranslationRoom { try await call("POST", "/translation-rooms/\(id)/start", body: Optional<Empty>.none) }
    func endRoom(_ id: String) async throws -> TranslationRoom { try await call("POST", "/translation-rooms/\(id)/end", body: Optional<Empty>.none) }
    func participants(_ id: String) async throws -> [RoomParticipant] { try await call("GET", "/translation-rooms/\(id)/participants") }
    func audioRoutes(_ id: String) async throws -> AudioRoutes { try await call("GET", "/translation-rooms/\(id)/audio-routes") }
    func leaveRoom(_ id: String) async throws { try await callVoid("PUT", "/translation-rooms/\(id)/participants/me/leave", body: Optional<Empty>.none) }

    // MARK: Meeting tokens

    func meetingJoin(roomId: String, displayName: String?) async throws -> MeetingGrant {
        try await call("POST", "/meetings/\(roomId)/join", body: MeetingJoinRequest(displayName: displayName))
    }

    func bridgeToken(roomId: String) async throws -> MeetingGrant {
        try await call("POST", "/meetings/\(roomId)/bridge-token", body: Optional<Empty>.none)
    }

    // MARK: External bridge (shape per spec 10 §2.1 — endpoints are being added server-side)

    func claimBridge(_ req: BridgeClaimRequest) async throws -> BridgeClaim {
        try await call("POST", "/translation-rooms/bridge/claim", body: req)
    }

    func bridgeHeartbeat(roomId: String, _ req: BridgeHeartbeatRequest) async throws -> BridgeCapturerStatus {
        try await call("POST", "/translation-rooms/\(roomId)/bridge/capturer/heartbeat", body: req)
    }

    func bridgeTakeover(roomId: String) async throws -> BridgeCapturerStatus {
        try await call("POST", "/translation-rooms/\(roomId)/bridge/capturer/takeover", body: Optional<Empty>.none)
    }

    func setBridgeAudioMode(roomId: String, mode: String) async throws -> BridgeAudioModeResponse {
        try await call("PUT", "/translation-rooms/\(roomId)/bridge/audio-mode", body: BridgeAudioModeRequest(mode: mode))
    }
}
