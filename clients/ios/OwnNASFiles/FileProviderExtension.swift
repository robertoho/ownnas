import FileProvider
import UniformTypeIdentifiers

/// The server in the Files app: one location per signed-in server (a file provider domain whose
/// identifier is the server's id). Folders are listed from the server when they are opened,
/// files are downloaded when opened, and what is created, changed, renamed, moved, or deleted in
/// Files goes to the server (deleting moves to the server's Trash).
final class FileProviderExtension: NSObject, NSFileProviderReplicatedExtension, NSFileProviderThumbnailing {
    let domain: NSFileProviderDomain
    let ids: ItemIds
    private let store = ServerStore()
    private let lock = NSLock()
    private var connection: Server?
    private var account: Account?

    required init(domain: NSFileProviderDomain) {
        self.domain = domain
        ids = ItemIds(domain: domain.identifier.rawValue)
        super.init()
    }

    func invalidate() {}

    /// The server, signed in with the app's token.
    func server() throws -> Server {
        lock.lock()
        defer { lock.unlock() }
        if let connection { return connection }
        let id = domain.identifier.rawValue
        guard let saved = store.load().first(where: { $0.id == id }), let token = store.token(id) else {
            throw NSFileProviderError(.notAuthenticated)
        }
        let made = try Server(address: saved.address, token: token, caPem: saved.caPem.map { Data($0.utf8) }, pinned: saved.pinned)
        connection = made
        return made
    }

    /// Whether the server takes changes (it may run with `--readonly`).
    func writable() -> Bool {
        lock.lock()
        let known = account
        lock.unlock()
        if let known { return !known.readonly }
        guard let fetched = try? server().account() else { return true }
        lock.lock()
        account = fetched
        lock.unlock()
        return !fetched.readonly
    }

    var rootName: String { store.load().first { $0.id == domain.identifier.rawValue }?.name ?? domain.displayName }

    // MARK: Items

    func item(for identifier: NSFileProviderItemIdentifier, request: NSFileProviderRequest, completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void) -> Progress {
        run(completionHandler) { [self] in
            if identifier == .rootContainer { return FileItem.root(name: rootName, writable: writable(), ids: ids) }
            guard let path = ids.path(identifier) else { throw NSFileProviderError(.noSuchItem) }
            return try lookUp(path)
        }
    }

    /// One file or folder, from its folder's listing.
    func lookUp(_ path: String) throws -> FileItem {
        let parent = (path as NSString).deletingLastPathComponent
        let folder = try call { try server().list(path: parent) }
        guard let entry = folder.entries.first(where: { $0.path == path }) else { throw NSFileProviderError(.noSuchItem) }
        return FileItem(entry, writable: writable(), ids: ids)
    }

    func fetchContents(for itemIdentifier: NSFileProviderItemIdentifier, version requestedVersion: NSFileProviderItemVersion?, request: NSFileProviderRequest, completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void) -> Progress {
        let progress = Progress(totalUnitCount: 100)
        let tracker = Cancel(progress)
        Task.detached { [self] in
            do {
                guard let path = ids.path(itemIdentifier) else { throw NSFileProviderError(.noSuchItem) }
                let item = try lookUp(path)
                let file = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
                let size = max(item.size, 1)
                let watch = Watch(tracker) { done in progress.completedUnitCount = Int64(min(100, done * 100 / size)) }
                _ = try call { try server().download(path: path, destination: file.path, transfer: watch) }
                completionHandler(file, item, nil)
            } catch {
                completionHandler(nil, nil, Self.fileError(error))
            }
        }
        return progress
    }

    func createItem(basedOn itemTemplate: NSFileProviderItem, fields: NSFileProviderItemFields, contents url: URL?, options: NSFileProviderCreateItemOptions = [], request: NSFileProviderRequest, completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void) -> Progress {
        let progress = Progress(totalUnitCount: 100)
        let tracker = Cancel(progress)
        Task.detached { [self] in
            do {
                guard let parent = ids.path(itemTemplate.parentItemIdentifier) else { throw NSFileProviderError(.noSuchItem) }
                let name = itemTemplate.filename
                let path: String
                if itemTemplate.contentType == .folder || itemTemplate.contentType?.conforms(to: .folder) == true {
                    do {
                        path = try call { try server().createFolder(parent: parent, name: name) }
                    } catch ClientError.Conflict where options.contains(.mayAlreadyExist) {
                        path = parent.isEmpty ? name : "\(parent)/\(name)"
                    }
                } else {
                    // The system asks again under another name ("name 2") when this one is taken.
                    let conflict: Conflict? = options.contains(.mayAlreadyExist) ? .overwrite : nil
                    try upload(url, to: parent, name: name, modified: itemTemplate.contentModificationDate ?? nil, conflict: conflict, tracker: tracker, progress: progress)
                    path = parent.isEmpty ? name : "\(parent)/\(name)"
                }
                completionHandler(try lookUp(path), [], false, nil)
            } catch {
                completionHandler(nil, [], false, Self.fileError(error))
            }
        }
        return progress
    }

