import XCTest
@testable import WarpTalk

final class BridgeProtocolTests: XCTestCase {
    func testRequestParsing() throws {
        let r = try XCTUnwrap(BridgeRequest(["method": "startExternalCapture", "params": ["roomId": "r1", "heartbeatIntervalSeconds": 15, "flag": true]]))
        XCTAssertEqual(r.method, "startExternalCapture")
        XCTAssertEqual(try r.string("roomId"), "r1")
        XCTAssertEqual(r.optDouble("heartbeatIntervalSeconds"), 15)
        XCTAssertEqual(r.optBool("flag"), true)
        XCTAssertThrowsError(try r.string("missing")) { XCTAssertEqual(($0 as? BridgeError)?.code, "INVALID_ARGUMENT") }
        XCTAssertNil(BridgeRequest("junk")); XCTAssertNil(BridgeRequest(["params": [:]]))
    }

    func testReplyShapes() {
        let ok = BridgeReply.ok(["a": 1])
        XCTAssertEqual(ok["ok"] as? Bool, true)
        let bad = BridgeReply.fail("X", "msg")
        XCTAssertEqual(bad["ok"] as? Bool, false)
        XCTAssertEqual((bad["error"] as? [String: String])?["code"], "X")
    }

    func testOriginPolicy() {
        XCTAssertTrue(OriginPolicy.isTrusted(scheme: "http", host: "localhost", port: 5173, allowed: "http://localhost:5173"))
        XCTAssertTrue(OriginPolicy.isTrusted(scheme: "http", host: "LOCALHOST", port: 5173, allowed: "http://localhost:5173/app"))
        XCTAssertFalse(OriginPolicy.isTrusted(scheme: "http", host: "localhost", port: 3000, allowed: "http://localhost:5173"))
        XCTAssertFalse(OriginPolicy.isTrusted(scheme: "https", host: "evil.example", port: 443, allowed: "http://localhost:5173"))
        XCTAssertTrue(OriginPolicy.isTrusted(scheme: "https", host: "app.example.com", port: 0, allowed: "https://app.example.com"))
        XCTAssertFalse(OriginPolicy.isTrusted(scheme: "http", host: "localhost", port: 5173, allowed: "not a url"))
    }

    func testInjectedScriptExposesContract() {
        for name in ["getCapabilities", "listAudioApps", "startExternalCapture", "stopExternalCapture", "captureStatus", "updateAuth",
                     "showOverlay", "pushOverlayCaption", "clearOverlay", "setOverlayOptions", "permissions", "requestPermission", "openSystemSettings"] {
            XCTAssertTrue(BridgeScript.source.contains("\(name):"), "missing \(name)")
        }
    }

    @MainActor func testOverlayModelUpsert() {
        let m = OverlayModel()
        m.push(speaker: "A", text: "xin", translation: nil, segmentId: "s1", isFinal: false)
        m.push(speaker: "A", text: "xin chao", translation: "hello", segmentId: "s1", isFinal: true)
        m.push(speaker: "A", text: "xin", translation: nil, segmentId: "s1", isFinal: false)   // late interim ignored
        XCTAssertEqual(m.lines.count, 1); XCTAssertEqual(m.lines[0].translation, "hello"); XCTAssertTrue(m.lines[0].isFinal)
        for i in 0..<30 { m.push(speaker: "B", text: "t\(i)", translation: nil, segmentId: nil, isFinal: true) }
        XCTAssertLessThanOrEqual(m.lines.count, 20); XCTAssertEqual(m.recent(3).count, 3)
    }

    func testAudioActivityLivenessIsNotSpeech() {
        let a = AudioActivity()
        let t = Date(timeIntervalSince1970: 100)
        a.record(rmsDbfs: -90, now: t)   // silent but buffers flowing
        let s = a.snapshot(now: t.addingTimeInterval(1))
        XCTAssertTrue(s.flowing); XCTAssertNil(s.lastAudioAt)
        XCTAssertFalse(a.snapshot(now: t.addingTimeInterval(10)).flowing)
        a.record(rmsDbfs: -30, now: t.addingTimeInterval(11))
        XCTAssertNotNil(a.snapshot(now: t.addingTimeInterval(11.5)).lastAudioAt)
    }
}
