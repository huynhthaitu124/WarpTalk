import Foundation

// Engine-agnostic copilot types. The UniFFI-generated module (`CopilotCore`, crates/pipeline/copilot-core) is only touched in
// `CopilotCoreEngine.swift` (`#if canImport(CopilotCore)`); everything else in the app talks to `CopilotEngineType`, so the app
// builds and the tests run without `make core`.

enum CopilotChannel: String, Equatable, Sendable {
    case me, others
}

enum CopilotPushStatus: Equatable, Sendable {
    case accepted, droppedOldest, closed, ignored
}

enum CopilotStatusKind: String, Equatable, Sendable {
    case started, stopped, speechStart, speechEnd, sttConnected, sttUnavailable, sinkOnline, sinkOffline, audioDropped
}

struct CopilotSegment: Equatable, Sendable {
    var clientSegmentId: String
    var channel: CopilotChannel
    var text: String
    var language: String
    var startMs: UInt64
    var endMs: UInt64
    var confidence: Float?
    var isFinal: Bool
    var likelyEcho: Bool
    var utteranceSeq: UInt64
}

enum CopilotEngineEvent: Equatable, Sendable {
    case partial(channel: CopilotChannel, text: String)
    case segment(CopilotSegment)
    case status(channel: CopilotChannel?, kind: CopilotStatusKind, detail: String?)
    case error(channel: CopilotChannel?, code: String, message: String)
    case levels(channel: CopilotChannel, rms: Float)
}

struct CopilotStartParams: Equatable, Sendable {
    var baseUrl: String        // backend origin, NO /api/v1 (the core appends /api/v1/copilot/...)
    var sessionId: String
    var bearer: String
}

enum CopilotEngineError: LocalizedError {
    /// The app was built without the CopilotCore module (`make core` not run).
    case unavailable
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .unavailable: return "Engine copilot chưa được nhúng vào app (chạy `make core && make gen && make build`)."
        case .failed(let m): return m
        }
    }
}

/// What the app needs from the copilot core. Implementations must be thread-safe: `pushAudio` is called directly from the
/// audio threads (AVAudioEngine tap / Core Audio IOProc / SCK queue) and must not block.
protocol CopilotEngineType: AnyObject, Sendable {
    func start(_ params: CopilotStartParams) throws
    func pushAudio(channel: CopilotChannel, samples: [Float], sampleRate: UInt32) -> CopilotPushStatus
    func updateBearer(_ bearer: String)
    /// Flushes pending segments (can take seconds) and closes. Call off the main thread.
    func stop() throws
    /// Called on an arbitrary thread; set before `start`.
    func setEventHandler(_ handler: @escaping @Sendable (CopilotEngineEvent) -> Void)
}

typealias CopilotEngineFactory = @Sendable (_ configJson: String) throws -> CopilotEngineType

enum CopilotEngines {
    static var isAvailable: Bool {
        #if canImport(CopilotCore)
        return true
        #else
        return false
        #endif
    }

    /// Real engine when the UniFFI module is linked, otherwise throws `CopilotEngineError.unavailable`.
    static let makeDefault: CopilotEngineFactory = { json in
        #if canImport(CopilotCore)
        return try CopilotCoreEngine(configJson: json)
        #else
        _ = json
        throw CopilotEngineError.unavailable
        #endif
    }
}

/// JSON handed to `CopilotHandle(configJson:)` (`CopilotConfig::from_json`: all fields optional, snake_case; see
/// crates/pipeline/copilot-core/src/config.rs). Real STT by default; `fake` is a hidden demo setting.
enum CopilotConfigBuilder {
    static func json(useFakeStt: Bool, languages: [String]?) -> String {
        var stt: [String: Any] = ["backend": useFakeStt ? "fake" : "realtime"]
        if let first = languages?.first(where: { !$0.isEmpty }) { stt["language_hint"] = first }
        let obj: [String: Any] = ["stt": stt]
        guard let data = try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys]),
              let s = String(data: data, encoding: .utf8) else { return "{}" }
        return s
    }
}

