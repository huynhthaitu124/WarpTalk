#if canImport(CopilotCore)
import CopilotCore
import Foundation

// Real adapter over the UniFFI binding (UniFFI 0.29 proc-macro naming, crates/pipeline/copilot-core). Compiled only when the
// `CopilotCore` module is available (`make core`). Generated names are taken from the facade docs in
// crates/pipeline/copilot-core/src/lib.rs; anything not confirmed against the generated file is marked `// VERIFY:`.

/// The Rust handle keeps its listener strongly until the handle is dropped (copilot-core `set_listener`), so the
/// engine registers this weak forwarder instead of itself: no retain cycle between `CopilotHandle` and the engine.
private final class WeakCopilotListener: CopilotEventListener, @unchecked Sendable {
    weak var target: CopilotCoreEngine?
    init(_ target: CopilotCoreEngine) { self.target = target }
    func onEvent(event: CopilotEvent) { target?.receive(event) }
}

final class CopilotCoreEngine: CopilotEngineType, @unchecked Sendable {
    private let handle: CopilotHandle
    private let lock = NSLock()
    private var handler: (@Sendable (CopilotEngineEvent) -> Void)?

    init(configJson: String) throws {
        do { handle = try CopilotHandle(configJson: configJson) }          // verified: `CopilotHandle(configJson:)`
        catch { throw CopilotEngineError.failed("\(error)") }
        // Verified against bindings/swift snapshot: `setListener(listener:)`; events arrive on the Rust
        // `copilot-listener` thread (callers hop to the main actor).
        handle.setListener(listener: WeakCopilotListener(self))
    }

    func setEventHandler(_ handler: @escaping @Sendable (CopilotEngineEvent) -> Void) {
        lock.lock(); self.handler = handler; lock.unlock()
    }

    func start(_ p: CopilotStartParams) throws {
        do { try handle.start(params: StartParams(baseUrl: p.baseUrl, sessionId: p.sessionId, bearer: p.bearer)) }
        catch { throw CopilotEngineError.failed("\(error)") }
    }

    func pushAudio(channel: CopilotChannel, samples: [Float], sampleRate: UInt32) -> CopilotPushStatus {
        switch handle.pushAudio(channel: channel == .me ? .me : .others, samples: samples, sampleRate: sampleRate) {
        case .accepted: return .accepted
        case .droppedOldest: return .droppedOldest
        case .closed: return .closed
        case .ignored: return .ignored
        }   // VERIFY: exhaustive (add `@unknown default` if the generated enum is non-frozen in your toolchain)
    }

    func updateBearer(_ bearer: String) { handle.updateBearer(bearer: bearer) }

    func stop() throws {
        defer { lock.lock(); handler = nil; lock.unlock() }
        do { try handle.stop() } catch { throw CopilotEngineError.failed("\(error)") }
    }

    // MARK: events (called on the Rust `copilot-listener` thread via WeakCopilotListener)

    fileprivate func receive(_ event: CopilotEvent) {
        lock.lock(); let h = handler; lock.unlock()
        guard let h, let mapped = Self.map(event) else { return }
        h(mapped)
    }

    static func channel(_ c: Channel) -> CopilotChannel { c == .me ? .me : .others }

    static func kind(_ k: StatusKind) -> CopilotStatusKind {
        switch k {
        case .started: return .started
        case .stopped: return .stopped
        case .speechStart: return .speechStart
        case .speechEnd: return .speechEnd
        case .sttConnected: return .sttConnected
        case .sttUnavailable: return .sttUnavailable
        case .sinkOnline: return .sinkOnline
        case .sinkOffline: return .sinkOffline
        case .audioDropped: return .audioDropped
        }
    }

    static func map(_ e: CopilotEvent) -> CopilotEngineEvent? {
        switch e {
        case .partial(let ch, let text): return .partial(channel: channel(ch), text: text)
        case .segment(let s):
            return .segment(CopilotSegment(clientSegmentId: s.clientSegmentId, channel: channel(s.channel), text: s.text, language: s.language,
                                           startMs: s.startMs, endMs: s.endMs, confidence: s.confidence, isFinal: s.isFinal,
                                           likelyEcho: s.likelyEcho, utteranceSeq: s.utteranceSeq))
        case .status(let ch, let kind, let detail): return .status(channel: ch.map(channel), kind: Self.kind(kind), detail: detail)
        case .error(let ch, let code, let message): return .error(channel: ch.map(channel), code: code, message: message)
        case .levels(let ch, let rms): return .levels(channel: channel(ch), rms: rms)
        }
    }
}
#endif
