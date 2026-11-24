import XCTest
@testable import WarpTalk

@MainActor
final class OverlayCopilotModelTests: XCTestCase {
    private func seg(_ id: String, _ ch: CopilotChannel, _ text: String, final: Bool = true, echo: Bool = false) -> CopilotEngineEvent {
        .segment(CopilotSegment(clientSegmentId: id, channel: ch, text: text, language: "vi", startMs: 0, endMs: 1, confidence: nil,
                                isFinal: final, likelyEcho: echo, utteranceSeq: 1))
    }

    func testSpeakerLabelsDefaultAndCustom() {
        let m = OverlayModel()
        m.enterCopilotMode()
        m.applyCopilot(seg("a", .me, "xin chào")); m.applyCopilot(seg("b", .others, "chào bạn"))
        XCTAssertEqual(m.copilotLines.map(\.speaker), ["Tôi", "Người khác"])
        m.enterCopilotMode(meName: "Lan", othersName: "Minh")
        m.applyCopilot(seg("c", .me, "x")); m.applyCopilot(seg("d", .others, "y"))
        XCTAssertEqual(m.copilotLines.map(\.speaker), ["Lan", "Minh"])
    }

    func testArrivalOrderAndRecentThree() {
        let m = OverlayModel(); m.enterCopilotMode()
        for (i, t) in ["1", "2", "3", "4"].enumerated() { m.applyCopilot(seg("s\(i)", i % 2 == 0 ? .me : .others, t)) }
        XCTAssertEqual(m.recentCopilot(3).map(\.text), ["2", "3", "4"])
    }

    func testPartialUpdatesInPlaceAndIsReplacedBySegment() {
        let m = OverlayModel(); m.enterCopilotMode()
        m.applyCopilot(.partial(channel: .others, text: "chúng ta"))
        m.applyCopilot(seg("a", .me, "tôi nói"))
        m.applyCopilot(.partial(channel: .others, text: "chúng ta cần"))     // updates in its own slot (first)
        XCTAssertEqual(m.copilotLines.map(\.id), ["partial-others", "a"])
        XCTAssertEqual(m.copilotLines.first?.text, "chúng ta cần")
        m.applyCopilot(seg("b", .others, "chúng ta cần chốt lịch"))
        XCTAssertEqual(m.copilotLines.map(\.id), ["a", "b"])                  // partial gone, segment appended last
        XCTAssertTrue(m.copilotLines.allSatisfy(\.isFinal))
    }

    func testEmptyPartialRemovesLine() {
        let m = OverlayModel(); m.enterCopilotMode()
        m.applyCopilot(.partial(channel: .me, text: "a")); m.applyCopilot(.partial(channel: .me, text: "  "))
        XCTAssertTrue(m.copilotLines.isEmpty)
    }

    func testSegmentUpsertNeverDowngrades() {
        let m = OverlayModel(); m.enterCopilotMode()
        m.applyCopilot(seg("a", .me, "một", final: false))
        m.applyCopilot(seg("a", .me, "một hai", final: true))
        m.applyCopilot(seg("a", .me, "một", final: false))
        XCTAssertEqual(m.copilotLines.count, 1)
        XCTAssertEqual(m.copilotLines[0].text, "một hai"); XCTAssertTrue(m.copilotLines[0].isFinal)
    }

    func testEchoAndEmptySegmentsAreHidden() {
        let m = OverlayModel(); m.enterCopilotMode()
        m.applyCopilot(seg("a", .me, "vọng", echo: true)); m.applyCopilot(seg("b", .me, "  "))
        XCTAssertTrue(m.copilotLines.isEmpty)
    }

    func testSuggestionsOrderUpsertRemoveAndCap() {
        let m = OverlayModel(); m.enterCopilotMode()
        func s(_ id: String, _ c: String = "x") -> OverlaySuggestion { OverlaySuggestion(id: id, category: "q", content: c, followUp: nil, isFollowUp: false) }
        m.upsertSuggestion(s("1")); m.upsertSuggestion(s("2")); m.upsertSuggestion(s("3")); m.upsertSuggestion(s("4"))
        XCTAssertEqual(m.visibleSuggestions().map(\.id), ["2", "3", "4"])            // newest 3
        m.upsertSuggestion(s("2", "đổi"))                                            // in place, keeps position
        XCTAssertEqual(m.suggestions.map(\.id), ["1", "2", "3", "4"]); XCTAssertEqual(m.suggestions[1].content, "đổi")
        m.removeSuggestion(id: "3"); m.removeSuggestion(id: "nope")
        XCTAssertEqual(m.suggestions.map(\.id), ["1", "2", "4"])
        for i in 10..<20 { m.upsertSuggestion(s("\(i)")) }
        XCTAssertEqual(m.suggestions.count, OverlayModel.maxSuggestions)
        XCTAssertEqual(m.suggestions.last?.id, "19")
    }

    func testModeSwitchClears() {
        let m = OverlayModel()
        XCTAssertEqual(m.mode, .captions)
        m.enterCopilotMode(); m.applyCopilot(seg("a", .me, "x"))
        m.upsertSuggestion(OverlaySuggestion(id: "1", category: "q", content: "c", followUp: nil, isFollowUp: false))
        m.exitCopilotMode()
        XCTAssertEqual(m.mode, .captions); XCTAssertTrue(m.copilotLines.isEmpty); XCTAssertTrue(m.suggestions.isEmpty)
    }

    func testCaptionModeUnaffected() {
        let m = OverlayModel()
        m.push(speaker: "A", text: "hi", translation: nil, segmentId: "s", isFinal: true)
        XCTAssertEqual(m.lines.count, 1)
    }
}
