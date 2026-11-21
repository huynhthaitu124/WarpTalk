import XCTest
@testable import WarpTalk

final class APIDecodingTests: XCTestCase {
    private let dec = APIDates.decoder()
    private func d(_ s: String) -> Data { Data(s.utf8) }

    func testAuthResponseWithNanoseconds() throws {
        let r = try dec.decode(AuthResponse.self, from: d(Fixtures.auth))
        XCTAssertEqual(r.accessToken, "at"); XCTAssertEqual(r.refreshToken, "rt")
        XCTAssertEqual(r.accessTokenExpiresAt.timeIntervalSince1970, 1_795_255_200.123, accuracy: 0.01)
    }

    func testWorkspacePage() throws {
        let p = try dec.decode(Page<Workspace>.self, from: d(Fixtures.workspacePage))
        XCTAssertEqual(p.total, 1); XCTAssertEqual(p.items[0].myRole, "OWNER"); XCTAssertNil(p.items[0].description)
    }

    func testRoom() throws {
        let r = try dec.decode(TranslationRoom.self, from: d(Fixtures.room))
        XCTAssertEqual(r.roomCode.count, 12); XCTAssertEqual(r.statusEnum, .inProgress)
        XCTAssertEqual(r.targetLanguages, ["en", "ja"]); XCTAssertTrue(r.translationActive)
        XCTAssertNotNil(r.startedAt)
    }

    func testParticipantAndGrant() throws {
        XCTAssertEqual(try dec.decode(RoomParticipant.self, from: d(Fixtures.participant)).role, "HOST")
        let g = try dec.decode(MeetingGrant.self, from: d(Fixtures.grant))
        XCTAssertEqual(g.serverUrl, "wss://lk.example.com"); XCTAssertEqual(g.participantIdentity, "user-1")
    }

    func testBridgeClaimBothSpellings() throws {
        let a = try dec.decode(BridgeClaim.self, from: d(Fixtures.claimSpec))
        XCTAssertTrue(a.isCapturer); XCTAssertTrue(a.created); XCTAssertEqual(a.heartbeatIntervalSeconds, 15)
        let b = try dec.decode(BridgeClaim.self, from: d(Fixtures.claimUpstream))
        XCTAssertFalse(b.isCapturer); XCTAssertEqual(b.leaseSeconds, 45)
    }

    func testErrorBody() throws {
        let e = APIClient.httpError(409, d(Fixtures.error))
        XCTAssertEqual(e.code, "INVALID_STATE"); XCTAssertEqual(e.status, 409)
        XCTAssertEqual(APIClient.httpError(422, d("bad json")).code, "HTTP_422")
    }

    func testRequestEncodingIsCamelCaseAndOmitsNil() throws {
        let data = try APIDates.encoder().encode(JoinByIdRequest(displayName: nil, speakLanguage: "vi", listenLanguage: "en"))
        let o = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(o["speakLanguage"] as? String, "vi"); XCTAssertNil(o["displayName"])
    }

    func testMeetingLinkParsing() {
        XCTAssertEqual(MeetingLink.parse("https://meet.google.com/abc-defg-hij?authuser=0")?.code, "abc-defg-hij")
        XCTAssertEqual(MeetingLink.parse("abc-defg-hij")?.provider, .googleMeet)
        XCTAssertEqual(MeetingLink.parse("https://us02web.zoom.us/j/12345678901?pwd=x")?.code, "12345678901")
    }
}
