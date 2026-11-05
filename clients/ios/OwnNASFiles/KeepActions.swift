#if os(macOS)
import FileProvider

/// "Keep Downloaded" and "Keep Online Only" in Finder's menu (`NSExtensionFileProviderActions` in
/// the extension's Info.plist). The choice is stored by path and told to the system as each
/// item's content policy, which then does the downloading and keeps new versions coming.
extension FileProviderExtension: NSFileProviderCustomAction {
    private static let keepDownloaded = "app.ownnas.keep-downloaded"
    private static let keepOnlineOnly = "app.ownnas.keep-online-only"

    func performAction(identifier: NSFileProviderExtensionActionIdentifier, onItemsWithIdentifiers itemIdentifiers: [NSFileProviderItemIdentifier], completionHandler: @escaping (Error?) -> Void) -> Progress {
        let progress = Progress(totalUnitCount: Int64(itemIdentifiers.count))
        Task.detached { [self] in
            do {
                guard let manager = NSFileProviderManager(for: domain) else { throw NSFileProviderError(.providerNotFound) }
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
