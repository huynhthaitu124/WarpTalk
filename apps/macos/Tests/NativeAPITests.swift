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
}