    func modifyItem(_ item: NSFileProviderItem, baseVersion version: NSFileProviderItemVersion, changedFields: NSFileProviderItemFields, contents newContents: URL?, options: NSFileProviderModifyItemOptions = [], request: NSFileProviderRequest, completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void) -> Progress {
        let progress = Progress(totalUnitCount: 100)
        let tracker = Cancel(progress)
        Task.detached { [self] in
            do {
                guard var path = ids.path(item.itemIdentifier) else { throw NSFileProviderError(.noSuchItem) }
                if changedFields.contains(.parentItemIdentifier), let destination = ids.path(item.parentItemIdentifier) {
                    let current = (path as NSString).deletingLastPathComponent
                    if destination != current {
                        let moved = path
                        path = try call { try server().moveTo(path: moved, dest: destination) }
                        ids.moved(from: moved, to: path)
                    }
                }
                if changedFields.contains(.filename), (path as NSString).lastPathComponent != item.filename {
                    let renamed = path
                    path = try call { try server().rename(path: renamed, name: item.filename) }
                    ids.moved(from: renamed, to: path)
                }
                if changedFields.contains(.contents), let newContents {
                    let parent = (path as NSString).deletingLastPathComponent
                    try upload(newContents, to: parent, name: (path as NSString).lastPathComponent, modified: item.contentModificationDate ?? nil, conflict: .overwrite, tracker: tracker, progress: progress)
                }
                completionHandler(try lookUp(path), [], false, nil)
            } catch {
                completionHandler(nil, [], false, Self.fileError(error))
            }
        }
        return progress
    }

    func deleteItem(identifier: NSFileProviderItemIdentifier, baseVersion version: NSFileProviderItemVersion, options: NSFileProviderDeleteItemOptions = [], request: NSFileProviderRequest, completionHandler: @escaping (Error?) -> Void) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        Task.detached { [self] in
            do {
                guard let path = ids.path(identifier), !path.isEmpty else { throw NSFileProviderError(.noSuchItem) }
                // To the server's Trash, where it can be restored from the web app.
                try call { try server().trash(path: path) }
                ids.removed(path)
                progress.completedUnitCount = 1
                completionHandler(nil)
            } catch {
                completionHandler(Self.fileError(error))
            }
        }
        return progress
    }

    private func upload(_ url: URL?, to parent: String, name: String, modified: Date?, conflict: Conflict?, tracker: Cancel, progress: Progress) throws {
        guard let url else { throw NSFileProviderError(.noSuchItem) }
        let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.uint64Value ?? 0
        let fd = open(url.path, O_RDONLY)
        guard fd >= 0 else { throw NSFileProviderError(.noSuchItem) }
        let mtime = modified.map { Int64(($0.timeIntervalSince1970 * 1000).rounded()) }
        let watch = Watch(tracker) { done in progress.completedUnitCount = Int64(min(100, done * 100 / max(size, 1))) }
        let result = try call { try server().uploadFd(dir: parent, name: name, fd: fd, size: size, mtimeMs: mtime, conflict: conflict, transfer: watch) }
        if case .exists = result { throw NSFileProviderError(.filenameCollision) }
    }

    // MARK: Folders

    func enumerator(for containerItemIdentifier: NSFileProviderItemIdentifier, request: NSFileProviderRequest) throws -> NSFileProviderEnumerator {
        if containerItemIdentifier == .workingSet { return WorkingSet(provider: self) }
        guard let path = ids.path(containerItemIdentifier) else { throw NSFileProviderError(.noSuchItem) }
        return FolderEnumerator(path: path, provider: self)
    }

    // MARK: Thumbnails

    func fetchThumbnails(for itemIdentifiers: [NSFileProviderItemIdentifier], requestedSize size: CGSize, perThumbnailCompletionHandler: @escaping (NSFileProviderItemIdentifier, Data?, Error?) -> Void, completionHandler: @escaping (Error?) -> Void) -> Progress {
        let progress = Progress(totalUnitCount: Int64(itemIdentifiers.count))
        Task.detached { [self] in
            for identifier in itemIdentifiers {
                if progress.isCancelled { break }
                let data = ids.path(identifier).flatMap { path in try? call { try server().thumbnail(path: path) } }
                perThumbnailCompletionHandler(identifier, data, nil)
                progress.completedUnitCount += 1
            }
            completionHandler(nil)
        }
        return progress
    }

    // MARK: Helpers

    /// A server call; a rejected sign-in drops the connection so the next call signs in again.
    func call<T>(_ block: () throws -> T) throws -> T {
        do {
            return try block()
        } catch ClientError.Unauthorized {
            lock.lock()
            connection = nil
            lock.unlock()
            throw NSFileProviderError(.notAuthenticated)
        }
    }

    private func run<T>(_ completion: @escaping (T?, Error?) -> Void, _ work: @escaping () throws -> T) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        Task.detached {
            do {
                let value = try work()
                progress.completedUnitCount = 1
                completion(value, nil)
            } catch {
                completion(nil, Self.fileError(error))
            }
        }
        return progress
    }

    /// The error Files understands for a failed server call.
    static func fileError(_ error: Error) -> Error {
        if error is NSFileProviderError { return error }
        switch error {
        case ClientError.NotFound: return NSFileProviderError(.noSuchItem)
        case ClientError.Unauthorized: return NSFileProviderError(.notAuthenticated)
        case ClientError.Conflict: return NSFileProviderError(.filenameCollision)
        case ClientError.Cancelled: return CocoaError(.userCancelled)
        case ClientError.Failed(let reason):
            let lower = reason.lowercased()
            if lower.contains("connect") || lower.contains("timed out") || lower.contains("network") || lower.contains("resolve") {
                return NSFileProviderError(.serverUnreachable)
            }
            return NSError(domain: NSCocoaErrorDomain, code: CocoaError.fileWriteUnknown.rawValue, userInfo: [NSLocalizedDescriptionKey: reason])
        default: return NSFileProviderError(.serverUnreachable)
        }
    }
}

