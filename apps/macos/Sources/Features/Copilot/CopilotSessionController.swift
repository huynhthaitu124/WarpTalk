import AVFoundation
import Foundation

/// Microphone permission, abstracted so the state machine is testable. `PermissionsManager` is the real implementation.
@MainActor
protocol CopilotPermissionProviding: AnyObject {
    /// Asks if undetermined; true when the microphone may be used.
    func ensureMicrophone() async -> Bool
}

extension PermissionsManager: CopilotPermissionProviding {
    func ensureMicrophone() async -> Bool {
        refresh()
        if microphone == .notDetermined { await requestMicrophone() }
        return microphone == .granted
    }
}

/// Everything the controller touches outside itself; tests substitute fakes.
struct CopilotDependencies {
    var engineFactory: CopilotEngineFactory
    var makeMic: () -> MicCapturing
    var makeProcessTap: () -> AudioCaptureSource
    var makeScreenCaptureKit: () -> AudioCaptureSource
    /// Running instance of the meeting app (NSWorkspace) or nil.
    var resolveTarget: (_ bundleId: String) -> CaptureTarget?
    var permissions: CopilotPermissionProviding
    var defaultBackendURL: () -> String
    var defaultMeetingAppBundleId: () -> String
    var defaultMicDeviceUid: () -> String
    var useFakeStt: () -> Bool
    /// Delay between attempts to start the `others` capture while the meeting app is not running / not playing audio yet.
    var othersRetryNanos: UInt64 = 5_000_000_000

    @MainActor
    static func live(permissions: PermissionsManager, settings: SettingsStore) -> CopilotDependencies {
        CopilotDependencies(
            engineFactory: CopilotEngines.makeDefault,
            makeMic: { MicCapture() },
            makeProcessTap: { ProcessTapCapture() },
            makeScreenCaptureKit: { SCKAudioCapture() },
            resolveTarget: { id in RunningMeetingApps.current().first { $0.bundleID == id } },
            permissions: permissions,
            defaultBackendURL: { settings.backendURL },
            defaultMeetingAppBundleId: { settings.meetingAppBundleId },
            defaultMicDeviceUid: { settings.micDeviceUID },
            useFakeStt: { settings.copilotFakeStt })
    }
}

/// Copilot session capture (ADR 0001): mic -> channel `.me`, meeting-app audio -> channel `.others`, both pushed straight from
/// their audio threads into the engine (`pushAudio`, no main-thread hop). Engine events go to the web app (`warptalk-native`
/// events `copilot.*`) and to the overlay. The engine streams segments to the backend itself (`POST /copilot/sessions/{id}/segments`).
@MainActor
final class CopilotSessionController: ObservableObject {
    enum State: String { case idle, starting, running, stopping, error }
    enum OthersState: String { case off, waiting, running }

    @Published private(set) var state: State = .idle
    @Published private(set) var othersState: OthersState = .off

    /// (eventType, payload): the bridge adds `type`/`ts` and dispatches `warptalk-native`.
    var emit: (String, [String: Any]) -> Void = { _, _ in }
    /// Engine events for the overlay (also `.segment` with `likelyEcho`; the overlay decides what to show).
    var onOverlayEvent: (@MainActor (CopilotEngineEvent) -> Void)?
    /// true when a session starts running (overlay: enter copilot mode, speaker names), false when it stops.
    var onSessionActive: (@MainActor (_ active: Bool, _ request: CopilotStartRequest?) -> Void)?

    private let deps: CopilotDependencies
    private var request: CopilotStartRequest?
    private var engine: CopilotEngineType?
    private var mic: MicCapturing?
    private var others: AudioCaptureSource?
    private var othersBackend = ""
    private var othersRetry: Task<Void, Never>?
    private var lastOthersErrorCode: String?
    private let activity = AudioActivity()
    private var generation = 0
    private var lastError: (code: String, message: String)?

    init(deps: CopilotDependencies) { self.deps = deps }

    var isActive: Bool { state != .idle && state != .error }

    // MARK: Status

    func copilotStatus() -> [String: Any] {
        let snap = activity.snapshot()
        var d: [String: Any] = [
            "state": state.rawValue,
            "roomId": nullable(request?.sessionId),
            "appBundleId": nullable(request?.appBundleId),
            "mode": NSNull(),
            "backend": nullable(othersBackend.isEmpty ? nil : othersBackend),
            "rmsDbfs": snap.rmsDbfs,
            "audioFlowing": snap.flowing,
            "lastAudioAt": nullable(snap.lastAudioAt.map { APIDates.string($0) }),
            "lastHeartbeatAt": NSNull(),
            "leaseExpiresAt": NSNull(),
            "dubTrack": NSNull(),
            "sessionId": nullable(request?.sessionId),
            "channels": ["me": mic != nil ? "running" : "off", "others": othersState.rawValue],
        ]
        if let e = lastError { d["error"] = ["code": e.code, "message": e.message] }
        return d
    }

