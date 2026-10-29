import SwiftUI

/// OwnNAS for the Mac: lives in the menu bar; its servers live in Finder's sidebar.
@main
struct OwnNASApp: App {
    @StateObject private var hub = MacHub()

    var body: some Scene {
        MenuBarExtra("OwnNAS", systemImage: "externaldrive.connected.to.line.below") {
            MenuContent().environmentObject(hub)
                // The menu opening is a good moment to see whether the person allowed it.
                .task { await hub.checkAllowed() }
        }
    }
}

/// The menu: each server with how it is doing, and the app's own choices.
struct MenuContent: View {
    @EnvironmentObject private var hub: MacHub

    var body: some View {
        if hub.servers.isEmpty {
            Text("No servers yet")
        }
        if !hub.needsAllowing.isEmpty {
            Text("Finder shows OwnNAS once you allow it:")
            Button("Allow in System Settings…") { hub.openExtensionSettings() }
            Divider()
        }
        ForEach(hub.servers) { server in
            Section(server.name) {
                Text(hub.status[server.id] ?? "Connecting…")
                Button("Open in Finder") { hub.openInFinder(server.id) }
                Button("Sign in again…") { hub.showSignIn(server) }
                Button("Sign out of \(server.host)") { hub.signOut(server.id) }
            }
        }
        Divider()
        Button("Add a server…") { hub.showSignIn() }
        Toggle("Open at login", isOn: Binding(get: { hub.opensAtLogin }, set: { hub.setOpensAtLogin($0) }))
        Divider()
        Button("Quit OwnNAS") { NSApp.terminate(nil) }.keyboardShortcut("q")
    }
}
