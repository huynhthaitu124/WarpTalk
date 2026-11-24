import AVFoundation
import XCTest
@testable import WarpTalk

// Fakes ---------------------------------------------------------------------------------------------------------------------

private final class FakePermissions: CopilotPermissionProviding {
    var granted = true
    var asked = 0
    func ensureMicrophone() async -> Bool { asked += 1; return granted }
}

private final class FakeMic: MicCapturing, @unchecked Sendable {
    var onSamples: (@Sendable ([Float], UInt32) -> Void)?
    var onError: (@Sendable (Error) -> Void)?
    var startedWith: String??
    var stopped = false
    var failStart: Error?
    func start(deviceUid: String?) throws { if let failStart { throw failStart }; startedWith = .some(deviceUid) }
    func stop() { stopped = true }
}

private final class FakeSource: AudioCaptureSource, @unchecked Sendable {
    let displayName = "fake"
    var onBuffer: (@Sendable (AVAudioPCMBuffer) -> Void)?
    var onError: (@Sendable (Error) -> Void)?
    var failStart: Error?
    var started = false, stopped = false
    func start(target: CaptureTarget) async throws { if let failStart { throw failStart }; started = true }
    func stop() { stopped = true }
    func deliver(frames: Int = 480) {
        guard let b = AVAudioPCMBuffer(pcmFormat: BridgeAudioFormat.format, frameCapacity: AVAudioFrameCount(frames)) else { return }
        b.frameLength = AVAudioFrameCount(frames)
        if let p = b.floatChannelData?[0] { for i in 0..<frames { p[i] = 0.1 } }
        onBuffer?(b)
    }
}

@MainActor
final class CopilotSessionControllerTests: XCTestCase {
    private var engine = FakeCopilotEngine()
    private var mic = FakeMic()
    private var tap = FakeSource()
    private var sck = FakeSource()
    private var perms = FakePermissions()
    private var appRunning = true
    private var emitted: [(type: String, payload: [String: Any])] = []
    private var overlayEvents: [CopilotEngineEvent] = []
    private var active: [Bool] = []
    private var factoryJson: [String] = []
    private var factoryError: Error?
    private var fakeStt = false

    private let target = CaptureTarget(bundleID: "com.google.Chrome", pid: 42, name: "Chrome", extraAudioPrefixes: [])

    override func setUp() {
        engine = FakeCopilotEngine(); mic = FakeMic(); tap = FakeSource(); sck = FakeSource(); perms = FakePermissions()
        appRunning = true; emitted = []; overlayEvents = []; active = []; factoryJson = []; factoryError = nil; fakeStt = false
    }

    private func makeController(retryNanos: UInt64 = 10_000_000) -> CopilotSessionController {
        let engine = self.engine, mic = self.mic, tap = self.tap, sck = self.sck
        let box = FactoryBox()
        box.owner = self
        let deps = CopilotDependencies(
            engineFactory: { json in try box.make(json, engine) },
            makeMic: { mic }, makeProcessTap: { tap }, makeScreenCaptureKit: { sck },
            resolveTarget: { [unowned self] id in self.appRunning && id == "com.google.Chrome" ? self.target : nil },
            permissions: perms,
            defaultBackendURL: { "http://localhost:8080" },
            defaultMeetingAppBundleId: { "com.google.Chrome" },
            defaultMicDeviceUid: { "" },
            useFakeStt: { [unowned self] in self.fakeStt },
            othersRetryNanos: retryNanos)
        let c = CopilotSessionController(deps: deps)
        c.emit = { [unowned self] t, p in self.emitted.append((t, p)) }
        c.onOverlayEvent = { [unowned self] e in self.overlayEvents.append(e) }
        c.onSessionActive = { [unowned self] a, _ in self.active.append(a) }
        return c
    }

    /// Lets the factory closure (Sendable) reach test state on the main actor.
    private final class FactoryBox: @unchecked Sendable {
        weak var owner: CopilotSessionControllerTests?
        func make(_ json: String, _ engine: FakeCopilotEngine) throws -> CopilotEngineType {
            let o = owner
            return try MainActor.assumeIsolated {
                o?.factoryJson.append(json)
                if let e = o?.factoryError { throw e }
                engine.configJson = json
                return engine
            }
        }
    }

    private let req = CopilotStartRequest(sessionId: "s1", accessToken: "jwt1", apiBaseUrl: "http://localhost:8080/")

    private func drainMain() async { await withCheckedContinuation { c in DispatchQueue.main.async { c.resume() } } }
    private func types() -> [String] { emitted.map(\.type) }

    // Tests ---------------------------------------------------------------------------------------------------------------

