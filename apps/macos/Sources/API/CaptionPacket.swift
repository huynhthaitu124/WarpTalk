import Foundation

/// One LiveKit data packet on topic `captions`, as published by the pipeline
/// (apps/pipeline/src/downstream.rs: `publish_stt_caption` => `type: "stt"`, `translation_caption` => `type: "translation"`).
/// Mirrors apps/web/src/lib/captions.ts `parseCaption`; pure Foundation so it is unit-tested without LiveKit.
///
/// Current wire shape (both types):
///   `segment_id`, `speakerId`/`speaker_id`, `speakerName` (nullable), `speakerUserId`, `text`, `lang`,
///   `is_early` (always false now), `is_flash` (the sentence was produced in flash mode), `translations` { lang: text }.
/// `type: "translation"` adds `sentenceIndex`, `isLastSentence`, `sentenceText`, `sentenceTranslations`, `wire_segment_id`.
/// `translations` is CUMULATIVE: it holds the whole segment so far for that language, so a client replaces, never appends.
struct CaptionPacket: Equatable {
    var type: String
    var segmentId: String?
    var speakerId: String?
    var speakerName: String?
    var speakerUserId: String?
    var text: String
    var lang: String?
    var sentenceIndex: Int?
    var isLastSentence: Bool?
    var isFlash: Bool
    var isEarly: Bool
    /// Cumulative translation of the segment so far, by target language.
    var translations: [String: String]
    /// This sentence alone (translation packets only).
    var sentenceText: String?
    var sentenceTranslations: [String: String]
    var emittedAtMs: Int64?

    /// Text is final unless the pipeline says `is_early` (explicit `final`/`isFinal` wins, as in the web parser).
    var isFinal: Bool { !isEarly }

    /// `nil` for empty payloads, non-JSON text, or packets with neither text nor translations.
    static func parse(_ data: Data) -> CaptionPacket? {
        guard !data.isEmpty,
              let obj = try? JSONSerialization.jsonObject(with: data),
              let j = obj as? [String: Any] else { return nil }

        func str(_ keys: String...) -> String? {
            for k in keys { if let v = j[k] as? String, !v.isEmpty { return v } }
            return nil
        }
        func map(_ key: String) -> [String: String] {
            guard let m = j[key] as? [String: Any] else { return [:] }
            return m.compactMapValues { ($0 as? String).flatMap { $0.isEmpty ? nil : $0 } }
        }
        func int(_ keys: String...) -> Int? {
            for k in keys {
                if let n = j[k] as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() {
                    let v = n.intValue
                    if v >= 0 { return v }
                }
            }
            return nil
        }
        func bool(_ keys: String...) -> Bool? {
            for k in keys { if let n = j[k] as? NSNumber, CFGetTypeID(n) == CFBooleanGetTypeID() { return n.boolValue } }
            return nil
        }

        let translations = map("translations")
        let text = str("text", "caption") ?? ""
        if text.isEmpty && translations.isEmpty { return nil }

        let emitted = (j["emittedAtMs"] as? NSNumber).map { $0.int64Value }
        let explicitFinal = bool("final", "isFinal")
        let early = explicitFinal.map { !$0 } ?? (bool("is_early") ?? false)

        return CaptionPacket(
            type: str("type") ?? "stt",
            segmentId: str("segment_id", "segmentId", "id"),
            speakerId: str("speakerId", "speaker_id"),
            speakerName: str("speakerName", "speaker_name", "speaker", "participantName")?.trimmingCharacters(in: .whitespacesAndNewlines),
            speakerUserId: str("speakerUserId"),
            text: text,
            lang: str("lang", "language"),
            sentenceIndex: int("sentenceIndex", "sentence_index", "chunk_idx"),
            isLastSentence: bool("isLastSentence", "is_last_sentence", "is_final_chunk"),
            isFlash: bool("is_flash") ?? false,
            isEarly: early,
            translations: translations,
            sentenceText: str("sentenceText"),
            sentenceTranslations: map("sentenceTranslations"),
            emittedAtMs: emitted)
    }

    private static let uuidPrefix = try? NSRegularExpression(pattern: "^[0-9a-f]{8}-[0-9a-f]{4}-", options: [.caseInsensitive])

    /// Same rule as the web: a real `speakerName` first; a raw id only when it is not a UUID; never `pipeline-agent-*`.
    var displaySpeaker: String {
        if let n = speakerName, !n.isEmpty, !n.hasPrefix("pipeline-agent") { return n }
        if let id = speakerId, !id.isEmpty, !id.hasPrefix("pipeline-agent") {
            let range = NSRange(id.startIndex..., in: id)
            if Self.uuidPrefix?.firstMatch(in: id, options: [], range: range) == nil { return id }
        }
        return ""
    }

    /// The translation to show for `listenLanguage` (cumulative text); without a language, the first available.
    func translation(for listenLanguage: String?) -> String? {
        if let l = listenLanguage, let t = translations[l] { return t }
        if listenLanguage == nil, let k = translations.keys.sorted().first { return translations[k] }
        return nil
    }

    /// Update for the overlay (`OverlayModel.push`). The segment id is shared by the `stt` message and every translation
    /// message of the same segment, so they upsert one line: the original first, then the growing translation.
    func overlayUpdate(listenLanguage: String?) -> OverlayUpdate? {
        let tr = translation(for: listenLanguage)
        if text.isEmpty && tr == nil { return nil }
        return OverlayUpdate(speaker: displaySpeaker, text: text, translation: tr, segmentId: segmentId, isFinal: isFinal)
    }
}

struct OverlayUpdate: Equatable {
    var speaker: String
    var text: String
    var translation: String?
    var segmentId: String?
    var isFinal: Bool
}
