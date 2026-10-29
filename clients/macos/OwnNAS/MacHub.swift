import AppKit
import OSLog
import FileProvider
import ServiceManagement
import SwiftUI

/// The Mac app's state: the servers signed in, their places in Finder's sidebar (one File
/// Provider domain each), and a watch on each server's change feed that tells Finder when
/// something changed there, so it shows new and changed files without being asked.
@MainActor
final class MacHub: ObservableObject {
    @Published private(set) var servers: [SavedServer] = []
    /// What each server is doing, for the menu ("Up to date", or what went wrong).
    @Published private(set) var status: [String: String] = [:]
    @Published private(set) var opensAtLogin = SMAppService.mainApp.status == .enabled
    /// Servers whose Finder location macOS keeps off until the person allows it in System Settings.
    @Published private(set) var needsAllowing: Set<String> = []

    let store = ServerStore()
    private var watches: [String: Task<Void, Never>] = [:]
    private var signInWindow: NSWindow?

    init() {
        servers = store.load()
        Task {
            await syncDomains()
            await checkAllowed()
            for server in servers where store.token(server.id) != nil { watch(server) }
            #if DEBUG
            // Tests sign in without the window: OWNNAS_TEST_ADDRESS, _USER and _PASSWORD.
            let env = ProcessInfo.processInfo.environment
            // And sign out of a test server again: OWNNAS_TEST_SIGN_OUT is its address.
            if let address = env["OWNNAS_TEST_SIGN_OUT"] {
                for server in servers where server.address == address { signOut(server.id) }
                return
            }
            if let address = env["OWNNAS_TEST_ADDRESS"], let user = env["OWNNAS_TEST_USER"], let password = env["OWNNAS_TEST_PASSWORD"] {
                do { try await signIn(address: address, username: user, password: password, caPem: nil, pinned: []) }
                catch { NSLog("OwnNAS test sign-in failed: %@", userMessage(error)) }
                return
            }
            #endif
            if servers.isEmpty { showSignIn() }
        }
    }

    // MARK: Servers

    /// Signs in and remembers the server (or signs it in again), and puts it in Finder.
    func signIn(address: String, username: String, password: String, caPem: String?, pinned: [String]) async throws {
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
        await syncDomains()
        watch(saved)
        openInFinder(saved.id)
    }

    /// Forgets a server here, takes it out of Finder, and ends its session there.
    func signOut(_ id: String) {
        guard let saved = servers.first(where: { $0.id == id }) else { return }
        watches.removeValue(forKey: id)?.cancel()
        let token = store.token(id)
        servers.removeAll { $0.id == id }
        store.save(servers)
        store.setToken(id, nil)
        status[id] = nil
        Task { await syncDomains() }
        if let token {
            Task.detached { try? Server(address: saved.address, token: token, caPem: saved.caPem.map { Data($0.utf8) }, pinned: saved.pinned).signOut() }
        }
    }

    /// One place in Finder's sidebar for each signed-in server, named after its shared folder.
    private func syncDomains() async {
        let existing = (try? await NSFileProviderManager.domains()) ?? []
        let wanted = servers.filter { store.token($0.id) != nil }
        for domain in existing where !wanted.contains(where: { $0.id == domain.identifier.rawValue }) {
            try? await NSFileProviderManager.remove(domain)
        }
        for server in wanted {
            // Two servers may share a folder name; the host tells them apart.
            let twin = wanted.contains { $0.id != server.id && $0.name == server.name }
            let name = twin ? "\(server.name) (\(server.host))" : server.name
            let token = store.token(server.id)
            if let domain = existing.first(where: { $0.identifier.rawValue == server.id }), domain.displayName == name, domain.userInfo?["token"] as? String == token {
                if domain.userEnabled { needsAllowing.remove(server.id) } else { needsAllowing.insert(server.id) }
                continue
            }
            do {
                // The extension signs in with the token handed over here (see ServerStore).
                let domain = NSFileProviderDomain(identifier: NSFileProviderDomainIdentifier(server.id), displayName: name)
                domain.userInfo = ["token": token ?? ""]
                try await NSFileProviderManager.add(domain)
            } catch {
                status[server.id] = "Could not add to Finder: \(error.localizedDescription)"
            }
        }
    }

    private func domain(_ id: String) -> NSFileProviderDomain? {
        guard let server = servers.first(where: { $0.id == id }) else { return nil }
        return NSFileProviderDomain(identifier: NSFileProviderDomainIdentifier(id), displayName: server.name)
    }

