import XCTest
@testable import WarpTalk

final class CopilotBridgeTests: XCTestCase {
    private func req(_ method: String, _ params: [String: Any]) throws -> BridgeRequest {
        try XCTUnwrap(BridgeRequest(["method": method, "params": params]))
    }

    // MARK: startCopilotSession parsing

    func testStartRequestMinimal() throws {
        let r = try CopilotStartRequest.parse(req("startCopilotSession", ["sessionId": "s1", "accessToken": "jwt", "apiBaseUrl": "http://localhost:8080"]))
        XCTAssertEqual(r.sessionId, "s1"); XCTAssertEqual(r.accessToken, "jwt"); XCTAssertEqual(r.apiBaseUrl, "http://localhost:8080")
        XCTAssertNil(r.appBundleId); XCTAssertNil(r.micDeviceUid); XCTAssertNil(r.languages)
    }

    func testStartRequestOptional() throws {
        let r = try CopilotStartRequest.parse(req("startCopilotSession", [
            "sessionId": "s1", "accessToken": "jwt", "apiBaseUrl": "http://h", "appBundleId": "us.zoom.xos", "micDeviceUid": "mic-1",
            "languages": ["vi", "en"], "speakerNames": ["me": "Lan", "others": ""],
        ]))
        XCTAssertEqual(r.appBundleId, "us.zoom.xos"); XCTAssertEqual(r.micDeviceUid, "mic-1"); XCTAssertEqual(r.languages, ["vi", "en"])
        XCTAssertEqual(r.meName, "Lan"); XCTAssertNil(r.othersName)
    }

    func testStartRequestMissingRequired() throws {
        for missing in ["sessionId", "accessToken", "apiBaseUrl"] {
            var p: [String: Any] = ["sessionId": "s1", "accessToken": "jwt", "apiBaseUrl": "http://h"]
            p[missing] = nil
            XCTAssertThrowsError(try CopilotStartRequest.parse(req("startCopilotSession", p))) {
                XCTAssertEqual(($0 as? BridgeError)?.code, "INVALID_ARGUMENT", missing)
            }
        }
        XCTAssertThrowsError(try CopilotStartRequest.parse(req("startCopilotSession", ["sessionId": "s", "accessToken": "t", "apiBaseUrl": "http://h", "languages": "vi"])))
    }

    func testResolveBaseUrl() throws {
        XCTAssertEqual(try CopilotStartRequest.resolveBaseUrl(requested: "http://localhost:8080/", fallback: "x"), "http://localhost:8080")
        XCTAssertEqual(try CopilotStartRequest.resolveBaseUrl(requested: "https://api.example.com/api/v1", fallback: "x"), "https://api.example.com")
        // Bundled app reports warptalk://app: use the Settings backend instead.
        XCTAssertEqual(try CopilotStartRequest.resolveBaseUrl(requested: "warptalk://app", fallback: "http://localhost:8080"), "http://localhost:8080")
        XCTAssertThrowsError(try CopilotStartRequest.resolveBaseUrl(requested: "warptalk://app", fallback: "nope"))
    }

    // MARK: suggestions

    func testSuggestionParsing() throws {
        let s = try OverlaySuggestion.parse(req("pushOverlaySuggestion", ["id": "a", "category": "question", "content": "Hỏi về ngân sách", "followUp": "Ai duyệt?"]))
        XCTAssertEqual(s, OverlaySuggestion(id: "a", category: "question", content: "Hỏi về ngân sách", followUp: "Ai duyệt?", isFollowUp: false))
        let f = try OverlaySuggestion.parse(req("pushOverlaySuggestion", ["id": "b", "content": "x", "followUp": true]))
        XCTAssertTrue(f.isFollowUp); XCTAssertNil(f.followUp); XCTAssertEqual(f.category, "suggestion")
        XCTAssertThrowsError(try OverlaySuggestion.parse(req("pushOverlaySuggestion", ["content": "x"])))
        XCTAssertThrowsError(try OverlaySuggestion.parse(req("pushOverlaySuggestion", ["id": "a", "content": "x", "followUp": 3])))
    }

    // MARK: event -> JSON

