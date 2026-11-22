import AVFoundation
import Foundation

/// Thread-safe audio activity meter (written from the capture queue, read from the main actor).
final class AudioActivity: @unchecked Sendable {
    private let lock = NSLock()
    private var lastBufferAt: Date?
    private var lastSignalAt: Date?
    private var lastRms: Double = -120
    var signalThresholdDb: Double = -60

    func record(rmsDbfs: Double, now: Date = Date()) {
        lock.lock(); defer { lock.unlock() }
        lastBufferAt = now
        lastRms = rmsDbfs
        if rmsDbfs > signalThresholdDb { lastSignalAt = now }
    }

    struct Snapshot { var rmsDbfs: Double; var lastAudioAt: Date?; var flowing: Bool }

    /// `flowing` = the capture pipeline is delivering buffers (liveness), NOT "somebody is talking":
    /// a quiet meeting must not look like a dead capturer. `lastAudioAt` = last time the level exceeded the threshold.
    func snapshot(now: Date = Date(), liveWindow: TimeInterval = 3) -> Snapshot {
        lock.lock(); defer { lock.unlock() }
        let flowing = lastBufferAt.map { now.timeIntervalSince($0) <= liveWindow } ?? false
        return Snapshot(rmsDbfs: flowing ? lastRms : -120, lastAudioAt: lastSignalAt, flowing: flowing)
    }
}

/// JSON-friendly optional: nil -> NSNull.
func nullable(_ v: Any?) -> Any { v ?? NSNull() }

struct StartCaptureParams {
    var appBundleId: String
    var roomId: String
    var mode: String                 // "text" | "voice"
    var accessToken: String
    var backendUrl: String?
    var heartbeatIntervalSeconds: Double
    var listenLanguage: String?      // required for voice
    var dubOutputDeviceUid: String?
    var captureBackend: String       // "auto" | "processTap" | "screenCaptureKit"
}

struct BridgeError: LocalizedError {
    let code: String
    let message: String
    var errorDescription: String? { message }
}

/// External Meeting capture, driven by the web app through the JS bridge.
/// The web app already claimed the bridge room; here we: get bridge token -> connect/publish -> capture app audio ->
/// heartbeat. Voice mode additionally runs a dub listener -> AVAudioEngine -> chosen output device.
@MainActor
final class NativeCaptureController {
    enum State: String { case idle, starting, running, stopping, error }

    /// (eventType, payload) — the bridge adds `type` and `ts` and dispatches `warptalk-native`.
    var emit: (String, [String: Any]) -> Void = { _, _ in }
    private let defaultBackendURL: () -> String
    private let defaultDubDeviceUID: () -> String
    private let permissions: PermissionsManager

    private(set) var state: State = .idle
    private var params: StartCaptureParams?
    private var api: NativeAPI?
    private let publisher = BridgePublisher()
    private var source: AudioCaptureSource?
    private var backendName = ""
    private var voice: VoiceOutputPlayer?
    private var dub: DubListener?
    private let activity = AudioActivity()
    private var heartbeatTask: Task<Void, Never>?
    private var meterTask: Task<Void, Never>?
    private var lastHeartbeatAt: Date?
    private var lastFlowing = false
    private var lastError: (code: String, message: String)?

    init(permissions: PermissionsManager, defaultBackendURL: @escaping () -> String, defaultDubDeviceUID: @escaping () -> String) {
        self.permissions = permissions
        self.defaultBackendURL = defaultBackendURL
        self.defaultDubDeviceUID = defaultDubDeviceUID
    }

    // MARK: Status

    func status() -> [String: Any] {
        let snap = activity.snapshot()
        var d: [String: Any] = [
            "state": state.rawValue,
            "roomId": nullable(params?.roomId),
            "appBundleId": nullable(params?.appBundleId),
            "mode": nullable(params?.mode),
            "backend": nullable(backendName.isEmpty ? nil : backendName),
            "rmsDbfs": snap.rmsDbfs,
            "audioFlowing": snap.flowing,
            "lastAudioAt": nullable(snap.lastAudioAt.map { APIDates.string($0) }),
            "lastHeartbeatAt": nullable(lastHeartbeatAt.map { APIDates.string($0) }),
            "dubTrack": nullable(dub?.subscribedTrackName),
        ]
        if let e = lastError { d["error"] = ["code": e.code, "message": e.message] }
        return d
    }

    private func setState(_ s: State) {
        state = s
        emit("status", status())
    }

    func updateToken(_ t: String) { api?.updateToken(t) }

    // MARK: Start / stop

