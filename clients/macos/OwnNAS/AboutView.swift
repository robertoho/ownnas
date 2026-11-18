import AppKit
import OSLog
import SwiftUI

/// This app's version, as the build wrote it into Info.plist (../apple-version.sh: the release
/// version from the workspace Cargo.toml, the build number and the commit from git).
enum AppVersion {
    static var version: String { info("CFBundleShortVersionString") ?? "?" }
    static var build: String { info("CFBundleVersion") ?? "?" }
    static var commit: String? { info("OwnNASCommit") }
    /// "0.9.9 (build 1234, a1b2c3d)".
    static var summary: String { "\(version) (build \(build)\(commit.map { ", \($0)" } ?? ""))" }

    private static func info(_ key: String) -> String? {
        Bundle.main.object(forInfoDictionaryKey: key) as? String
    }
}

/// "About OwnNAS" in the menu: this app's version and build, and the address and version of each
/// server it is signed in to, which it asks the servers for each time it opens. Copy Details puts
/// all of it on the clipboard, to paste into a bug report.
struct AboutView: View {
    @EnvironmentObject private var hub: MacHub
    let close: () -> Void

    private enum Answer: Equatable {
        case checking
        case version(String)
        case failed(String)
    }

    @State private var answers: [String: Answer] = [:]
    @State private var copied = false

    var body: some View {
        VStack(spacing: 16) {
            VStack(spacing: 6) {
                Image(nsImage: NSApp.applicationIconImage)
                    .resizable()
                    .frame(width: 64, height: 64)
                Text("OwnNAS").font(.title2.bold())
                Text("Version \(AppVersion.summary)")
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
            if hub.servers.isEmpty {
                Text("Not signed in to a server yet.").foregroundStyle(.secondary)
            } else {
                Form {
                    ForEach(hub.servers) { server in
                        Section(server.name) {
                            LabeledContent("Address") { Text(server.address).textSelection(.enabled) }
                            LabeledContent("Signed in as") { Text(server.username) }
                            LabeledContent("Server version") { answerText(server) }
                        }
                    }
                }
                .formStyle(.grouped)
                .scrollDisabled(hub.servers.count < 3)
                .frame(minHeight: CGFloat(min(hub.servers.count, 3)) * 120)
            }
            HStack {
                Button(copied ? "Copied" : "Copy Details", action: copy)
                Button("Check Again") { Task { await check() } }
                    .disabled(hub.servers.isEmpty)
                Spacer()
                Button("OK", action: close).keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 420)
        // Again whenever servers are added or taken away while it is open.
        .task(id: hub.servers.map(\.id)) { await check() }
    }

    @ViewBuilder private func answerText(_ server: SavedServer) -> some View {
        switch answers[server.id] ?? .checking {
        case .checking: Text("Checking…").foregroundStyle(.secondary)
        case .version(let version): Text(version).textSelection(.enabled)
        case .failed(let reason): Text(reason).foregroundStyle(.red)
        }
    }

    /// Asks each server for its version (`/api/me`), all at once.
    private func check() async {
        let servers = hub.servers
        for server in servers { answers[server.id] = .checking }
        await withTaskGroup(of: (String, Answer).self) { group in
            for server in servers {
                let token = hub.store.token(server.id)
                group.addTask {
                    guard let token else { return (server.id, .failed("Signed out: sign in again")) }
                    do {
                        let account = try Server(address: server.address, token: token, caPem: server.caPem.map { Data($0.utf8) }, pinned: server.pinned).account()
                        return (server.id, .version(account.version))
                    } catch {
                        return (server.id, .failed(userMessage(error)))
                    }
                }
            }
            for await (id, answer) in group { answers[id] = answer }
        }
        #if DEBUG
        Logger(subsystem: "app.ownnas.mac", category: "about").notice("About: \(details, privacy: .public)")
        #endif
    }

    /// The details as text, for the clipboard.
    private var details: String {
        var lines = ["OwnNAS for Mac \(AppVersion.summary)", "macOS \(ProcessInfo.processInfo.operatingSystemVersionString)"]
        for server in hub.servers {
            let version: String
            switch answers[server.id] ?? .checking {
            case .checking: version = "server version not known yet"
            case .version(let found): version = "server \(found)"
            case .failed(let reason): version = "server version not known (\(reason))"
            }
            lines.append("\(server.name): \(server.address), \(version), signed in as \(server.username)")
        }
        return lines.joined(separator: "\n")
    }

    private func copy() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(details, forType: .string)
        copied = true
        Task {
            try? await Task.sleep(for: .seconds(2))
            copied = false
        }
    }
}

/// The About window: one at a time, brought to the front when asked for again while open, and
/// made afresh (asking the servers again) once closed.
@MainActor
enum AboutWindow {
    private static var window: NSWindow?

    static func show(_ hub: MacHub) {
        if window?.isVisible != true {
            let window = NSWindow(contentViewController: NSHostingController(rootView: AboutView(close: { AboutWindow.window?.close() }).environmentObject(hub)))
            window.title = "About OwnNAS"
            window.styleMask = [.titled, .closable]
            window.isReleasedWhenClosed = false
            window.center()
            self.window = window
        }
        NSApp.activate()
        window?.makeKeyAndOrderFront(nil)
    }
}