    private func setState(_ s: State) {
        state = s
        emit("status", copilotStatus())
    }

    private func setOthers(_ s: OthersState) {
        guard othersState != s else { return }
        othersState = s
        emit("status", copilotStatus())
    }

    // MARK: Auth

    /// Called by the bridge on `updateAuth` (the web app refreshed its JWT).
    func updateBearer(_ token: String) { engine?.updateBearer(token) }

    // MARK: Start

    func startCopilotSession(_ req: CopilotStartRequest) async throws -> [String: Any] {
        guard state == .idle || state == .error else { throw BridgeError(code: "ALREADY_RUNNING", message: "Phiên copilot đang chạy.") }
        generation += 1
        let gen = generation
        request = req
        lastError = nil
        lastOthersErrorCode = nil
        setState(.starting)
        do {
            let baseUrl = try CopilotStartRequest.resolveBaseUrl(requested: req.apiBaseUrl, fallback: deps.defaultBackendURL())
            guard await deps.permissions.ensureMicrophone() else {
                throw BridgeError(code: "PERMISSION_DENIED", message: "Chưa được cấp quyền micro. Mở System Settings > Privacy & Security > Microphone.")
            }
            try checkpoint(gen)

            let json = CopilotConfigBuilder.json(useFakeStt: deps.useFakeStt(), languages: req.languages)
            let engine: CopilotEngineType
            do { engine = try deps.engineFactory(json) }
            catch let e as CopilotEngineError {
                if case .unavailable = e { throw BridgeError(code: "ENGINE_UNAVAILABLE", message: e.localizedDescription) }
                throw BridgeError(code: "CAPTURE_FAILED", message: e.localizedDescription)
            }
            engine.setEventHandler { [weak self] ev in
                // Rust/engine thread -> main queue (FIFO, keeps event order).
                DispatchQueue.main.async { MainActor.assumeIsolated { self?.handle(ev) } }
            }
            self.engine = engine
            do { try engine.start(CopilotStartParams(baseUrl: baseUrl, sessionId: req.sessionId, bearer: req.accessToken)) }
            catch { throw BridgeError(code: "CAPTURE_FAILED", message: error.localizedDescription) }
            try checkpoint(gen)

            // `.me`: microphone. Audio-thread closure captures the engine directly (no actor hops).
            let mic = deps.makeMic()
            let activity = self.activity
            mic.onSamples = { samples, rate in
                activity.record(rmsDbfs: Self.rmsDbfs(samples))
                _ = engine.pushAudio(channel: .me, samples: samples, sampleRate: rate)
            }
            mic.onError = { [weak self] err in
                DispatchQueue.main.async { MainActor.assumeIsolated {
                    self?.emit("copilot.error", ["channel": "me", "code": "CAPTURE_FAILED", "message": err.localizedDescription])
                } }
            }
            try mic.start(deviceUid: req.micDeviceUid ?? nonEmpty(deps.defaultMicDeviceUid()))
            self.mic = mic
            try checkpoint(gen)

            // `.others`: meeting app. Not fatal: the app may not be running / playing yet, then retry in the background.
            let bundle = req.appBundleId ?? nonEmpty(deps.defaultMeetingAppBundleId())
            if let bundle {
                if !(await tryStartOthers(engine: engine, bundleId: bundle, gen: gen)) { scheduleOthersRetry(engine: engine, bundleId: bundle, gen: gen) }
            }
            try checkpoint(gen)

            setState(.running)
            onSessionActive?(true, req)
            return copilotStatus()
        } catch {
            if (error as? BridgeError)?.code == "CANCELLED" { return copilotStatus() }   // stop() won the race and owns the state
            let e = NativeCaptureController.map(error)
            lastError = (e.code, e.message)
            await teardown()
            setState(.error)
            emit("copilot.error", ["channel": NSNull(), "code": e.code, "message": e.message, "fatal": true])
            throw e
        }
    }

    private func checkpoint(_ gen: Int) throws {
        if gen != generation { throw BridgeError(code: "CANCELLED", message: "Phiên copilot đã bị dừng trong lúc khởi động.") }
    }

    private func nonEmpty(_ s: String) -> String? { s.isEmpty ? nil : s }

    // MARK: Others channel