/// A file or folder on the server, as Files shows it.
final class FileItem: NSObject, NSFileProviderItem {
    let path: String
    let name: String
    let isFolder: Bool
    let size: UInt64
    /// Milliseconds since 1970.
    let modified: Int64
    let writable: Bool
    let itemIdentifier: NSFileProviderItemIdentifier
    let parentItemIdentifier: NSFileProviderItemIdentifier

    init(path: String, name: String, isFolder: Bool, size: UInt64, modified: Int64, writable: Bool, ids: ItemIds) {
        self.path = path
        itemIdentifier = ids.id(path)
        parentItemIdentifier = ids.id((path as NSString).deletingLastPathComponent)
        self.name = name
        self.isFolder = isFolder
        self.size = size
        self.modified = modified
        self.writable = writable
    }

    convenience init(_ entry: Entry, writable: Bool, ids: ItemIds) {
        self.init(path: entry.path, name: entry.name, isFolder: entry.dir, size: entry.size, modified: entry.modified, writable: writable, ids: ids)
    }

    static func root(name: String, writable: Bool, ids: ItemIds) -> FileItem {
        FileItem(path: "", name: name, isFolder: true, size: 0, modified: 0, writable: writable, ids: ids)
    }

    var filename: String { name }

    var contentType: UTType {
        if isFolder { return .folder }
        return UTType(filenameExtension: (name as NSString).pathExtension) ?? .data
    }

    var documentSize: NSNumber? { isFolder ? nil : NSNumber(value: size) }

    var contentModificationDate: Date? { modified > 0 ? Date(timeIntervalSince1970: Double(modified) / 1000) : nil }

    var capabilities: NSFileProviderItemCapabilities {
        var can: NSFileProviderItemCapabilities = isFolder ? [.allowsReading, .allowsContentEnumerating] : [.allowsReading]
        #if os(macOS)
        // "Remove Download" in Finder: the file stays on the server, and comes back when opened.
        can.insert(.allowsEvicting)
        #endif
        if writable {
            can.formUnion(isFolder ? [.allowsAddingSubItems] : [.allowsWriting])
            if !path.isEmpty { can.formUnion([.allowsRenaming, .allowsReparenting, .allowsDeleting]) }
        }
        return can
    }

    var itemVersion: NSFileProviderItemVersion {
        let content = Data("\(modified)-\(size)".utf8)
        return NSFileProviderItemVersion(contentVersion: content, metadataVersion: Data("\(modified)-\(size)-\(name)".utf8))
    }
}

/// A folder's files and folders, listed from the server each time Files looks.
final class FolderEnumerator: NSObject, NSFileProviderEnumerator {
    let path: String
    weak var provider: FileProviderExtension?

    init(path: String, provider: FileProviderExtension) {
        self.path = path
        self.provider = provider
    }

    func invalidate() {}

    func enumerateItems(for observer: NSFileProviderEnumerationObserver, startingAt page: NSFileProviderPage) {
        Task.detached { [path, provider] in
            guard let provider else {
                observer.finishEnumeratingWithError(NSFileProviderError(.serverUnreachable))
                return
            }
            do {
                let folder = try provider.call { try provider.server().list(path: path) }
                let writable = provider.writable()
                provider.ids.listed(path, names: Set(folder.entries.map(\.name)))
                observer.didEnumerate(folder.entries.map { FileItem($0, writable: writable, ids: provider.ids) })
                observer.finishEnumerating(upTo: nil)
            } catch {
                observer.finishEnumeratingWithError(FileProviderExtension.fileError(error))
            }
        }
    }