    private func jsonRoundTrip(_ p: [String: Any]) throws -> [String: Any] {
        XCTAssertTrue(JSONSerialization.isValidJSONObject(p))
        let data = try JSONSerialization.data(withJSONObject: p)
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    func testPartialMapping() throws {
        let w = CopilotEventMapper.wire(.partial(channel: .others, text: "xin chào"))
        XCTAssertEqual(w.type, "copilot.partial")
        let j = try jsonRoundTrip(w.payload)
        XCTAssertEqual(j["channel"] as? String, "others"); XCTAssertEqual(j["text"] as? String, "xin chào")
    }

    func testSegmentMapping() throws {
        let seg = CopilotSegment(clientSegmentId: "c1", channel: .me, text: "ok", language: "vi", startMs: 1500, endMs: 4200,
                                 confidence: 0.5, isFinal: true, likelyEcho: false, utteranceSeq: 7)
        let w = CopilotEventMapper.wire(.segment(seg))
        XCTAssertEqual(w.type, "copilot.segment")
        let j = try jsonRoundTrip(w.payload)
        XCTAssertEqual(j["clientSegmentId"] as? String, "c1"); XCTAssertEqual(j["channel"] as? String, "me")
        XCTAssertEqual(j["startMs"] as? Int, 1500); XCTAssertEqual(j["endMs"] as? Int, 4200); XCTAssertEqual(j["utteranceSeq"] as? Int, 7)
        XCTAssertEqual(j["confidence"] as? Double, 0.5); XCTAssertEqual(j["isFinal"] as? Bool, true); XCTAssertEqual(j["likelyEcho"] as? Bool, false)
        var noConf = seg; noConf.confidence = nil
        XCTAssertTrue(try jsonRoundTrip(CopilotEventMapper.wire(.segment(noConf)).payload)["confidence"] is NSNull)
    }

    func testStatusErrorLevelsMapping() throws {
        let st = CopilotEventMapper.wire(.status(channel: nil, kind: .sttConnected, detail: nil))
        XCTAssertEqual(st.type, "copilot.status")
        var j = try jsonRoundTrip(st.payload)
        XCTAssertEqual(j["kind"] as? String, "sttConnected"); XCTAssertTrue(j["channel"] is NSNull); XCTAssertTrue(j["detail"] is NSNull)

        let st2 = CopilotEventMapper.wire(.status(channel: .others, kind: .audioDropped, detail: "3"))
        j = try jsonRoundTrip(st2.payload)
        XCTAssertEqual(j["channel"] as? String, "others"); XCTAssertEqual(j["detail"] as? String, "3"); XCTAssertEqual(j["kind"] as? String, "audioDropped")

        let er = CopilotEventMapper.wire(.error(channel: .me, code: "stt", message: "boom"))
        XCTAssertEqual(er.type, "copilot.error")
        j = try jsonRoundTrip(er.payload)
        XCTAssertEqual(j["code"] as? String, "stt"); XCTAssertEqual(j["message"] as? String, "boom"); XCTAssertEqual(j["channel"] as? String, "me")

        let lv = CopilotEventMapper.wire(.levels(channel: .others, rms: 0.25))
        XCTAssertEqual(lv.type, "copilot.levels")
        j = try jsonRoundTrip(lv.payload)
        XCTAssertEqual(j["rms"] as? Double, 0.25)
    }

    func testAuthErrorHeuristic() {
        XCTAssertTrue(CopilotEventMapper.isAuthError(code: "sink", message: "HTTP 401 Unauthorized"))
        XCTAssertFalse(CopilotEventMapper.isAuthError(code: "stt", message: "connection reset"))
    }

    // MARK: config

    func testConfigJson() throws {
        func parse(_ s: String) throws -> [String: Any] { try XCTUnwrap(JSONSerialization.jsonObject(with: Data(s.utf8)) as? [String: Any]) }
        let real = try parse(CopilotConfigBuilder.json(useFakeStt: false, languages: nil))
        XCTAssertEqual((real["stt"] as? [String: Any])?["backend"] as? String, "realtime")
        XCTAssertNil((real["stt"] as? [String: Any])?["language_hint"])
        let fake = try parse(CopilotConfigBuilder.json(useFakeStt: true, languages: ["vi", "en"]))
        XCTAssertEqual((fake["stt"] as? [String: Any])?["backend"] as? String, "fake")
        XCTAssertEqual((fake["stt"] as? [String: Any])?["language_hint"] as? String, "vi")
    }

    func testBridgeScriptExposesCopilotMethods() {
        let js = BridgeScript.source
        for m in ["startCopilotSession", "stopCopilotSession", "copilotStatus", "pushOverlaySuggestion", "clearOverlaySuggestion"] {
            XCTAssertTrue(js.contains(m), m)
        }
    }

    func testOverlayPositionOrigins() {
        let f = CGRect(x: 0, y: 0, width: 1000, height: 800)
        let size = CGSize(width: 400, height: 200)
        XCTAssertEqual(OverlayPosition.bottomCenter.origin(in: f, size: size, margin: 20), CGPoint(x: 300, y: 20))
        XCTAssertEqual(OverlayPosition.topRight.origin(in: f, size: size, margin: 20), CGPoint(x: 580, y: 580))
        XCTAssertEqual(OverlayPosition.topLeft.origin(in: f, size: size, margin: 20), CGPoint(x: 20, y: 580))
    }
}
