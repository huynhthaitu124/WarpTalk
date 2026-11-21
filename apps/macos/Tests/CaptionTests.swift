import XCTest
@testable import WarpTalk

final class CaptionTests: XCTestCase {
    func testParse() throws {
        let c = try XCTUnwrap(Caption.parse(Data(Fixtures.caption.utf8)))
        XCTAssertEqual(c.segmentId, "s1"); XCTAssertTrue(c.isFinal)
        XCTAssertEqual(c.translations["en"], "Hello"); XCTAssertEqual(c.speakerName, "An")
    }

    func testNumericIdsAndMissingTranslations() throws {
        let j = #"{"segmentId":7,"speakerId":3,"lang":"en","text":"hi","isFinal":false}"#
        let c = try XCTUnwrap(Caption.parse(Data(j.utf8)))
        XCTAssertEqual(c.segmentId, "7"); XCTAssertEqual(c.speakerId, "3"); XCTAssertTrue(c.translations.isEmpty)
    }

    func testGarbageIsNil() { XCTAssertNil(Caption.parse(Data("nope".utf8))) }

    func testLatencyTrackerInterimToFinal() {
        var t = LatencyTracker()
        var clock = Date(timeIntervalSince1970: 1000)
        t.now = { clock }
        t.observe(Caption(segmentId: "a", speakerId: "u", lang: "vi", text: "xin", isFinal: false), listenLanguage: "en")
        XCTAssertNil(t.lastMs)
        clock = clock.addingTimeInterval(1.5)
        t.observe(Caption(segmentId: "a", speakerId: "u", lang: "vi", text: "xin chao", isFinal: true, translations: ["en": "hello"]), listenLanguage: "en")
        XCTAssertEqual(try XCTUnwrap(t.lastMs), 1500, accuracy: 1)
        XCTAssertEqual(t.badge, "1.5 s")
    }

    @MainActor func testStoreUpsertDoesNotDowngradeFinal() {
        let s = CaptionStore(); s.listenLanguage = "en"
        s.ingest(Caption(segmentId: "a", speakerId: "u", lang: "vi", text: "xin chao", isFinal: true, translations: ["en": "hello"]))
        s.ingest(Caption(segmentId: "a", speakerId: "u", lang: "vi", text: "xin", isFinal: false))
        XCTAssertEqual(s.lines.count, 1); XCTAssertTrue(s.lines[0].isFinal); XCTAssertEqual(s.lines[0].translation, "hello")
    }
}