    /// What changed in this folder since the anchor, from the server's change feed.
    func enumerateChanges(for observer: NSFileProviderChangeObserver, from anchor: NSFileProviderSyncAnchor) {
        ChangeFeed.report(provider, from: anchor, to: observer, folder: path)
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        ChangeFeed.anchor(provider, completionHandler)
    }
}

/// What the system watches for changes everywhere: everything it has seen, kept up to date from
/// the server's change feed (the app tells the system when there is something new).
final class WorkingSet: NSObject, NSFileProviderEnumerator {
    weak var provider: FileProviderExtension?

    init(provider: FileProviderExtension) {
        self.provider = provider
    }

    func invalidate() {}

    func enumerateItems(for observer: NSFileProviderEnumerationObserver, startingAt page: NSFileProviderPage) {
        observer.finishEnumerating(upTo: nil)
    }

    func enumerateChanges(for observer: NSFileProviderChangeObserver, from anchor: NSFileProviderSyncAnchor) {
        ChangeFeed.report(provider, from: anchor, to: observer, folder: nil)
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        ChangeFeed.anchor(provider, completionHandler)
    }
}

/// The server's change feed (`/api/sync/changes`): an anchor is the server's cursor, and the
/// changes after it are told to the system as items updated or deleted.
enum ChangeFeed {
    static func anchor(_ provider: FileProviderExtension?, _ completion: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        Task.detached {
            let cursor = provider.flatMap { provider in try? provider.call { try provider.server().changes(cursor: "", waitSeconds: 0) }.cursor }
            completion(NSFileProviderSyncAnchor(Data((cursor ?? "\(Date().timeIntervalSince1970)").utf8)))
        }
    }

    /// Changes after `anchor`: everywhere, or (with `folder`) just in that folder.
    static func report(_ provider: FileProviderExtension?, from anchor: NSFileProviderSyncAnchor, to observer: NSFileProviderChangeObserver, folder: String?) {
        Task.detached {
            guard let provider else {
                observer.finishEnumeratingWithError(NSFileProviderError(.serverUnreachable))
                return
            }
            do {
                let cursor = String(data: anchor.rawValue, encoding: .utf8) ?? ""
                let feed = try provider.call { try provider.server().changes(cursor: cursor, waitSeconds: 0) }
                if !feed.watching {
                    // The server cannot watch its folder: folders are listed again when opened.
                    if folder != nil { observer.finishEnumeratingWithError(NSFileProviderError(.syncAnchorExpired)) }
                    else { observer.finishEnumeratingChanges(upTo: anchor, moreComing: false) }
                    return
                }
                if feed.expired {
                    observer.finishEnumeratingWithError(NSFileProviderError(.syncAnchorExpired))
                    return
                }
                let writable = provider.writable()
                var updated: [NSFileProviderItem] = []
                var deleted: [NSFileProviderItemIdentifier] = []
                for change in feed.changes {
                    if let folder, (change.path as NSString).deletingLastPathComponent != folder { continue }
                    if change.deleted {
                        // Only items the system knows; what was inside a folder goes with it.
                        if let id = provider.ids.known(change.path) {
                            deleted.append(id)
                            provider.ids.removed(change.path)
                        }
                    } else {
                        updated.append(FileItem(path: change.path, name: (change.path as NSString).lastPathComponent, isFolder: change.dir, size: change.size, modified: change.modified, writable: writable, ids: provider.ids))
                    }
                }
                if !deleted.isEmpty { observer.didDeleteItems(withIdentifiers: deleted) }
                if !updated.isEmpty { observer.didUpdate(updated) }
                observer.finishEnumeratingChanges(upTo: NSFileProviderSyncAnchor(Data(feed.cursor.utf8)), moreComing: false)
            } catch {
                observer.finishEnumeratingWithError(FileProviderExtension.fileError(error))
            }
        }
    }
}

/// Cancelling in Files stops the transfer.
final class Cancel: @unchecked Sendable {
    private let progress: Progress
    init(_ progress: Progress) { self.progress = progress }
    var cancelled: Bool { progress.isCancelled }
}

/// A transfer's progress for Files.
final class Watch: Transfer, @unchecked Sendable {
    private let cancel: Cancel
    private let report: (UInt64) -> Void

    init(_ cancel: Cancel, report: @escaping (UInt64) -> Void) {
        self.cancel = cancel
        self.report = report
    }

    func keepGoing(done: UInt64) -> Bool {
        report(done)
        return !cancel.cancelled
    }
}
