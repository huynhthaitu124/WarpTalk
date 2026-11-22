import XCTest
@testable import WarpTalk

/// Drives the real `NativeAPI` over a stubbed URLSession (URLProtocol): asserts what actually goes on the wire.
final class NativeAPIRoundTripTests: XCTestCase {
    final class Stub: URLProtocol {
        static var handler: ((URLRequest) -> (Int, Data))?
        static var seen: [URLRequest] = []
        static var bodies: [Data] = []

        override class func canInit(with request: URLRequest) -> Bool { true }
        override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
        override func startLoading() {
            var body = request.httpBody
            if body == nil, let s = request.httpBodyStream {   // URLSession moves httpBody into a stream for URLProtocol
                s.open(); defer { s.close() }
                var d = Data(); var buf = [UInt8](repeating: 0, count: 4096)
                while s.hasBytesAvailable { let n = s.read(&buf, maxLength: buf.count); if n <= 0 { break }; d.append(buf, count: n) }
                body = d
            }
            Self.seen.append(request); Self.bodies.append(body ?? Data())
            let (status, data) = Self.handler?(request) ?? (500, Data())
            let resp = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "application/json"])!
            client?.urlProtocol(self, didReceive: resp, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        }
        override func stopLoading() {}
    }

    private func api() -> NativeAPI {
        Stub.seen = []; Stub.bodies = []
        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [Stub.self]
        return NativeAPI(baseURL: "http://localhost:8080", accessToken: "tok", session: URLSession(configuration: cfg))
    }

    private let stateJSON = #"{"roomId":"r1","leaseExpiresAt":"2026-11-22T10:01:00.000Z","serverTime":"2026-11-22T10:00:30.000Z","heartbeatIntervalSeconds":15,"leaseSeconds":45}"#

    func testMeetingJoinSendsBridgeListenerTag() async throws {
        let api = api()
        Stub.handler = { _ in (200, Data(#"{"roomName":"r","serverUrl":"ws://lk","token":"jwt","participantIdentity":"u:bridge-listener"}"#.utf8)) }
        let g = try await api.meetingJoin(roomId: "r1")
        XCTAssertEqual(g.participantIdentity, "u:bridge-listener")
        XCTAssertEqual(Stub.seen.first?.url?.path, "/api/v1/meetings/r1/join")
        let o = try XCTUnwrap(JSONSerialization.jsonObject(with: Stub.bodies[0]) as? [String: String])
        XCTAssertEqual(o, ["clientTag": "bridge-listener"])
    }

    func testBridgeTokenHasNoBody() async throws {
        let api = api()
        Stub.handler = { _ in (200, Data(#"{"roomName":"r","serverUrl":"ws://lk","token":"jwt","participantIdentity":"stand-in"}"#.utf8)) }
        _ = try await api.bridgeToken(roomId: "r1")
        XCTAssertEqual(Stub.seen.first?.url?.path, "/api/v1/meetings/r1/bridge-token")
        XCTAssertEqual(Stub.seen.first?.httpMethod, "POST")
        XCTAssertTrue(Stub.bodies[0].isEmpty)
    }

    func testHeartbeatPayloadAndLeaseResponse() async throws {
        let api = api()
        Stub.handler = { [stateJSON] _ in (200, Data(stateJSON.utf8)) }
        let at = Date(timeIntervalSince1970: 1_795_255_200)
        let st = try await api.heartbeat(roomId: "r1", BridgeHeartbeatRequest(audioFlowing: true, lastAudioAt: at, rmsDbfs: -33.5))
        XCTAssertEqual(Stub.seen.first?.url?.path, "/api/v1/translation-rooms/r1/bridge/capturer/heartbeat")
        let o = try XCTUnwrap(JSONSerialization.jsonObject(with: Stub.bodies[0]) as? [String: Any])
        XCTAssertEqual(o["audioFlowing"] as? Bool, true); XCTAssertEqual(o["rmsDbfs"] as? Double, -33.5)
        XCTAssertEqual(o["lastAudioAt"] as? String, "2026-11-21T10:00:00.000Z")
        XCTAssertEqual(try XCTUnwrap(st?.leaseRemainingSeconds), 30, accuracy: 0.01)
    }

    func testReleaseCallsReleaseEndpoint() async throws {
        let api = api()
        Stub.handler = { [stateJSON] _ in (200, Data(stateJSON.utf8)) }
        try await api.releaseCapturer(roomId: "r1")
        XCTAssertEqual(Stub.seen.count, 1)
        XCTAssertEqual(Stub.seen[0].httpMethod, "POST")
        XCTAssertEqual(Stub.seen[0].url?.path, "/api/v1/translation-rooms/r1/bridge/capturer/release")
        XCTAssertEqual(Stub.seen[0].value(forHTTPHeaderField: "Authorization"), "Bearer tok")
    }

    func testHeartbeat409SurfacesBackendCode() async {
        let api = api()
        Stub.handler = { _ in (409, Data(#"{"code":"CAPTURER_LEASE_LOST","message":"another capturer"}"#.utf8)) }
        do {
            _ = try await api.heartbeat(roomId: "r1", BridgeHeartbeatRequest(audioFlowing: true, lastAudioAt: nil, rmsDbfs: nil))
            XCTFail("expected error")
        } catch {
            if case .leaseLost = HeartbeatFailure.classify(error) {} else { XCTFail("not classified as lease lost: \(error)") }
        }
    }
}
