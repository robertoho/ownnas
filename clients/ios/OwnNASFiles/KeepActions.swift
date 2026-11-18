#if os(macOS)
import FileProvider

/// "Keep Downloaded", "Keep Online Only" and "Refresh" in Finder's menu
/// (`NSExtensionFileProviderActions` in the extension's Info.plist). The keep choice is stored by
/// path and told to the system as each item's content policy, which then does the downloading and
/// keeps new versions coming. Refresh lists a folder (or a file's folder) again from the server.
extension FileProviderExtension: NSFileProviderCustomAction {
    private static let keepDownloaded = "app.ownnas.keep-downloaded"
    private static let keepOnlineOnly = "app.ownnas.keep-online-only"
    private static let refresh = "app.ownnas.refresh"

    func performAction(identifier: NSFileProviderExtensionActionIdentifier, onItemsWithIdentifiers itemIdentifiers: [NSFileProviderItemIdentifier], completionHandler: @escaping (Error?) -> Void) -> Progress {
        let progress = Progress(totalUnitCount: Int64(itemIdentifiers.count))
        Task.detached { [self] in
            do {
                guard let manager = NSFileProviderManager(for: domain) else { throw NSFileProviderError(.providerNotFound) }
                if identifier.rawValue == Self.refresh {
                    // A folder Finder has listed, and the folder each item is in.
                    var folders: Set<String> = []
                    for item in itemIdentifiers {
                        guard let path = ids.path(item) else { continue }
                        if ids.visited(path) { folders.insert(path) }
                        if !path.isEmpty { folders.insert((path as NSString).deletingLastPathComponent) }
                    }
                    addRefreshes(Array(folders))
                    try await manager.signalEnumerator(for: .workingSet)
                    progress.completedUnitCount = progress.totalUnitCount
                    completionHandler(nil)
                    return
                }
                let keep: Bool
                switch identifier.rawValue {
                case Self.keepDownloaded: keep = true
                case Self.keepOnlineOnly: keep = false
                default: throw CocoaError(.featureUnsupported)
                }
                for item in itemIdentifiers {
                    guard let path = ids.path(item) else { throw NSFileProviderError(.noSuchItem) }
                    addPolicyChanges(ids.choose(path, keep: keep))
                }
                try await manager.signalEnumerator(for: .workingSet)
                if !keep {
                    for item in itemIdentifiers {
                        try await removeDownload(item, manager)
                        progress.completedUnitCount += 1
                    }
                }
                completionHandler(nil)
            } catch {
                completionHandler(error)
            }
        }
        return progress
    }

    /// The system refuses until it has heard the item may be removed, so it gets a few tries.
    private func removeDownload(_ item: NSFileProviderItemIdentifier, _ manager: NSFileProviderManager) async throws {
        for attempt in 1...10 {
            do {
                try await manager.evictItem(identifier: item)
                return
            } catch let error as NSFileProviderError where error.code == .noSuchItem {
                return
            } catch let error as NSFileProviderError where (error.code == .nonEvictable || error.code == .nonEvictableChildren) && attempt < 10 {
                try await Task.sleep(for: .seconds(1))
            }
        }
    }
}
#endif