    func testStartRunsBothChannelsAndStopFlushes() async throws {
        let c = makeController()
        let st = try await c.startCopilotSession(req)
        XCTAssertEqual(c.state, .running)
        XCTAssertEqual(st["state"] as? String, "running")
        XCTAssertEqual((st["channels"] as? [String: String])?["me"], "running")
        XCTAssertEqual((st["channels"] as? [String: String])?["others"], "running")
        XCTAssertEqual(st["backend"] as? String, "processTap")
        XCTAssertEqual(engine.startParams, CopilotStartParams(baseUrl: "http://localhost:8080", sessionId: "s1", bearer: "jwt1"))
        XCTAssertTrue(factoryJson[0].contains("\"realtime\""))
        XCTAssertEqual(active, [true])

        // Audio threads push straight into the engine.
        mic.onSamples?([0.1, 0.2], 48_000)
        tap.deliver()
        tap.deliver()
        XCTAssertEqual(engine.pushedCount(.me), 1); XCTAssertEqual(engine.pushedCount(.others), 2)
        XCTAssertTrue(types().contains("status"))

        let end = await c.stopCopilotSession()
        XCTAssertEqual(c.state, .idle); XCTAssertEqual(end["state"] as? String, "idle")
        XCTAssertEqual(engine.stopCalls, 1)
        XCTAssertTrue(mic.stopped); XCTAssertTrue(tap.stopped)
        XCTAssertEqual(active, [true, false])
        // stop on idle is a no-op
        _ = await c.stopCopilotSession()
        XCTAssertEqual(engine.stopCalls, 1)
    }

    func testFakeSttSettingReachesConfig() async throws {
        fakeStt = true
        let c = makeController()
        _ = try await c.startCopilotSession(req)
        XCTAssertTrue(factoryJson[0].contains("\"fake\""))
        await c.stopCopilotSession()
    }

    func testAlreadyRunning() async throws {
        let c = makeController()
        _ = try await c.startCopilotSession(req)
        do { _ = try await c.startCopilotSession(req); XCTFail("expected ALREADY_RUNNING") }
        catch { XCTAssertEqual((error as? BridgeError)?.code, "ALREADY_RUNNING") }
        XCTAssertEqual(c.state, .running)
        await c.stopCopilotSession()
    }

    func testMicPermissionDenied() async {
        perms.granted = false
        let c = makeController()
        do { _ = try await c.startCopilotSession(req); XCTFail("expected PERMISSION_DENIED") }
        catch { XCTAssertEqual((error as? BridgeError)?.code, "PERMISSION_DENIED") }
        XCTAssertEqual(c.state, .error)
        XCTAssertTrue(factoryJson.isEmpty)                         // engine never created
        XCTAssertEqual((c.copilotStatus()["error"] as? [String: String])?["code"], "PERMISSION_DENIED")
        XCTAssertTrue(emitted.contains { $0.type == "copilot.error" && $0.payload["fatal"] as? Bool == true })
    }

    func testEngineUnavailableMapsToCode() async {
        factoryError = CopilotEngineError.unavailable
        let c = makeController()
        do { _ = try await c.startCopilotSession(req); XCTFail() }
        catch { XCTAssertEqual((error as? BridgeError)?.code, "ENGINE_UNAVAILABLE") }
        XCTAssertEqual(c.state, .error)
    }

    func testEngineStartFailureTearsDownAndCanRestart() async throws {
        engine.failStart = CopilotEngineError.failed("bad config")
        let c = makeController()
        do { _ = try await c.startCopilotSession(req); XCTFail() }
        catch { XCTAssertEqual((error as? BridgeError)?.code, "CAPTURE_FAILED") }
        XCTAssertEqual(c.state, .error)
        XCTAssertNil(mic.startedWith)                               // mic was never started
        // Recover: the same controller can start again after an error.
        engine.failStart = nil
        _ = try await c.startCopilotSession(req)
        XCTAssertEqual(c.state, .running)
        await c.stopCopilotSession()
    }

    func testMicFailureIsFatalAndStopsEngine() async {
        mic.failStart = BridgeError(code: "CAPTURE_FAILED", message: "no mic")
        let c = makeController()
        do { _ = try await c.startCopilotSession(req); XCTFail() }
        catch { XCTAssertEqual((error as? BridgeError)?.code, "CAPTURE_FAILED") }
        XCTAssertEqual(c.state, .error)
        XCTAssertEqual(engine.stopCalls, 1)
    }

