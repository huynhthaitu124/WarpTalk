import Foundation

enum BridgeProviderKind: String, CaseIterable, Identifiable {
    case googleMeet = "google_meet", zoom, teams
    var id: String { rawValue }
    var label: String {
        switch self { case .googleMeet: return "Google Meet"; case .zoom: return "Zoom"; case .teams: return "Microsoft Teams" }
    }
}

/// Normalises what the user pastes (URL or bare code) into `(provider, meetCode)` for `bridge/claim`.
enum MeetingLink {
    static func parse(_ raw: String) -> (provider: BridgeProviderKind?, code: String)? {
        let s = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !s.isEmpty else { return nil }

        // Bare Meet code abc-defg-hij
        if isMeetCode(s.lowercased()) { return (.googleMeet, s.lowercased()) }

        guard let url = URL(string: s), let host = url.host?.lowercased() else { return (nil, s) }
        let parts = url.path.split(separator: "/").map(String.init)
        if host.hasSuffix("meet.google.com") {
            if let c = parts.first?.lowercased(), isMeetCode(c) { return (.googleMeet, c) }
            return (.googleMeet, parts.first?.lowercased() ?? s)   // nickname / lookup links: server decides
        }
        if host.hasSuffix("zoom.us") || host.hasSuffix("zoom.com") {
            // /j/{meetingNumber}  or /wc/join/{number}
            if let n = parts.last(where: { $0.allSatisfy(\.isNumber) && $0.count >= 9 }) { return (.zoom, n) }
            return (.zoom, s)
        }
        if host.hasSuffix("teams.microsoft.com") || host.hasSuffix("teams.live.com") { return (.teams, s) }
        return (nil, s)
    }

    /// abc-defg-hij (3-4 letters, 3 groups)
    static func isMeetCode(_ s: String) -> Bool {
        let p = s.split(separator: "-", omittingEmptySubsequences: false)
        return p.count == 3 && p.allSatisfy { (3...4).contains($0.count) && $0.allSatisfy { $0.isLetter && $0.isASCII } }
    }
}
