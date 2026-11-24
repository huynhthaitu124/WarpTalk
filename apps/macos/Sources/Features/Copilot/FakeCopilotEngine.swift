import Foundation

/// In-memory `CopilotEngineType` for unit tests, SwiftUI previews and demos. Records calls; `emit` simulates core events.
final class FakeCopilotEngine: CopilotEngineType, @unchecked Sendable {
    private let lock = NSLock()
    private var handler: (@Sendable (CopilotEngineEvent) -> Void)?
    private var _startParams: CopilotStartParams?
    private var _bearers: [String] = []
    private var _pushed: [CopilotChannel: Int] = [.me: 0, .others: 0]
    private var _stopCalls = 0
    private var _started = false

    var configJson: String = ""
    var failStart: Error?
    var failStop: Error?

    var startParams: CopilotStartParams? { lock.lock(); defer { lock.unlock() }; return _startParams }
    var bearers: [String] { lock.lock(); defer { lock.unlock() }; return _bearers }
    var stopCalls: Int { lock.lock(); defer { lock.unlock() }; return _stopCalls }
    func pushedCount(_ c: CopilotChannel) -> Int { lock.lock(); defer { lock.unlock() }; return _pushed[c] ?? 0 }

    func setEventHandler(_ handler: @escaping @Sendable (CopilotEngineEvent) -> Void) {
        lock.lock(); self.handler = handler; lock.unlock()
    }

    func start(_ params: CopilotStartParams) throws {
        if let failStart { throw failStart }
        lock.lock(); _startParams = params; _started = true; lock.unlock()
        emit(.status(channel: nil, kind: .started, detail: nil))
    }

    func pushAudio(channel: CopilotChannel, samples: [Float], sampleRate: UInt32) -> CopilotPushStatus {
        lock.lock(); defer { lock.unlock() }
        guard _started else { return .closed }
        _pushed[channel, default: 0] += 1
        return .accepted
    }

    func updateBearer(_ bearer: String) { lock.lock(); _bearers.append(bearer); lock.unlock() }

    func stop() throws {
        lock.lock(); _stopCalls += 1; _started = false; lock.unlock()
        if let failStop { throw failStop }
        emit(.status(channel: nil, kind: .stopped, detail: nil))
    }

    func emit(_ e: CopilotEngineEvent) {
        lock.lock(); let h = handler; lock.unlock()
        h?(e)
    }

    /// Canned conversation for previews / demos.
    func emitDemoConversation() {
        emit(.partial(channel: .others, text: "Chúng ta cần chốt lịch ra mắt"))
        emit(.segment(CopilotSegment(clientSegmentId: "demo-1", channel: .others, text: "Chúng ta cần chốt lịch ra mắt trước thứ Sáu.", language: "vi",
                                     startMs: 0, endMs: 3200, confidence: 0.92, isFinal: true, likelyEcho: false, utteranceSeq: 1)))
        emit(.segment(CopilotSegment(clientSegmentId: "demo-2", channel: .me, text: "Được, tôi sẽ gửi bản cập nhật chiều nay.", language: "vi",
                                     startMs: 3600, endMs: 6100, confidence: 0.9, isFinal: true, likelyEcho: false, utteranceSeq: 2)))
    }
}