    private func tryStartOthers(engine: CopilotEngineType, bundleId: String, gen: Int) async -> Bool {
        guard gen == generation else { return true }
        do {
            guard let target = deps.resolveTarget(bundleId) else {
                throw BridgeError(code: "APP_NOT_RUNNING", message: "Ứng dụng \(bundleId) chưa chạy. Đang chờ…")
            }
            let (src, backend) = try await beginOthers(target: target, engine: engine)
            guard gen == generation else { src.stop(); return true }
            others = src
            othersBackend = backend
            lastOthersErrorCode = nil
            setOthers(.running)
            return true
        } catch {
            guard gen == generation else { return true }
            let e = NativeCaptureController.map(error)
            setOthers(.waiting)
            if lastOthersErrorCode != e.code {       // do not spam the web app on every retry
                lastOthersErrorCode = e.code
                emit("copilot.error", ["channel": "others", "code": e.code, "message": e.message, "fatal": false])
            }
            return false
        }
    }

    /// Process Tap first; ScreenCaptureKit only when the tap failed for a reason other than "no audio process yet"
    /// (avoids a Screen Recording prompt while the meeting simply has not started).
    private func beginOthers(target: CaptureTarget, engine: CopilotEngineType) async throws -> (AudioCaptureSource, String) {
        let primary = deps.makeProcessTap()
        do {
            try await begin(primary, target: target, engine: engine)
            return (primary, "processTap")
        } catch let first {
            if case CaptureError.noAudioProcess = first { throw first }
            if let b = first as? BridgeError, b.code == "NO_AUDIO_PROCESS" { throw first }
            emit("log", ["message": "Process Tap lỗi (\(first.localizedDescription)) → thử ScreenCaptureKit"])
            let fallback = deps.makeScreenCaptureKit()
            do {
                try await begin(fallback, target: target, engine: engine)
                return (fallback, "screenCaptureKit")
            } catch { throw first }
        }
    }

    private func begin(_ src: AudioCaptureSource, target: CaptureTarget, engine: CopilotEngineType) async throws {
        let activity = self.activity
        src.onBuffer = { buf in
            // Capture thread (IOProc / SCK queue): straight into the engine.
            activity.record(rmsDbfs: Resampler.rmsDbfs(buf))
            let samples = AudioSamples.floats(buf)
            if !samples.isEmpty { _ = engine.pushAudio(channel: .others, samples: samples, sampleRate: UInt32(buf.format.sampleRate)) }
        }
        src.onError = { [weak self] err in
            DispatchQueue.main.async { MainActor.assumeIsolated {
                self?.emit("copilot.error", ["channel": "others", "code": "CAPTURE_FAILED", "message": err.localizedDescription, "fatal": false])
            } }
        }
        try await src.start(target: target)
    }

    private func scheduleOthersRetry(engine: CopilotEngineType, bundleId: String, gen: Int) {
        let interval = deps.othersRetryNanos
        othersRetry = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: interval)
                guard !Task.isCancelled, let self, gen == self.generation else { return }
                if await self.tryStartOthers(engine: engine, bundleId: bundleId, gen: gen) { return }
            }
        }
    }

    // MARK: Stop

    @discardableResult
    func stopCopilotSession() async -> [String: Any] {
        guard state != .idle else { return copilotStatus() }
        generation += 1                       // aborts a start() that is still awaiting
        setState(.stopping)
        await teardown()
        lastError = nil
        request = nil
        setState(.idle)
        return copilotStatus()
    }

    private func teardown() async {
        othersRetry?.cancel(); othersRetry = nil
        mic?.stop(); mic = nil                // no more pushes after this
        others?.stop(); others = nil
        othersBackend = ""
        othersState = .off
        let e = engine
        engine = nil
        onSessionActive?(false, nil)
        if let e {
            // Flushes pending segments to the backend (up to the sink's flush timeout): never on the main actor.
            let result: Result<Void, Error> = await Task.detached { Result { try e.stop() } }.value
            if case .failure(let err) = result {
                emit("copilot.error", ["channel": NSNull(), "code": "CAPTURE_FAILED", "message": "Dừng engine lỗi: \(err.localizedDescription)", "fatal": false])
            }
        }
    }

    // MARK: Engine events

    private func handle(_ ev: CopilotEngineEvent) {
        let w = CopilotEventMapper.wire(ev)
        emit(w.type, w.payload)
        onOverlayEvent?(ev)
        if case .error(_, let code, let message) = ev, CopilotEventMapper.isAuthError(code: code, message: message) {
            emit("auth-required", ["reason": "copilot"])      // web app should call updateAuth({accessToken})
        }
    }

    nonisolated static func rmsDbfs(_ s: [Float]) -> Double {
        guard !s.isEmpty else { return -120 }
        var sum: Float = 0
        for v in s { sum += v * v }
        let rms = (sum / Float(s.count)).squareRoot()
        return rms > 0 ? max(-120, Double(20 * log10(rms))) : -120
    }
}
