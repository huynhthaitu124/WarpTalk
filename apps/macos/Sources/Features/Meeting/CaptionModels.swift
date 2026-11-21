import Foundation

/// Caption packet published by the pipeline on the LiveKit data channel, topic "captions":
/// `{segmentId, speakerId, speakerName?, lang, text, isFinal, translations:{lang:text}}`.
/// Extra optional timing fields are accepted if the pipeline adds them (see `emittedAt`).
struct Caption: Decodable, Equatable {
    static let topic = "captions"

    let segmentId: String
    let speakerId: String
    let speakerName: String?
    let lang: String
    let text: String
    let isFinal: Bool
    let translations: [String: String]
    /// Optional server emit time (epoch ms or s; tolerated keys: emittedAtMs, emittedAt, ts, timestamp).
    let emittedAt: Date?

    private enum K: String, CodingKey {
        case segmentId, speakerId, speakerName, lang, text, isFinal, translations
        case emittedAtMs, emittedAt, ts, timestamp
    }

    init(segmentId: String, speakerId: String, speakerName: String? = nil, lang: String, text: String,
         isFinal: Bool, translations: [String: String] = [:], emittedAt: Date? = nil) {
        self.segmentId = segmentId; self.speakerId = speakerId; self.speakerName = speakerName
        self.lang = lang; self.text = text; self.isFinal = isFinal
        self.translations = translations; self.emittedAt = emittedAt
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        guard let sid = try Self.flexString(c, .segmentId) else {
            throw DecodingError.keyNotFound(K.segmentId, .init(codingPath: c.codingPath, debugDescription: "segmentId missing"))
        }
        segmentId = sid
        speakerId = (try Self.flexString(c, .speakerId)) ?? ""
        speakerName = try c.decodeIfPresent(String.self, forKey: .speakerName)
        lang = try c.decode(String.self, forKey: .lang)
        text = try c.decode(String.self, forKey: .text)
        isFinal = (try c.decodeIfPresent(Bool.self, forKey: .isFinal)) ?? false
        translations = (try c.decodeIfPresent([String: String].self, forKey: .translations)) ?? [:]
        var stamp: Date?
        for key in [K.emittedAtMs, .emittedAt, .ts, .timestamp] {
            if let v = try? c.decodeIfPresent(Double.self, forKey: key) {
                // > 1e12 => milliseconds, otherwise seconds
                stamp = Date(timeIntervalSince1970: v > 1e12 ? v / 1000 : v)
                break
            }
        }
        emittedAt = stamp
    }

    private static func flexString(_ c: KeyedDecodingContainer<K>, _ k: K) throws -> String? {
        if let s = try? c.decodeIfPresent(String.self, forKey: k) { return s }
        if let i = try? c.decodeIfPresent(Int.self, forKey: k) { return String(i) }
        return nil
    }

    static func parse(_ data: Data) -> Caption? { try? JSONDecoder().decode(Caption.self, from: data) }
}

struct CaptionLine: Identifiable, Equatable {
    let id: String               // segmentId
    var speaker: String
    var lang: String
    var original: String
    var translation: String?     // in the viewer's listen language (nil when lang == listen or not ready)
    var isFinal: Bool
    var updatedAt: Date
}

/// Measures how long a segment takes from first sight to "final + translation in my language".
/// This is receipt-based (no clock sync needed). If the packet carries `emittedAt`, we also report
/// the one-way age at arrival (needs roughly synced clocks, so shown only as a secondary figure).
struct LatencyTracker {
    private(set) var firstSeen: [String: Date] = [:]
    private(set) var samples: [Double] = []   // ms, last 10
    private(set) var lastServerAgeMs: Double?
    var now: () -> Date = Date.init

    var lastMs: Double? { samples.last }
    var averageMs: Double? { samples.isEmpty ? nil : samples.reduce(0, +) / Double(samples.count) }

    mutating func observe(_ c: Caption, listenLanguage: String) {
        let t = now()
        if firstSeen[c.segmentId] == nil {
            firstSeen[c.segmentId] = t
            if firstSeen.count > 500 { firstSeen = firstSeen.filter { t.timeIntervalSince($0.value) < 120 } }
        }
        if let e = c.emittedAt { lastServerAgeMs = t.timeIntervalSince(e) * 1000 }
        let hasMyText = c.lang == listenLanguage || c.translations[listenLanguage] != nil
        if c.isFinal, hasMyText, let first = firstSeen[c.segmentId] {
            samples.append(t.timeIntervalSince(first) * 1000)
            if samples.count > 10 { samples.removeFirst() }
            firstSeen[c.segmentId] = nil
        }
    }

    /// e.g. "1.4 s" (avg of last 10: "1.6 s")
    var badge: String? {
        guard let last = lastMs else { return nil }
        let l = String(format: "%.1f s", last / 1000)
        if let avg = averageMs, samples.count > 1 { return "\(l) · tb \(String(format: "%.1f s", avg / 1000))" }
        return l
    }
}

@MainActor
final class CaptionStore: ObservableObject {
    @Published private(set) var lines: [CaptionLine] = []
    @Published private(set) var latencyBadge: String?
    var listenLanguage: String = "en"
    private var tracker = LatencyTracker()
    private let maxLines = 300

    func ingest(data: Data) {
        guard let c = Caption.parse(data) else { return }
        ingest(c)
    }

    func ingest(_ c: Caption) {
        tracker.observe(c, listenLanguage: listenLanguage)
        latencyBadge = tracker.badge
        let translation: String? = c.lang == listenLanguage ? nil : c.translations[listenLanguage]
        let line = CaptionLine(id: c.segmentId,
                               speaker: (c.speakerName?.isEmpty == false ? c.speakerName! : c.speakerId),
                               lang: c.lang, original: c.text, translation: translation,
                               isFinal: c.isFinal, updatedAt: Date())
        if let i = lines.lastIndex(where: { $0.id == c.segmentId }) {
            // Never downgrade a final line with a late interim.
            if lines[i].isFinal && !c.isFinal { return }
            var merged = line
            if merged.translation == nil { merged.translation = lines[i].translation }
            lines[i] = merged
        } else {
            lines.append(line)
            if lines.count > maxLines { lines.removeFirst(lines.count - maxLines) }
        }
    }

    func recent(_ n: Int) -> [CaptionLine] { Array(lines.suffix(n)) }
    func clear() { lines = []; latencyBadge = nil; tracker = LatencyTracker() }
}
