import Foundation

enum Language: String, CaseIterable, Identifiable, Codable {
    case vi, en, ja
    var id: String { rawValue }
    var label: String {
        switch self {
        case .vi: return "Tiếng Việt"
        case .en: return "English"
        case .ja: return "日本語"
        }
    }
}
