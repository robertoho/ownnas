import Foundation
import Security

/// A server signed in from this device. The token lives in the Keychain, not here.
struct SavedServer: Codable, Identifiable, Hashable {
    var id: String
    var address: String
    var username: String
    /// The name of the server's shared folder, shown as its title.
    var name: String
    /// A private CA to trust (the `ca.crt` from `ownnas tls generate`), PEM.
    var caPem: String?
    /// Certificate fingerprints the user accepted.
    var pinned: [String]

    var host: String {
        address.replacingOccurrences(of: "https://", with: "").replacingOccurrences(of: "http://", with: "")
    }
}

/// The saved servers (in UserDefaults) and their tokens (in the Keychain, this device only).
/// Both are shared with the Files app extension: the list through the App Group, the tokens
/// through the first Keychain group in the entitlements, where items go by default.
final class ServerStore {
    /// The App Group shared with the extension (`OwnNASAppGroup` in Info.plist).
    static let appGroup = Bundle.main.object(forInfoDictionaryKey: "OwnNASAppGroup") as? String
    private let defaults = ServerStore.appGroup.flatMap { UserDefaults(suiteName: $0) } ?? .standard
    private let listKey = "servers"
    /// Tokens the Keychain refused (an unsigned build): kept for this run only.
    private var unsaved: [String: String] = [:]

    func load() -> [SavedServer] {
        guard let data = defaults.data(forKey: listKey) else { return [] }
        return (try? JSONDecoder().decode([SavedServer].self, from: data)) ?? []
    }

    func save(_ servers: [SavedServer]) {
        defaults.set(try? JSONEncoder().encode(servers), forKey: listKey)
    }

    /// Moves what the app saved before it shared with the extension: the server list into the
    /// App Group, and the tokens into the shared Keychain group. Once.
    func shareWithExtension() {
        guard defaults != .standard, !defaults.bool(forKey: "shared.v1") else { return }
        let old = UserDefaults.standard
        for key in old.dictionaryRepresentation().keys where key == listKey || key.hasPrefix("gallery.workspace.") {
            if defaults.object(forKey: key) == nil { defaults.set(old.object(forKey: key), forKey: key) }
        }
        for server in load() {
            // Saved again without a group, it lands in the default (shared) one.
            if let token = token(server.id) { setToken(server.id, token) }
        }
        defaults.set(true, forKey: "shared.v1")
    }

    func token(_ id: String) -> String? {
        if let token = unsaved[id] { return token }
        var query = keychainQuery(id)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var found: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &found) == errSecSuccess, let data = found as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    func setToken(_ id: String, _ token: String?) {
        SecItemDelete(keychainQuery(id) as CFDictionary)
        unsaved[id] = nil
        guard let token else { return }
        var item = keychainQuery(id)
        item[kSecValueData as String] = Data(token.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        if SecItemAdd(item as CFDictionary, nil) != errSecSuccess { unsaved[id] = token }
    }

    private func keychainQuery(_ id: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "app.ownnas.token", kSecAttrAccount as String: id]
    }

    /// The Gallery workspace last shown for a server (`nil`: every photo).
    func galleryWorkspace(_ id: String) -> Int64? {
        (defaults.object(forKey: "gallery.workspace.\(id)") as? NSNumber)?.int64Value
    }

    func setGalleryWorkspace(_ id: String, _ workspace: Int64?) {
        if let workspace { defaults.set(NSNumber(value: workspace), forKey: "gallery.workspace.\(id)") } else { defaults.removeObject(forKey: "gallery.workspace.\(id)") }
    }
}
