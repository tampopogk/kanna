import Foundation
import Security

enum Keychain {
    private static let service = "kanna.device-key"

    private static func query(_ account: String = "phone") -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: account]
    }

    static func read(_ account: String = "phone") -> String? {
        var query = query(account)
        query[kSecReturnData as String] = true
        var found: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &found) == errSecSuccess, let data = found as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func make() -> String? {
        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else { return nil }
        let secret = bytes.map { String(format: "%02x", $0) }.joined()
        var item = query()
        item[kSecValueData as String] = Data(secret.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly
        SecItemDelete(query() as CFDictionary)
        return SecItemAdd(item as CFDictionary, nil) == errSecSuccess ? secret : nil
    }

    static func delete() {
        SecItemDelete(query() as CFDictionary)
        SecItemDelete(query("relay") as CFDictionary)
    }

    static func keepRelayToken(_ token: String?) {
        SecItemDelete(query("relay") as CFDictionary)
        guard let token else { return }
        var item = query("relay")
        item[kSecValueData as String] = Data(token.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly
        SecItemAdd(item as CFDictionary, nil)
    }
}
