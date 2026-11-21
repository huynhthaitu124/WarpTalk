import Foundation

/// JSON fixtures mirroring the Rust DTOs (camelCase, chrono timestamps with nanosecond fractions).
enum Fixtures {
    static let auth = """
    {"userId":"0190a1b2-0000-7000-8000-000000000001","email":"a@example.com","accessToken":"at","accessTokenExpiresAt":"2026-11-21T10:00:00.123456789Z","refreshToken":"rt"}
    """
    static let workspacePage = """
    {"items":[{"id":"0190a1b2-0000-7000-8000-0000000000aa","name":"Demo","slug":"demo","description":null,"logoUrl":null,"ownerId":"0190a1b2-0000-7000-8000-000000000001","myRole":"OWNER","createdAt":"2026-11-21T09:00:00Z"}],"page":1,"pageSize":20,"total":1}
    """
    static let room = """
    {"id":"0190a1b2-0000-7000-8000-0000000000bb","workspaceId":"0190a1b2-0000-7000-8000-0000000000aa","hostId":"0190a1b2-0000-7000-8000-000000000001","activeHostId":null,"effectiveHostId":"0190a1b2-0000-7000-8000-000000000001","title":"Standup","roomCode":"abcdefghijkl","status":"IN_PROGRESS","sourceLanguage":"vi","targetLanguages":["en","ja"],"requiresApproval":false,"scheduledAt":null,"startedAt":"2026-11-21T09:30:00.5Z","endedAt":null,"createdAt":"2026-11-21T09:00:00.000001Z","translationActive":true}
    """
    static let participant = """
    {"id":"p1","userId":null,"displayName":"An","role":"HOST","speakLanguage":"vi","listenLanguage":"en","status":"CONNECTED","isUsingVoiceClone":false,"joinedAt":"2026-11-21T09:30:00Z","leftAt":null}
    """
    static let grant = """
    {"roomName":"room-1","serverUrl":"wss://lk.example.com","token":"jwt","participantIdentity":"user-1"}
    """
    static let error = #"{"code":"INVALID_STATE","message":"meeting has already ended"}"#
    static var claimSpec: String { """
    {"room":\(room),"bridgeRole":"capturer","created":true,"heartbeatIntervalSeconds":15,"leaseSeconds":45,"audioMode":"text"}
    """ }
    static var claimUpstream: String { """
    {"room":\(room),"capturerHeartbeatIntervalSeconds":15,"capturerLeaseSeconds":45}
    """ }
    static let caption = """
    {"segmentId":"s1","speakerId":"u1","speakerName":"An","lang":"vi","text":"Xin chào","isFinal":true,"translations":{"en":"Hello","ja":"こんにちは"}}
    """
}
