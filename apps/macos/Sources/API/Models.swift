import Foundation

// Typed DTOs. Shapes copied from the Rust handlers (camelCase):
//  identity  : crates/modules/identity/src/http/dto.rs
//  workspace : crates/modules/workspace/src/http/dto.rs
//  room      : crates/modules/room/src/http/dto.rs
//  meeting   : crates/modules/meeting/src/http/dto.rs
// Ids are UUID strings; kept as String to stay tolerant.

// MARK: - Auth

struct RegisterRequest: Encodable {
    let email: String
    let password: String
    let fullName: String
}

struct RegisterResponse: Decodable {
    let userId: String
}

struct LoginRequest: Encodable {
    let email: String
    let password: String
}

struct RefreshRequest: Encodable {
    let refreshToken: String
}

struct AuthResponse: Decodable {
    let userId: String
    let email: String
    let accessToken: String
    let accessTokenExpiresAt: Date
    let refreshToken: String
}

// MARK: - Paging

struct Page<T: Decodable>: Decodable {
    let items: [T]
    let page: Int
    let pageSize: Int
    let total: Int
}

// MARK: - Workspaces

struct Workspace: Decodable, Identifiable, Hashable {
    let id: String
    let name: String
    let slug: String
    let description: String?
    let logoUrl: String?
    let ownerId: String
    let myRole: String
    let createdAt: Date
}

struct CreateWorkspaceRequest: Encodable {
    let name: String
    let description: String?
}

// MARK: - Rooms

enum RoomStatus: String {
    case scheduled = "SCHEDULED", waiting = "WAITING", open = "OPEN", inProgress = "IN_PROGRESS"
    case paused = "PAUSED", ended = "ENDED", cancelled = "CANCELLED", expired = "EXPIRED", failed = "FAILED"
    case unknown

    var isClosed: Bool { [.ended, .cancelled, .expired, .failed].contains(self) }
}

struct TranslationRoom: Decodable, Identifiable, Hashable {
    let id: String
    let workspaceId: String
    let hostId: String
    let activeHostId: String?
    let effectiveHostId: String
    let title: String
    let roomCode: String
    let status: String
    let sourceLanguage: String
    let targetLanguages: [String]
    let requiresApproval: Bool
    let scheduledAt: Date?
    let startedAt: Date?
    let endedAt: Date?
    let createdAt: Date
    let translationActive: Bool

    var statusEnum: RoomStatus { RoomStatus(rawValue: status) ?? .unknown }
}

struct CreateRoomRequest: Encodable {
    let workspaceId: String
    let title: String
    let displayName: String?
    let sourceLanguage: String
    let targetLanguages: [String]
    let requiresApproval: Bool?
}

struct JoinByCodeRequest: Encodable {
    let roomCode: String
    let displayName: String?
    let speakLanguage: String
    let listenLanguage: String
}

struct JoinByIdRequest: Encodable {
    let displayName: String?
    let speakLanguage: String
    let listenLanguage: String
}

struct RoomParticipant: Decodable, Identifiable, Hashable {
    let id: String
    let userId: String?
    let displayName: String
    let role: String            // "HOST" | "PARTICIPANT"
    let speakLanguage: String
    let listenLanguage: String
    let status: String
    let isUsingVoiceClone: Bool
    let joinedAt: Date?
    let leftAt: Date?
}

/// Tolerant JSON value (route `status` is an arbitrary serde value).
enum JSONValue: Decodable, Hashable {
    case string(String), number(Double), bool(Bool), array([JSONValue]), object([String: JSONValue]), null

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode([JSONValue].self) { self = .array(v) }
        else if let v = try? c.decode([String: JSONValue].self) { self = .object(v) }
        else { self = .null }
    }
}

struct AudioRoute: Decodable, Hashable {
    let routeId: String
    let sourceParticipantId: String
    let targetParticipantId: String
    let sourceLanguage: String
    let targetLanguage: String
    let voiceCloneEnabled: Bool
    let status: JSONValue
}

struct AudioRoutes: Decodable {
    let generatedAt: Date
    let roomStatus: String
    let translationActive: Bool
    let roomLanguages: [String]
    let routes: [AudioRoute]
}

// MARK: - Meeting (LiveKit grant)

struct MeetingJoinRequest: Encodable {
    let displayName: String?
}

struct MeetingGrant: Decodable {
    let roomName: String
    let serverUrl: String
    let token: String
    let participantIdentity: String
}

// MARK: - External bridge (spec 10 §2.1 / §3.8, external-meeting.md §4.4)

struct BridgeClaimRequest: Encodable {
    let workspaceId: String
    /// "google_meet" | "zoom" | "teams" (domain/bridge.rs BridgeProvider)
    let provider: String
    let meetCode: String
    let sourceLanguage: String?
    let targetLanguages: [String]?
    let externalMeetingLanguage: String?
    let displayName: String?
    /// "voice" | "text"
    let audioMode: String?
}

struct BridgeClaim: Decodable {
    let room: TranslationRoom
    let bridgeRole: String          // "capturer" | "member" (anything else treated as member)
    let created: Bool
    let heartbeatIntervalSeconds: Double
    let leaseSeconds: Double
    let audioMode: String?

    var isCapturer: Bool { bridgeRole.lowercased() == "capturer" }

    private enum CodingKeys: String, CodingKey {
        case room, bridgeRole, created, audioMode
        case heartbeatIntervalSeconds, leaseSeconds
        case capturerHeartbeatIntervalSeconds, capturerLeaseSeconds   // upstream (warptalk-web) spelling
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        room = try c.decode(TranslationRoom.self, forKey: .room)
        bridgeRole = (try c.decodeIfPresent(String.self, forKey: .bridgeRole)) ?? "member"
        created = (try c.decodeIfPresent(Bool.self, forKey: .created)) ?? false
        audioMode = try c.decodeIfPresent(String.self, forKey: .audioMode)
        heartbeatIntervalSeconds =
            (try c.decodeIfPresent(Double.self, forKey: .heartbeatIntervalSeconds))
            ?? (try c.decodeIfPresent(Double.self, forKey: .capturerHeartbeatIntervalSeconds)) ?? 15
        leaseSeconds =
            (try c.decodeIfPresent(Double.self, forKey: .leaseSeconds))
            ?? (try c.decodeIfPresent(Double.self, forKey: .capturerLeaseSeconds)) ?? 45
    }
}

struct BridgeHeartbeatRequest: Encodable {
    let audioFlowing: Bool
    let lastAudioAt: Date?
    let rmsDbfs: Double?
}

struct BridgeCapturerStatus: Decodable {
    let roomId: String?
    let bridgeRole: String?
    let capturerUserId: String?
    let capturerHeartbeatAt: Date?
    let capturerLeaseSeconds: Double?
}

struct BridgeAudioModeRequest: Encodable {
    let mode: String
}

struct BridgeAudioModeResponse: Decodable {
    let roomId: String?
    let userId: String?
    let mode: String
    let translationActive: Bool?
}

// MARK: - Errors

/// `{ "code": "...", "message": "..." }` from warptalk_kernel::AppError.
struct ErrorBody: Decodable {
    let code: String
    let message: String
}
