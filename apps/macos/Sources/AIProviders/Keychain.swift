import Foundation
import Security

public struct KeychainError: Error, Equatable, Sendable { public let status: Int32 }

/// API keys live only here. Items are device-local and unavailable while locked.
public struct KeychainSecretStore: SecretStore {
    public let service: String
    public init(service: String = "io.github.candid-oss.candid.ai-providers") { self.service = service }

    private func query(_ ref: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: ref]
    }

    public func secret(for ref: String) throws -> String? {
        var q = query(ref); q[kSecReturnData as String] = true; q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &out)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw KeychainError(status: status) }
        return (out as? Data).flatMap { String(data: $0, encoding: .utf8) }
    }

    public func setSecret(_ value: String, for ref: String) throws {
        let data = Data(value.utf8)
        let status = SecItemUpdate(query(ref) as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecSuccess { return }
        guard status == errSecItemNotFound else { throw KeychainError(status: status) }
        var add = query(ref)
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        let added = SecItemAdd(add as CFDictionary, nil)
        guard added == errSecSuccess else { throw KeychainError(status: added) }
    }

    /// No dialog: an item whose access needs the user's approval (for example after the App was rebuilt) fails
    /// with `errSecInteractionNotAllowed` instead of blocking the caller until someone answers a prompt.
    public func secretWithoutPrompt(for ref: String) throws -> String? {
        var q = query(ref); q[kSecReturnData as String] = true; q[kSecMatchLimit as String] = kSecMatchLimitOne
        q[kSecUseAuthenticationUI as String] = kSecUseAuthenticationUIFail
        var out: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &out)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw KeychainError(status: status) }
        return (out as? Data).flatMap { String(data: $0, encoding: .utf8) }
    }

    public func hasSecret(for ref: String) throws -> Bool {
        var q = query(ref); q[kSecMatchLimit as String] = kSecMatchLimitOne; q[kSecReturnAttributes as String] = true
        let status = SecItemCopyMatching(q as CFDictionary, nil)
        if status == errSecItemNotFound { return false }
        guard status == errSecSuccess else { throw KeychainError(status: status) }
        return true
    }

    public func deleteSecret(for ref: String) throws {
        let status = SecItemDelete(query(ref) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw KeychainError(status: status) }
    }
}