    func start(_ p: StartCaptureParams) async throws {
        guard state == .idle || state == .error else { throw BridgeError(code: "ALREADY_RUNNING", message: "Capture đang chạy.") }
        guard let target = RunningMeetingApps.current().first(where: { $0.bundleID == p.appBundleId }) else {
            throw BridgeError(code: "APP_NOT_RUNNING", message: "Ứng dụng \(p.appBundleId) chưa chạy.")
        }
        if p.mode == "voice" && (p.listenLanguage ?? "").isEmpty {
            throw BridgeError(code: "INVALID_ARGUMENT", message: "mode=voice cần listenLanguage.")
        }
        params = p
        lastError = nil
        setState(.starting)
        let api = NativeAPI(baseURL: p.backendUrl ?? defaultBackendURL(), accessToken: p.accessToken)
        self.api = api

        do {
            if p.mode == "voice" {
                let uid = p.dubOutputDeviceUid ?? defaultDubDeviceUID()
                let player = VoiceOutputPlayer(outputDeviceUID: uid)
                voice = player
                let listener = DubListener(language: p.listenLanguage ?? "en") { [player] buf in player.enqueue(buf) }
                let grant = try await api.meetingJoin(roomId: p.roomId, clientTag: "bridge-listener")
                try await listener.connect(url: grant.serverUrl, token: grant.token)
                dub = listener
            }

            let bt = try await api.bridgeToken(roomId: p.roomId)
            try await publisher.connect(url: bt.serverUrl, token: bt.token)

            try await startCapture(target: target, preference: p.captureBackend)
            startHeartbeat(roomId: p.roomId, interval: max(5, p.heartbeatIntervalSeconds))
            startMeter()
            setState(.running)
        } catch {
            let e = Self.map(error)
            lastError = (e.code, e.message)
            await teardown()
            setState(.error)
            emit("error", ["code": e.code, "message": e.message, "fatal": true])
            throw e
        }
    }

    func stop() async {
        guard state != .idle else { return }
        setState(.stopping)
        await teardown()
        lastError = nil
        setState(.idle)
    }

    private func teardown() async {
        heartbeatTask?.cancel(); heartbeatTask = nil
        meterTask?.cancel(); meterTask = nil
        source?.stop(); source = nil
        voice?.stop(); voice = nil
        await dub?.disconnect(); dub = nil
        await publisher.disconnect()
        backendName = ""
    }

    static func map(_ error: Error) -> BridgeError {
        if let b = error as? BridgeError { return b }
        if let c = error as? CaptureError {
            switch c {
            case .permissionDenied: return BridgeError(code: "PERMISSION_DENIED", message: c.localizedDescription)
            case .noAudioProcess: return BridgeError(code: "NO_AUDIO_PROCESS", message: c.localizedDescription)
            default: return BridgeError(code: "CAPTURE_FAILED", message: c.localizedDescription)
            }
        }
        if let a = error as? APIError {
            if a.status == 401 { return BridgeError(code: "AUTH_EXPIRED", message: a.localizedDescription) }
            return BridgeError(code: a.code ?? "API_ERROR", message: a.localizedDescription)
        }
        return BridgeError(code: "INTERNAL", message: error.localizedDescription)
    }

    // MARK: Capture

    private func startCapture(target: CaptureTarget, preference: String) async throws {
        let useSCK = preference == "screenCaptureKit"
        let primary: AudioCaptureSource = useSCK ? SCKAudioCapture() : ProcessTapCapture()
        do {
            try await begin(primary, target: target)
        } catch {
            if !useSCK { permissions.recordAudioCaptureResult(success: false) }
            guard preference == "auto", !useSCK else { throw error }
            emit("log", ["message": "Process Tap lỗi (\(error.localizedDescription)) → thử ScreenCaptureKit"])
            try await begin(SCKAudioCapture(), target: target)
        }
    }

    private func begin(_ src: AudioCaptureSource, target: CaptureTarget) async throws {
        let activity = self.activity
        let publisher = self.publisher
        src.onBuffer = { buf in
            activity.record(rmsDbfs: Resampler.rmsDbfs(buf))
            publisher.push(buf)
        }
        src.onError = { [weak self] err in
            Task { @MainActor in
                self?.emit("error", ["code": "CAPTURE_FAILED", "message": err.localizedDescription, "fatal": false])
            }
        }
        try await src.start(target: target)
        source = src
        backendName = (src is ProcessTapCapture) ? "processTap" : "screenCaptureKit"
        if src is ProcessTapCapture { permissions.recordAudioCaptureResult(success: true) }
    }

    // MARK: Heartbeat (default 15 s) + meter (5 Hz)

    private func startHeartbeat(roomId: String, interval: Double) {
        heartbeatTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.beat(roomId: roomId)
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
            }
        }
    }

    private func beat(roomId: String) async {
        guard let api else { return }
        let snap = activity.snapshot()
        do {
            try await api.heartbeat(roomId: roomId, BridgeHeartbeatRequest(audioFlowing: snap.flowing, lastAudioAt: snap.lastAudioAt, rmsDbfs: snap.rmsDbfs))
            lastHeartbeatAt = Date()
            emit("heartbeat", ["ok": true, "at": APIDates.string(Date())])
        } catch let e as APIError where e.status == 409 {
            lastError = ("LEASE_LOST", "Mất quyền capturer (409).")
            emit("error", ["code": "LEASE_LOST", "message": "Mất quyền capturer (409). Đã dừng capture.", "fatal": true])
            Task { await self.stop() }
        } catch let e as APIError where e.status == 401 {
            emit("auth-required", ["reason": "heartbeat"])     // web app should call updateAuth({accessToken})
        } catch {
            emit("heartbeat", ["ok": false, "message": error.localizedDescription])
        }
    }

    private func startMeter() {
        lastFlowing = false
        meterTask = Task { [weak self] in
            while !Task.isCancelled {
                if let self {
                    let s = self.activity.snapshot()
                    self.emit("level", ["rmsDbfs": s.rmsDbfs, "audioFlowing": s.flowing])
                    if s.flowing != self.lastFlowing {
                        self.lastFlowing = s.flowing
                        self.emit("audioFlowing", ["audioFlowing": s.flowing, "lastAudioAt": nullable(s.lastAudioAt.map { APIDates.string($0) })])
                    }
                }
                try? await Task.sleep(nanoseconds: 200_000_000)
            }
        }
    }
}
