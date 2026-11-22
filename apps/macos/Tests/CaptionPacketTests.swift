import XCTest
@testable import WarpTalk

/// Fixtures copy the JSON of apps/pipeline/src/downstream.rs (`publish_stt_caption`, `translation_caption`).
final class CaptionPacketTests: XCTestCase {
    private let seg = "0192f5a0-1111-7000-8000-00000000abcd"

    private func stt(name: String? = "Lan", speakerId: String = "0192f5a0-2222-7000-8000-000000000001") -> Data {
        let n = name.map { "\"\($0)\"" } ?? "null"
        return Data("""
        {"type":"stt","segment_id":"\(seg)","meeting_id":"r1","speaker_id":"\(speakerId)","speakerId":"\(speakerId)",
         "speakerName":\(n),"speakerUserId":"u-1","seq":4,"utterance_seq":2,"text":"xin chào mọi người","lang":"vi",
         "is_early":false,"is_flash":true,"translations":{},"ts_ms":1,"emittedAtMs":1795255200123}
        """.utf8)
    }

    private func translation(index: Int, last: Bool, text: String, cumulative: String, sentence: String) -> Data {
        Data("""
        {"type":"translation","segment_id":"\(seg)","wire_segment_id":"\(seg)-en-c\(index)","chunk_idx":\(index),"is_final_chunk":\(last),
         "sentenceIndex":\(index),"isLastSentence":\(last),"meeting_id":"r1","speaker_id":"sp","speakerId":"sp","speakerName":"Lan",
         "speakerUserId":"u-1","seq":5,"text":"\(text)","sentenceText":"s","lang":"vi","is_early":false,"is_flash":false,
         "translations":{"en":"\(cumulative)"},"sentenceTranslations":{"en":"\(sentence)"},"emittedAtMs":1795255200456}
        """.utf8)
    }

    func testSttPacketNewFields() throws {
        let p = try XCTUnwrap(CaptionPacket.parse(stt()))
        XCTAssertEqual(p.type, "stt"); XCTAssertEqual(p.segmentId, seg)
        XCTAssertEqual(p.speakerName, "Lan"); XCTAssertEqual(p.speakerUserId, "u-1")
        XCTAssertEqual(p.text, "xin chào mọi người"); XCTAssertEqual(p.lang, "vi")
        XCTAssertTrue(p.isFlash); XCTAssertFalse(p.isEarly); XCTAssertTrue(p.isFinal)
        XCTAssertTrue(p.translations.isEmpty); XCTAssertNil(p.sentenceIndex)
        XCTAssertEqual(p.emittedAtMs, 1_795_255_200_123)
    }

    func testTranslationPacketIsCumulativeWithSentenceMarkers() throws {
        let p = try XCTUnwrap(CaptionPacket.parse(translation(index: 1, last: true, text: "câu một câu hai", cumulative: "one two", sentence: "two")))
        XCTAssertEqual(p.type, "translation")
        XCTAssertEqual(p.sentenceIndex, 1); XCTAssertEqual(p.isLastSentence, true)
        XCTAssertEqual(p.translations, ["en": "one two"])            // whole segment so far
        XCTAssertEqual(p.sentenceTranslations, ["en": "two"])        // this sentence alone
        XCTAssertFalse(p.isFlash)
    }

    func testNullSpeakerNameFallsBackToNonUuidId() throws {
        let uuid = try XCTUnwrap(CaptionPacket.parse(stt(name: nil)))
        XCTAssertEqual(uuid.displaySpeaker, "", "a UUID is not a display name")
        let named = try XCTUnwrap(CaptionPacket.parse(stt(name: nil, speakerId: "host-1")))
        XCTAssertEqual(named.displaySpeaker, "host-1")
        XCTAssertEqual(try XCTUnwrap(CaptionPacket.parse(stt())).displaySpeaker, "Lan")
    }

    func testPipelineAgentIsNeverASpeaker() throws {
        let p = try XCTUnwrap(CaptionPacket.parse(stt(name: "pipeline-agent-7", speakerId: "pipeline-agent-7")))
        XCTAssertEqual(p.displaySpeaker, "")
    }

    func testFlashFlagDoesNotMakeALineInterim() throws {
        // is_flash = produced by flash mode; is_early stays false => final text (audit R-03)
        XCTAssertTrue(try XCTUnwrap(CaptionPacket.parse(stt())).isFinal)
        let early = Data(#"{"type":"stt","segment_id":"s","text":"x","is_early":true,"is_flash":true}"#.utf8)
        XCTAssertFalse(try XCTUnwrap(CaptionPacket.parse(early)).isFinal)
    }

    func testInvalidOrEmptyPayloads() {
        XCTAssertNil(CaptionPacket.parse(Data()))
        XCTAssertNil(CaptionPacket.parse(Data("plain text".utf8)))
        XCTAssertNil(CaptionPacket.parse(Data(#"{"type":"stt","text":"","translations":{}}"#.utf8)))
        XCTAssertNil(CaptionPacket.parse(Data("[1,2]".utf8)))
    }

    func testLegacySnakeCaseShape() throws {
        let p = try XCTUnwrap(CaptionPacket.parse(Data(#"{"segment_id":"s","speaker_id":"a","text":"t","chunk_idx":2,"is_final_chunk":true,"translations":{"en":"T"}}"#.utf8)))
        XCTAssertEqual(p.sentenceIndex, 2); XCTAssertEqual(p.isLastSentence, true); XCTAssertEqual(p.speakerId, "a")
    }

    func testOverlayUpdatesUpsertOneLineAndGrowTranslation() throws {
        let model = OverlayModelProbe()
        for data in [stt(),
                     translation(index: 0, last: false, text: "câu một", cumulative: "one", sentence: "one"),
                     translation(index: 1, last: true, text: "câu một câu hai", cumulative: "one two", sentence: "two")] {
            let u = try XCTUnwrap(try XCTUnwrap(CaptionPacket.parse(data)).overlayUpdate(listenLanguage: "en"))
            model.apply(u)
        }
        XCTAssertEqual(model.lines.count, 1)
        XCTAssertEqual(model.lines[0].translation, "one two", "cumulative translation replaces, never appends")
        XCTAssertEqual(model.lines[0].text, "câu một câu hai")
        XCTAssertEqual(model.lines[0].speaker, "Lan")
    }

    func testTranslationLanguageSelection() throws {
        let p = try XCTUnwrap(CaptionPacket.parse(Data(#"{"type":"translation","segment_id":"s","text":"t","translations":{"en":"E","fr":"F"}}"#.utf8)))
        XCTAssertEqual(p.translation(for: "fr"), "F")
        XCTAssertNil(p.translation(for: "de"))
        XCTAssertEqual(p.translation(for: nil), "E")
        XCTAssertEqual(p.overlayUpdate(listenLanguage: "fr")?.translation, "F")
    }

    /// Mirrors `OverlayModel.push` semantics without needing the main actor.
    private final class OverlayModelProbe {
        struct Line { var id: String; var speaker: String; var text: String; var translation: String? }
        var lines: [Line] = []
        func apply(_ u: OverlayUpdate) {
            if let sid = u.segmentId, let i = lines.lastIndex(where: { $0.id == sid }) {
                lines[i] = Line(id: sid, speaker: u.speaker, text: u.text, translation: u.translation ?? lines[i].translation)
            } else {
                lines.append(Line(id: u.segmentId ?? "x", speaker: u.speaker, text: u.text, translation: u.translation))
            }
        }
    }
}
