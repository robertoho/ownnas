import SwiftUI

/// The servers signed in from this device.
struct ServersView: View {
    @EnvironmentObject private var hub: Hub
    @State private var signingIn: SignInTarget?
    @State private var removing: SavedServer?

    var body: some View {
        List {
            if hub.servers.isEmpty {
                VStack(spacing: 12) {
                    Image(systemName: "externaldrive.connected.to.line.below").font(.system(size: 44)).foregroundStyle(.tint)
                    Text("Connect to your OwnNAS").font(.title2.bold())
                    Text("Sign in with the address, username, and password you use in the browser.")
                        .multilineTextAlignment(.center).foregroundStyle(.secondary)
                    Button("Add server") { signingIn = SignInTarget(existing: nil) }.buttonStyle(.borderedProminent)
                }
                .frame(maxWidth: .infinity).padding(.vertical, 40).listRowBackground(Color.clear)
            }
            ForEach(hub.servers) { server in
                Button {
                    if hub.signedIn(server.id) { hub.path.append(.browse(server: server.id, path: "")) } else { signingIn = SignInTarget(existing: server) }
                } label: {
                    HStack(spacing: 14) {
                        Image(systemName: "externaldrive.fill").font(.title2).foregroundStyle(.tint).frame(width: 36)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(server.name).font(.headline).foregroundStyle(.primary)
                            Text(hub.signedIn(server.id) ? "\(server.username) · \(server.host)" : "Signed out · tap to sign in")
                                .font(.subheadline).foregroundStyle(.secondary)
                        }
                    }
                }
                .contextMenu {
                    Button("Gallery", systemImage: "photo.on.rectangle") { hub.path.append(.gallery(server: server.id)) }
                    Button("Sign in again", systemImage: "person.badge.key") { signingIn = SignInTarget(existing: server) }
                    Button("Remove", systemImage: "trash", role: .destructive) { removing = server }
                }
                .swipeActions {
                    Button("Remove", role: .destructive) { removing = server }
                }
            }
        }
        .navigationTitle("OwnNAS")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button("Add server", systemImage: "plus") { signingIn = SignInTarget(existing: nil) }
            }
        }
        .sheet(item: $signingIn) { target in
            NavigationStack { SignInView(existing: target.existing) }
        }
        .confirmationDialog("Remove \(removing?.name ?? "")?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible) {
            Button("Remove server", role: .destructive) {
                if let removing { hub.remove(removing.id) }
                removing = nil
            }
        } message: {
            Text("This device signs out. Nothing on the server changes.")
        }
    }
}

struct SignInTarget: Identifiable {
    let id = UUID()
    let existing: SavedServer?
}
