import XCTest
@testable import WarpTalk

/// Fixtures mirror crates/modules/meeting/src/http/dto.rs and the kernel error body.
final class NativeAPITests: XCTestCase {
    private let grantJSON = #"{"roomName":"room-1","serverUrl":"wss://lk.example.com","token":"jwt","participantIdentity":"external-bridge"}"#
    private let errorJSON = #"{"code":"INVALID_STATE","message":"meeting has already ended"}"#

    func testGrantDecoding() throws {
        let g = try APIDates.decoder().decode(MeetingGrant.self, from: Data(grantJSON.utf8))
        XCTAssertEqual(g.serverUrl, "wss://lk.example.com"); XCTAssertEqual(g.participantIdentity, "external-bridge")
    }

    func testErrorBody() {
        let e = NativeAPI.httpError(409, Data(errorJSON.utf8))
        XCTAssertEqual(e.code, "INVALID_STATE"); XCTAssertEqual(e.status, 409)
        XCTAssertEqual(NativeAPI.httpError(422, Data("plain".utf8)).code, "HTTP_422")
    }

    func testURLBuildingTrimsSlash() throws {
        let api = NativeAPI(baseURL: "http://localhost:8080/", accessToken: "t")
        XCTAssertEqual(try api.makeURL("/meetings/x/bridge-token").absoluteString, "http://localhost:8080/api/v1/meetings/x/bridge-token")
    }

    func testHeartbeatEncodingIsCamelCase() throws {
        let d = Date(timeIntervalSince1970: 1_795_255_200.5)
        let data = try APIDates.encoder().encode(BridgeHeartbeatRequest(audioFlowing: true, lastAudioAt: d, rmsDbfs: -42))
        let o = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(o["audioFlowing"] as? Bool, true); XCTAssertEqual(o["rmsDbfs"] as? Double, -42)
        XCTAssertEqual(o["lastAudioAt"] as? String, "2026-11-21T10:00:00.500Z")
    }

    func testChronoNanosecondDates() throws {
        XCTAssertEqual(try XCTUnwrap(APIDates.parse("2026-11-21T10:00:00.123456789Z")).timeIntervalSince1970, 1_795_255_200.123, accuracy: 0.01)
        XCTAssertNotNil(APIDates.parse("2026-11-21T10:00:00Z"))
        XCTAssertNotNil(APIDates.parse("2026-11-21T10:00:00.5Z"))
    }

    // MARK: request building (contract: clientTag, bearer token, endpoints)

    func testJoinRequestCarriesBridgeListenerClientTag() throws {
        let api = NativeAPI(baseURL: "http://localhost:8080", accessToken: "tok-1")
        let body = try NativeAPI.joinBody(clientTag: "bridge-listener")
        let req = try api.makeRequest("POST", "/meetings/room-1/join", body: body)
        XCTAssertEqual(req.url?.absoluteString, "http://localhost:8080/api/v1/meetings/room-1/join")
        XCTAssertEqual(req.httpMethod, "POST")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer tok-1")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Content-Type"), "application/json")
        let o = try XCTUnwrap(JSONSerialization.jsonObject(with: try XCTUnwrap(req.httpBody)) as? [String: String])
        XCTAssertEqual(o, ["clientTag": "bridge-listener"])
    }

    func testTokenUpdateIsUsedByNextRequest() throws {
        let api = NativeAPI(baseURL: "http://x", accessToken: "old")
        api.updateToken("new")
        XCTAssertEqual(try api.makeRequest("POST", "/a").value(forHTTPHeaderField: "Authorization"), "Bearer new")
        XCTAssertNil(try api.makeRequest("POST", "/a").httpBody)
    }

    func testReleaseAndHeartbeatPaths() throws {
        let api = NativeAPI(baseURL: "https://api.example.com", accessToken: "t")
        XCTAssertEqual(try api.makeRequest("POST", "/translation-rooms/r1/bridge/capturer/release").url?.absoluteString,
                       "https://api.example.com/api/v1/translation-rooms/r1/bridge/capturer/release")
        XCTAssertEqual(try api.makeRequest("POST", "/translation-rooms/r1/bridge/capturer/heartbeat").url?.path,
                       "/api/v1/translation-rooms/r1/bridge/capturer/heartbeat")
    }

    func testHeartbeatBodyOmitsNilLastAudioAt() throws {
        let data = try APIDates.encoder().encode(BridgeHeartbeatRequest(audioFlowing: false, lastAudioAt: nil, rmsDbfs: -120))
        let o = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(Set(o.keys), ["audioFlowing", "rmsDbfs"])
        XCTAssertEqual(o["audioFlowing"] as? Bool, false)
    }

    // MARK: lease (heartbeat / release response = room BridgeStateResponse, camelCase, RFC 3339 nanoseconds)

    func testLeaseFieldsDecodeAndRemainingIsServerRelative() throws {
        let json = #"{"roomId":"6f0c0c52-0000-7000-8000-000000000001","provider":"meet","meetingCode":"abc-defg-hij","audioFlowing":true,"epoch":3,"capturerUserId":"u1","leaseExpiresAt":"2026-11-22T10:00:45.123456789Z","serverTime":"2026-11-22T10:00:15.100000000Z","heartbeatIntervalSeconds":15,"leaseSeconds":45}"#
        let st = try APIDates.decoder().decode(BridgeStateResponse.self, from: Data(json.utf8))
        XCTAssertEqual(st.epoch, 3); XCTAssertEqual(st.heartbeatIntervalSeconds, 15); XCTAssertEqual(st.leaseSeconds, 45)
        XCTAssertEqual(try XCTUnwrap(st.leaseRemainingSeconds), 30.02, accuracy: 0.05)
    }

    func testLeaseNullWithoutCapturer() throws {
        let json = #"{"roomId":"r","leaseExpiresAt":null,"serverTime":"2026-11-22T10:00:15Z"}"#
        let st = try APIDates.decoder().decode(BridgeStateResponse.self, from: Data(json.utf8))
        XCTAssertNil(st.leaseExpiresAt); XCTAssertNil(st.leaseRemainingSeconds)
    }

    func testHeartbeatFailureClassificationUsesErrorCode() {
        func http(_ s: Int, _ c: String) -> Error { APIError.http(status: s, code: c, message: "m") }
        XCTAssertEqual(HeartbeatFailure.classify(http(401, "UNAUTHORIZED")), .authRequired)
        if case .leaseLost = HeartbeatFailure.classify(http(409, "CAPTURER_LEASE_LOST")) {} else { XCTFail("lease lost") }
        // audit R-08: other 409s must NOT be reported as "lost capturer"
        if case .conflict(let code, _) = HeartbeatFailure.classify(http(409, "NOT_BRIDGE_ROOM")) { XCTAssertEqual(code, "NOT_BRIDGE_ROOM") } else { XCTFail("conflict") }
        if case .conflict(let code, _) = HeartbeatFailure.classify(http(409, "INVALID_STATE")) { XCTAssertEqual(code, "INVALID_STATE") } else { XCTFail("conflict") }
        if case .transient = HeartbeatFailure.classify(http(503, "UNAVAILABLE")) {} else { XCTFail("transient") }
        if case .transient = HeartbeatFailure.classify(APIError.transport(URLError(.notConnectedToInternet))) {} else { XCTFail("transient") }
    }
}
