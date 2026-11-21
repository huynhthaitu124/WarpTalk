import Foundation
import Security

/// Everything we persist about the session (one Keychain item, JSON blob).
struct StoredSession: Codable, Equatable {
    var userId: String
    var email: String
    var accessToken: String
    var accessTokenExpiresAt: Date
    var refreshToken: String
}

protocol TokenStore: AnyObject {
    func load() -> StoredSession?
    func save(_ s: StoredSession)
    func clear()
}

/// Generic-password Keychain item. Legacy (file-based) keychain: works for unsigned/ad-hoc dev builds
/// without keychain-access-groups entitlements.
final class KeychainTokenStore: TokenStore {
    private let service: String
    private let account = "session"

    init(service: String = "com.example.warptalk.session") { self.service = service }

    private var baseQuery: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service,
         kSecAttrAccount as String: account]
    }

    func load() -> StoredSession? {
        var q = baseQuery
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return nil }
        let dec = JSONDecoder()
        dec.dateDecodingStrategy = .secondsSince1970
        return try? dec.decode(StoredSession.self, from: data)
    }

    func save(_ s: StoredSession) {
        let enc = JSONEncoder()
        enc.dateEncodingStrategy = .secondsSince1970
        guard let data = try? enc.encode(s) else { return }
        let status = SecItemUpdate(baseQuery as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var add = baseQuery
            add[kSecValueData as String] = data
            add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
            SecItemAdd(add as CFDictionary, nil)
        }
    }

    func clear() { SecItemDelete(baseQuery as CFDictionary) }
}

/// In-memory store for tests.
final class MemoryTokenStore: TokenStore {
    var session: StoredSession?
    init(_ s: StoredSession? = nil) { session = s }
    func load() -> StoredSession? { session }
    func save(_ s: StoredSession) { session = s }
    func clear() { session = nil }
}