    func testOthersWaitsForMeetingAppThenStarts() async throws {
        appRunning = false
        let c = makeController()
        _ = try await c.startCopilotSession(req)
        XCTAssertEqual(c.state, .running)                           // not fatal
        XCTAssertEqual(c.othersState, .waiting)
        XCTAssertEqual(emitted.filter { $0.type == "copilot.error" && $0.payload["code"] as? String == "APP_NOT_RUNNING" }.count, 1)
        appRunning = true
        for _ in 0..<100 where c.othersState != .running { try await Task.sleep(nanoseconds: 20_000_000) }
        XCTAssertEqual(c.othersState, .running)
        XCTAssertTrue(tap.started)
        // The APP_NOT_RUNNING error was not repeated by retries.
        XCTAssertEqual(emitted.filter { $0.type == "copilot.error" }.count, 1)
        await c.stopCopilotSession()
    }

    func testOthersNoAudioProcessDoesNotFallBackToScreenCaptureKit() async throws {
        tap.failStart = CaptureError.noAudioProcess("Chrome")
        let c = makeController(retryNanos: 5_000_000_000)
        _ = try await c.startCopilotSession(req)
        XCTAssertEqual(c.othersState, .waiting)
        XCTAssertFalse(sck.started)
        XCTAssertTrue(emitted.contains { $0.payload["code"] as? String == "NO_AUDIO_PROCESS" })
        await c.stopCopilotSession()
    }

    func testOthersFallsBackToScreenCaptureKit() async throws {
        tap.failStart = CaptureError.coreAudio("tap", -1)
        let c = makeController()
        let st = try await c.startCopilotSession(req)
        XCTAssertTrue(sck.started)
        XCTAssertEqual(st["backend"] as? String, "screenCaptureKit")
        XCTAssertEqual(c.othersState, .running)
        await c.stopCopilotSession()
    }

    func testAuthRefreshForwardsBearer() async throws {
        let c = makeController()
        c.updateBearer("ignored-while-idle")                        // no engine yet: no crash
        _ = try await c.startCopilotSession(req)
        c.updateBearer("jwt2")
        XCTAssertEqual(engine.bearers, ["jwt2"])
        await c.stopCopilotSession()
    }

    func testEventsForwardedToWebAndOverlayInOrder() async throws {
        let c = makeController()
        _ = try await c.startCopilotSession(req)
        await drainMain()                                           // the engine's own "started" status arrives asynchronously
        emitted.removeAll(); overlayEvents.removeAll()
        engine.emit(.partial(channel: .others, text: "a"))
        engine.emit(.segment(CopilotSegment(clientSegmentId: "x", channel: .others, text: "ab", language: "vi", startMs: 0, endMs: 10,
                                            confidence: nil, isFinal: true, likelyEcho: false, utteranceSeq: 1)))
        engine.emit(.levels(channel: .me, rms: 0.1))
        engine.emit(.status(channel: nil, kind: .sttConnected, detail: nil))
        await drainMain()
        XCTAssertEqual(types(), ["copilot.partial", "copilot.segment", "copilot.levels", "copilot.status"])
        XCTAssertEqual(overlayEvents.count, 4)
        await c.stopCopilotSession()
    }

    func testEngineAuthErrorRequestsNewToken() async throws {
        let c = makeController()
        _ = try await c.startCopilotSession(req)
        await drainMain()
        emitted.removeAll()
        engine.emit(.error(channel: nil, code: "sink", message: "HTTP 401 Unauthorized"))
        await drainMain()
        XCTAssertEqual(types(), ["copilot.error", "auth-required"])
        XCTAssertEqual(emitted[1].payload["reason"] as? String, "copilot")
        emitted.removeAll()
        engine.emit(.error(channel: .me, code: "stt", message: "reset"))
        await drainMain()
        XCTAssertEqual(types(), ["copilot.error"])
        await c.stopCopilotSession()
    }

    func testStopFailureIsReportedButStateGoesIdle() async throws {
        let c = makeController()
        _ = try await c.startCopilotSession(req)
        engine.failStop = CopilotEngineError.failed("flush timeout")
        emitted.removeAll()
        await c.stopCopilotSession()
        XCTAssertEqual(c.state, .idle)
        XCTAssertTrue(emitted.contains { $0.type == "copilot.error" })
    }

    func testStatusEventsFollowStateChanges() async throws {
        let c = makeController()
        _ = try await c.startCopilotSession(req)
        await c.stopCopilotSession()
        let states = emitted.filter { $0.type == "status" }.compactMap { $0.payload["state"] as? String }
        XCTAssertEqual(states.first, "starting")
        XCTAssertTrue(states.contains("running"))
        XCTAssertEqual(Array(states.suffix(2)), ["stopping", "idle"])
    }
}
