import Foundation

/// chrono serialises `DateTime<Utc>` as RFC 3339 with up to 9 fractional digits ("...:00.123456789Z").
/// ISO8601DateFormatter is picky about fraction length, so we normalise to exactly 3 digits first.
enum APIDates {
    private static func makeFormatter(fractional: Bool) -> ISO8601DateFormatter {
        let f = ISO8601DateFormatter()
        f.formatOptions = fractional ? [.withInternetDateTime, .withFractionalSeconds] : [.withInternetDateTime]
        return f
    }
    // ISO8601DateFormatter is thread-safe for date(from:) in practice (documented since macOS 10.12).
    private static let withFraction = makeFormatter(fractional: true)
    private static let plain = makeFormatter(fractional: false)

    static func parse(_ raw: String) -> Date? {
        var s = raw
        if let dot = s.firstIndex(of: ".") {
            // find end of the digit run
            var end = s.index(after: dot)
            while end < s.endIndex, s[end].isNumber { end = s.index(after: end) }
            let digits = s[s.index(after: dot)..<end]
            var d = String(digits.prefix(3))
            while d.count < 3 { d += "0" }
            s = String(s[..<dot]) + "." + d + String(s[end...])
        }
        return withFraction.date(from: s) ?? plain.date(from: s)
    }

    static func string(_ d: Date) -> String { withFraction.string(from: d) }

    static func decoder() -> JSONDecoder {
        let dec = JSONDecoder()
        dec.dateDecodingStrategy = .custom { decoder in
            let c = try decoder.singleValueContainer()
            let raw = try c.decode(String.self)
            guard let d = parse(raw) else {
                throw DecodingError.dataCorruptedError(in: c, debugDescription: "Bad date: \(raw)")
            }
            return d
        }
        return dec
    }

    static func encoder() -> JSONEncoder {
        let enc = JSONEncoder()
        enc.dateEncodingStrategy = .custom { date, encoder in
            var c = encoder.singleValueContainer()
            try c.encode(string(date))
        }
        return enc
    }
}
