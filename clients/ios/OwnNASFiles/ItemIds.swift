import FileProvider

/// Stable identifiers for the server's files and folders. Files needs an item to keep its
/// identifier when it is renamed or moved, and the server only knows paths, so each path gets an
/// id here, kept in a file, and renames and moves update the paths under the same ids.
final class ItemIds: @unchecked Sendable {
    private let lock = NSLock()
    private let file: URL
    private var byPath: [String: String] = [:]
    private var byId: [String: String] = [:]

    init(domain: String) {
        let folder = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        file = folder.appendingPathComponent("ids-\(domain).plist")
        if let data = try? Data(contentsOf: file), let saved = try? PropertyListDecoder().decode([String: String].self, from: data) {
            byPath = saved
            for (path, id) in saved { byId[id] = path }
        }
    }

    /// The identifier for a path ("" is the top), made the first time it is seen.
    func id(_ path: String) -> NSFileProviderItemIdentifier {
        if path.isEmpty { return .rootContainer }
        lock.lock()
        defer { lock.unlock() }
        if let known = byPath[path] { return NSFileProviderItemIdentifier(known) }
        let made = UUID().uuidString
        byPath[path] = made
        byId[made] = path
        saveLocked()
        return NSFileProviderItemIdentifier(made)
    }

    /// The identifier a path already has, without making one (for changes to items never seen).
    func known(_ path: String) -> NSFileProviderItemIdentifier? {
        if path.isEmpty { return .rootContainer }
        lock.lock()
        defer { lock.unlock() }
        return byPath[path].map { NSFileProviderItemIdentifier($0) }
    }

    /// Where an item is on the server, or nil for an unknown item or a place the server lacks.
    func path(_ identifier: NSFileProviderItemIdentifier) -> String? {
        switch identifier {
        case .rootContainer: return ""
        case .trashContainer, .workingSet: return nil
        default:
            lock.lock()
            defer { lock.unlock() }
            return byId[identifier.rawValue]
        }
    }

    /// A rename or move: the item, and everything inside a folder, keep their ids.
    func moved(from old: String, to new: String) {
        guard old != new else { return }
        lock.lock()
        defer { lock.unlock() }
        for (path, id) in byPath where path == old || path.hasPrefix(old + "/") {
            let next = new + path.dropFirst(old.count)
            byPath[path] = nil
            byPath[next] = id
            byId[id] = next
        }
        saveLocked()
    }

    /// Forgets a deleted item and what was inside it.
    func removed(_ old: String) {
        lock.lock()
        defer { lock.unlock() }
        for (path, id) in byPath where path == old || path.hasPrefix(old + "/") {
            byPath[path] = nil
            byId[id] = nil
        }
        saveLocked()
    }

    /// After listing a folder: forgets what is no longer in it (changed outside Files).
    func listed(_ folder: String, names: Set<String>) {
        let prefix = folder.isEmpty ? "" : folder + "/"
        lock.lock()
        let gone = byPath.keys.filter { path in
            guard path.hasPrefix(prefix) else { return false }
            let rest = path.dropFirst(prefix.count)
            return !rest.contains("/") && !names.contains(String(rest))
        }
        lock.unlock()
        for path in gone { removed(path) }
    }

    private func saveLocked() {
        if let data = try? PropertyListEncoder().encode(byPath) { try? data.write(to: file, options: .atomic) }
    }
}