    func openInFinder(_ id: String) {
        guard let domain = domain(id), let manager = NSFileProviderManager(for: domain) else { return }
        manager.getUserVisibleURL(for: .rootContainer) { url, error in
            Task { @MainActor in
                // The URL is security-scoped: the sandbox lets the app hand it to Finder while
                // it holds access.
                if let url {
                    let scoped = url.startAccessingSecurityScopedResource()
                    defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                    NSWorkspace.shared.activateFileViewerSelecting([url])
                }
                else if let error { self.status[id] = "Could not open in Finder: \(error.localizedDescription)" }
            }
        }
    }

    /// Checks again whether macOS lets the Finder locations sync (the person may have just
    /// allowed them).
    func checkAllowed() async {
        let domains = (try? await NSFileProviderManager.domains()) ?? []
        #if DEBUG
        for domain in domains {
            Logger(subsystem: "app.ownnas.mac", category: "domains").notice("domain \(domain.identifier.rawValue, privacy: .public) enabled \(domain.userEnabled, privacy: .public) hasToken \((domain.userInfo?["token"] as? String)?.isEmpty == false, privacy: .public) disconnected \(domain.isDisconnected, privacy: .public)")
        }
        #endif
        needsAllowing = Set(domains.filter { !$0.userEnabled }.map(\.identifier.rawValue)).intersection(servers.map(\.id))
    }

    /// System Settings where File Provider extensions are turned on (General › Login Items &
    /// Extensions › File Providers).
    func openExtensionSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.LoginItems-Settings.extension") { NSWorkspace.shared.open(url) }
    }

    // MARK: Changes on the server

    /// Follows a server's change feed while the app runs: each change tells Finder to ask the
    /// Finder extension what changed (it reads the feed itself, from where it got to).
    private func watch(_ saved: SavedServer) {
        watches[saved.id]?.cancel()
        let store = self.store
        watches[saved.id] = Task { [weak self] in
            var cursor = ""
            var server: Server?
            while !Task.isCancelled {
                do {
                    if server == nil {
                        guard let token = store.token(saved.id) else { throw ClientError.Unauthorized }
                        server = try Server(address: saved.address, token: token, caPem: saved.caPem.map { Data($0.utf8) }, pinned: saved.pinned)
                    }
                    let connection = server!
                    let asked = cursor
                    let feed = try await Task.detached { try connection.changes(cursor: asked, waitSeconds: 25) }.value
                    if !asked.isEmpty && (feed.expired || !feed.changes.isEmpty) { self?.tellFinder(saved.id) }
                    cursor = feed.cursor
                    self?.status[saved.id] = feed.watching ? "Up to date" : "Up to date (the server cannot watch its folder: folders refresh when opened)"
                } catch ClientError.Unauthorized {
                    self?.status[saved.id] = "Signed out by the server: sign in again"
                    return
                } catch {
                    server = nil
                    self?.status[saved.id] = "Cannot reach the server: \(userMessage(error))"
                    try? await Task.sleep(for: .seconds(15))
                }
            }
        }
    }

    private func tellFinder(_ id: String) {
        guard let domain = domain(id) else { return }
        NSFileProviderManager(for: domain)?.signalEnumerator(for: .workingSet) { _ in }
    }

    // MARK: The app

    func setOpensAtLogin(_ on: Bool) {
        do {
            if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
        } catch {
            NSAlert(error: error).runModal()
        }
        opensAtLogin = SMAppService.mainApp.status == .enabled
    }

    /// The window to add a server (or sign in to one again).
    func showSignIn(_ existing: SavedServer? = nil) {
        signInWindow?.close()
        let window = NSWindow(contentViewController: NSHostingController(rootView: SignInView(existing: existing, close: { [weak self] in self?.signInWindow?.close() }).environmentObject(self)))
        window.title = existing == nil ? "Add an OwnNAS server" : "Sign in to \(existing!.name)"
        window.styleMask = [.titled, .closable]
        window.isReleasedWhenClosed = false
        window.center()
        signInWindow = window
        NSApp.activate()
        window.makeKeyAndOrderFront(nil)
    }
}

/// A message for a failed call, in words for people.
func userMessage(_ error: Error) -> String {
    switch error {
    case ClientError.Unauthorized: return "Wrong username or password"
    case ClientError.NotFound: return "Not found on the server"
    case ClientError.Untrusted: return "The server's certificate is not trusted"
    case ClientError.Failed(let reason): return reason
    default: return error.localizedDescription
    }
}
