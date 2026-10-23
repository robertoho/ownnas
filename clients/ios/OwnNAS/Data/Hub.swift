import Foundation
import SwiftUI

/// Where the app is: the screens on the navigation stack.
enum Route: Hashable {
    case browse(server: String, path: String)
    case search(server: String, path: String)
    case gallery(server: String)
    case album(server: String, id: Int64)
    case person(server: String, id: Int64)
    case tag(server: String, tag: String)
    case photoSearch(server: String, words: String)
    case similar(server: String, path: String)
    case hidden(server: String)
    case place(server: String, name: String, west: Double, south: Double, east: Double, north: Double)
    case viewer(server: String, list: Int, start: Int)
    case terminals(server: String, path: String)
    case terminal(server: String, id: String)
}

/// The signed-in servers and the app's navigation. Server calls block, so [call] runs them off
/// the main thread.
@MainActor
final class Hub: ObservableObject {
    @Published private(set) var servers: [SavedServer]
    @Published var path: [Route] = []
    /// Bumped after every change made from this device, so open folders reload.
    @Published private(set) var revision = 0

    let store = ServerStore()
    private var connections: [String: Server] = [:]
    private var accounts: [String: Account] = [:]

    /// The photos the viewer swipes through, by list id.
    private(set) var viewerLists: [Int: [MediaItem]] = [:]
    private var nextList = 0

    init() {
        servers = store.load()
    }

    func server(_ id: String) -> SavedServer? { servers.first { $0.id == id } }

    func signedIn(_ id: String) -> Bool { store.token(id) != nil }

    private func connection(_ id: String) throws -> Server {
        if let existing = connections[id] { return existing }
        guard let saved = server(id) else { throw ClientError.NotFound }
        guard let token = store.token(id) else { throw ClientError.Unauthorized }
        let made = try Server(address: saved.address, token: token, caPem: saved.caPem.map { Data($0.utf8) }, pinned: saved.pinned)
        made.setGalleryWorkspace(id: store.galleryWorkspace(id))
        connections[id] = made
        return made
    }

    /// Runs a server call off the main thread. A rejected token is forgotten, so the app asks to
    /// sign in again.
    func call<T>(_ id: String, _ block: @escaping (Server) throws -> T) async throws -> T {
        let server = try connection(id)
        do {
            return try await Task.detached(priority: .userInitiated) { try block(server) }.value
        } catch ClientError.Unauthorized {
            connections[id] = nil
            accounts[id] = nil
            store.setToken(id, nil)
            objectWillChange.send()
            throw ClientError.Unauthorized
        }
    }

    /// Who is signed in, and whether the server takes changes. Fetched once per app run.
    func account(_ id: String) async throws -> Account {
        if let known = accounts[id] { return known }
        let fetched = try await call(id) { try $0.account() }
        accounts[id] = fetched
        return fetched
    }

    /// Signs in and remembers the server (or signs it in again).
    func signIn(address: String, username: String, password: String, caPem: String?, pinned: [String]) async throws -> SavedServer {
        let (normalized, token, account) = try await Task.detached(priority: .userInitiated) { () throws -> (String, String, Account) in
            let normalized = try normalizeAddress(input: address)
            let ca = caPem.map { Data($0.utf8) }
            let token = try OwnNAS.signIn(address: normalized, username: username.trimmingCharacters(in: .whitespaces), password: password, caPem: ca, pinned: pinned)
            let account = try Server(address: normalized, token: token, caPem: ca, pinned: pinned).account()
            return (normalized, token, account)
        }.value
        let existing = servers.first { $0.address == normalized && $0.username == account.username }
        let saved = SavedServer(id: existing?.id ?? UUID().uuidString, address: normalized, username: account.username, name: account.rootName, caPem: caPem, pinned: pinned)
        if let index = servers.firstIndex(where: { $0.id == saved.id }) { servers[index] = saved } else { servers.append(saved) }
        store.save(servers)
        store.setToken(saved.id, token)
        connections[saved.id] = nil
        accounts[saved.id] = account
        return saved
    }

    /// Forgets a server here and ends its session there, best effort.
    func remove(_ id: String) {
        let server = connections.removeValue(forKey: id)
        accounts[id] = nil
        servers.removeAll { $0.id == id }
        store.save(servers)
        store.setToken(id, nil)
        store.setGalleryWorkspace(id, nil)
        if let server { Task.detached { try? server.signOut() } }
    }

    func changed() { revision += 1 }

    // MARK: Gallery

    func galleryWorkspace(_ id: String) -> Int64? { store.galleryWorkspace(id) }

    func setGalleryWorkspace(_ id: String, _ workspace: Int64?) {
        store.setGalleryWorkspace(id, workspace)
        connections[id]?.setGalleryWorkspace(id: workspace)
    }

    func openViewer(_ server: String, _ items: [MediaItem], start: Int) {
        let id = nextList
        nextList += 1
        viewerLists[id] = items
        path.append(.viewer(server: server, list: id, start: start))
    }

    // MARK: Files

    /// A local copy of a server file, downloaded unless an unchanged copy is already cached.
    /// `modified` is in milliseconds.
    func cachedCopy(_ id: String, path: String, size: UInt64, modified: Int64, transfer: Transfer) async throws -> URL {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let file = caches.appendingPathComponent("opened/\(id)/\(path)")
        let stamp = Date(timeIntervalSince1970: Double(modified) / 1000)
        if let attributes = try? FileManager.default.attributesOfItem(atPath: file.path),
           (attributes[.size] as? NSNumber)?.uint64Value == size,
           let date = attributes[.modificationDate] as? Date, abs(date.timeIntervalSince(stamp)) < 1 {
            return file
        }
        try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        _ = try await call(id) { try $0.download(path: path, destination: file.path, transfer: transfer) }
        try? FileManager.default.setAttributes([.modificationDate: stamp], ofItemAtPath: file.path)
        return file
    }
}

/// Follows a transfer for a progress bar, and cancels it.
final class Tracker: Transfer, @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    private let onProgress: (UInt64) -> Void

    init(_ onProgress: @escaping (UInt64) -> Void = { _ in }) {
        self.onProgress = onProgress
    }

    func cancel() {
        lock.lock()
        cancelled = true
        lock.unlock()
    }

    func keepGoing(done: UInt64) -> Bool {
        onProgress(done)
        lock.lock()
        defer { lock.unlock() }
        return !cancelled
    }
}

/// What to tell the user about a failed call.
func userMessage(_ error: Error) -> String {
    switch error {
    case ClientError.Unauthorized: return "Sign in again"
    case ClientError.NotFound: return "Not found on the server"
    case ClientError.Conflict: return "Changed on the server"
    case ClientError.Cancelled: return "Cancelled"
    case ClientError.Locked: return "Enter the terminal password"
    case ClientError.Untrusted: return "The server's certificate is not trusted"
    case ClientError.Failed(let reason): return reason
    default: return error.localizedDescription
    }
}

func isCancel(_ error: Error) -> Bool {
    if case ClientError.Cancelled = error { return true }
    return error is CancellationError
}
