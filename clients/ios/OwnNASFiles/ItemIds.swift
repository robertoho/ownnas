import FileProvider

/// Stable identifiers for the server's files and folders. Files needs an item to keep its
/// identifier when it is renamed or moved, and the server only knows paths, so each path gets an
/// id here, kept in a file, and renames and moves update the paths under the same ids.
final class ItemIds: @unchecked Sendable {
    private let lock = NSLock()
    private let file: URL
    private var byPath: [String: String] = [:]
    private var byId: [String: String] = [:]
    /// What was chosen for a file or folder: keep it downloaded (true) or online only (false).
    /// What is inside a folder follows the folder, unless chosen otherwise.
    private let keepFile: URL
    private var keep: [String: Bool] = [:]
    /// The folders whose contents the system has (it listed them): what is listed again when the
    /// server's change feed cannot say what changed. "" is the top, always there.
    private let foldersFile: URL
    private var folders: Set<String> = [""]

    init(domain: String) {
        let folder = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        file = folder.appendingPathComponent("ids-\(domain).plist")
        keepFile = folder.appendingPathComponent("keep-\(domain).plist")
        foldersFile = folder.appendingPathComponent("folders-\(domain).plist")
        if let data = try? Data(contentsOf: keepFile), let saved = try? PropertyListDecoder().decode([String: Bool].self, from: data) {
            keep = saved
        }
        if let data = try? Data(contentsOf: file), let saved = try? PropertyListDecoder().decode([String: String].self, from: data) {
            byPath = saved
            for (path, id) in saved { byId[id] = path }
        }
        if let data = try? Data(contentsOf: foldersFile), let saved = try? PropertyListDecoder().decode(Set<String>.self, from: data) {
            folders.formUnion(saved)
        } else {
            // From before folders were remembered: a folder with something known in it was listed.
            for path in byPath.keys { folders.insert((path as NSString).deletingLastPathComponent) }
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
        for (path, choice) in keep where path == old || path.hasPrefix(old + "/") {
            keep[path] = nil
            keep[new + path.dropFirst(old.count)] = choice
        }
        for path in folders where path == old || path.hasPrefix(old + "/") {
            folders.remove(path)
            folders.insert(new + path.dropFirst(old.count))
        }
        saveLocked()
        saveKeepLocked()
        saveFoldersLocked()
    }

    /// Forgets a deleted item and what was inside it.
    func removed(_ old: String) {
        lock.lock()
        defer { lock.unlock() }
        for (path, id) in byPath where path == old || path.hasPrefix(old + "/") {
            byPath[path] = nil
            byId[id] = nil
        }
        for path in keep.keys where path == old || path.hasPrefix(old + "/") {
            keep[path] = nil
        }
        if !old.isEmpty {
            folders = folders.filter { $0 != old && !$0.hasPrefix(old + "/") }
        }
        saveLocked()
        saveKeepLocked()
        saveFoldersLocked()
    }

    /// The choice made for exactly this path, if any.
    func keepChoice(_ path: String) -> Bool? {
        lock.lock()
        defer { lock.unlock() }
        return keep[path]
    }

    /// Whether this path ends up kept downloaded: its own choice, or the nearest folder's.
    func keptDownloaded(_ path: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return keptLocked(path)
    }

    /// Records a choice for a path, replacing any made for what is inside it. Returns the paths
    /// whose own choice changed, which the system has to be told about again.
    func choose(_ path: String, keep choice: Bool) -> [String] {
        lock.lock()
        defer { lock.unlock() }
        let replaced = keep.keys.filter { $0 == path || $0.hasPrefix(path.isEmpty ? "" : path + "/") }
        for old in replaced { keep[old] = nil }
        // Only a choice that differs from the folder's needs saying.
        if keptLocked(path) != choice { keep[path] = choice }
        saveKeepLocked()
        return Array(Set(replaced + [path]))
    }

    private func keptLocked(_ path: String) -> Bool {
        var at = path
        while true {
            if let choice = keep[at] { return choice }
            if at.isEmpty { return false }
            at = (at as NSString).deletingLastPathComponent
        }
    }

    private func saveKeepLocked() {
        if let data = try? PropertyListEncoder().encode(keep) { try? data.write(to: keepFile, options: .atomic) }
    }

    /// After listing a folder: remembers that the system has its contents, and forgets what is no
    /// longer in it (changed outside Files). Returns the identifiers of what it forgot.
    @discardableResult
    func listed(_ folder: String, names: Set<String>) -> [NSFileProviderItemIdentifier] {
        let prefix = folder.isEmpty ? "" : folder + "/"
        lock.lock()
        let gone = byPath.filter { path, _ in
            guard path.hasPrefix(prefix) else { return false }
            let rest = path.dropFirst(prefix.count)
            return !rest.contains("/") && !names.contains(String(rest))
        }
        if folders.insert(folder).inserted { saveFoldersLocked() }
        lock.unlock()
        for path in gone.keys { removed(path) }
        return gone.values.map { NSFileProviderItemIdentifier($0) }
    }

    /// Whether the system has this folder's contents (it was listed).
    func visited(_ folder: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return folders.contains(folder)
    }

    /// Every folder the system has the contents of, each above what is inside it.
    func visitedFolders() -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return folders.sorted()
    }

    private func saveFoldersLocked() {
        if let data = try? PropertyListEncoder().encode(folders) { try? data.write(to: foldersFile, options: .atomic) }
    }

    private func saveLocked() {
        if let data = try? PropertyListEncoder().encode(byPath) { try? data.write(to: file, options: .atomic) }
    }
}