/// Engine event -> `warptalk-native` event (`type` and `ts` are added by `NativeBridge.dispatch`). Contract: BRIDGE.md.
enum CopilotEventMapper {
    static func wire(_ e: CopilotEngineEvent) -> (type: String, payload: [String: Any]) {
        switch e {
        case .partial(let ch, let text):
            return ("copilot.partial", ["channel": ch.rawValue, "text": text])
        case .segment(let s):
            return ("copilot.segment", [
                "clientSegmentId": s.clientSegmentId, "channel": s.channel.rawValue, "text": s.text, "language": s.language,
                "startMs": NSNumber(value: s.startMs), "endMs": NSNumber(value: s.endMs),
                "confidence": nullable(s.confidence.map { Double($0) }),
                "isFinal": s.isFinal, "likelyEcho": s.likelyEcho, "utteranceSeq": NSNumber(value: s.utteranceSeq),
            ])
        case .status(let ch, let kind, let detail):
            return ("copilot.status", ["channel": nullable(ch?.rawValue), "kind": kind.rawValue, "detail": nullable(detail)])
        case .error(let ch, let code, let message):
            return ("copilot.error", ["channel": nullable(ch?.rawValue), "code": code, "message": message])
        case .levels(let ch, let rms):
            return ("copilot.levels", ["channel": ch.rawValue, "rms": Double(rms)])
        }
    }

    /// Heuristic (VERIFY: exact codes emitted by the core for HTTP 401 from the segment sink / stt-token): such errors
    /// make the controller also emit `auth-required`.
    static func isAuthError(code: String, message: String) -> Bool {
        let hay = (code + " " + message).lowercased()
        return hay.contains("401") || hay.contains("unauthor") || hay.contains("auth_expired") || hay.contains("token expired")
    }
}

// MARK: Bridge requests

struct CopilotStartRequest: Equatable {
    var sessionId: String
    var accessToken: String
    var apiBaseUrl: String
    var appBundleId: String?
    var micDeviceUid: String?
    var languages: [String]?
    /// Overlay speaker labels, default "Tôi" / "Người khác".
    var meName: String?
    var othersName: String?

    static func parse(_ r: BridgeRequest) throws -> CopilotStartRequest {
        var langs: [String]?
        if let raw = r.params["languages"] {
            guard let arr = raw as? [String] else { throw BridgeError(code: "INVALID_ARGUMENT", message: "'languages' phải là mảng string.") }
            langs = arr.filter { !$0.isEmpty }
        }
        let names = r.params["speakerNames"] as? [String: Any]
        func name(_ k: String) -> String? { (names?[k] as? String).flatMap { $0.isEmpty ? nil : $0 } }
        return CopilotStartRequest(
            sessionId: try r.string("sessionId"), accessToken: try r.string("accessToken"), apiBaseUrl: try r.string("apiBaseUrl"),
            appBundleId: r.optString("appBundleId"), micDeviceUid: r.optString("micDeviceUid"),
            languages: langs, meName: name("me"), othersName: name("others"))
    }

    /// The core talks plain HTTP(S) to the backend. The bundled app reports `apiBaseUrl` as `warptalk://app` (same-origin proxy),
    /// which is no use here: fall back to the native Settings backend URL. A trailing `/api/v1` or `/` is stripped.
    static func resolveBaseUrl(requested: String, fallback: String) throws -> String {
        func clean(_ s: String) -> String? {
            var t = s.trimmingCharacters(in: .whitespacesAndNewlines)
            while t.hasSuffix("/") { t.removeLast() }
            if t.hasSuffix("/api/v1") { t.removeLast("/api/v1".count) }
            while t.hasSuffix("/") { t.removeLast() }
            guard let u = URL(string: t), let scheme = u.scheme?.lowercased(), scheme == "http" || scheme == "https",
                  let host = u.host, !host.isEmpty else { return nil }
            return t
        }
        if let ok = clean(requested) { return ok }
        if let ok = clean(fallback) { return ok }
        throw BridgeError(code: "INVALID_ARGUMENT", message: "apiBaseUrl không hợp lệ (cần http(s)://host[:port]).")
    }
}

struct OverlaySuggestion: Identifiable, Equatable {
    let id: String
    var category: String
    var content: String
    /// Follow-up question text shown under the content (string form of the `followUp` argument).
    var followUp: String?
    /// `followUp: true` form: this suggestion itself is a follow-up item.
    var isFollowUp: Bool

    static func parse(_ r: BridgeRequest) throws -> OverlaySuggestion {
        var text: String?
        var flag = false
        switch r.params["followUp"] {
        case let s as String: text = s.isEmpty ? nil : s
        case let b as Bool: flag = b
        case nil, is NSNull: break
        default: throw BridgeError(code: "INVALID_ARGUMENT", message: "'followUp' phải là string hoặc bool.")
        }
        return OverlaySuggestion(id: try r.string("id"), category: r.optString("category") ?? "suggestion",
                                 content: try r.string("content"), followUp: text, isFollowUp: flag)
    }
}
