import Foundation

enum APIError: LocalizedError {
    case invalidURL(String)
    case transport(Error)
    case http(status: Int, code: String, message: String)
    case decoding(Error, body: String)
    case notAuthenticated

    var errorDescription: String? {
        switch self {
        case .invalidURL(let s): return "URL không hợp lệ: \(s)"
        case .transport(let e): return "Lỗi mạng: \(e.localizedDescription)"
        case .http(let status, let code, let message): return "\(message) (\(status) \(code))"
        case .decoding(let e, _): return "Không đọc được phản hồi từ server: \(e.localizedDescription)"
        case .notAuthenticated: return "Phiên đăng nhập đã hết hạn, hãy đăng nhập lại."
        }
    }

    var code: String? { if case .http(_, let c, _) = self { return c } else { return nil } }
    var status: Int? { if case .http(let s, _, _) = self { return s } else { return nil } }
}
