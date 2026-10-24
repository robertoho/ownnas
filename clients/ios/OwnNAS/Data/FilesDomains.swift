import FileProvider

/// The servers in the Files app: one location (file provider domain) per signed-in server.
enum FilesDomains {
    /// Adds a location for each signed-in server and removes the others.
    static func sync(_ servers: [SavedServer], signedIn: Set<String>) async {
        let existing = (try? await NSFileProviderManager.domains()) ?? []
        let wanted = servers.filter { signedIn.contains($0.id) }
        for domain in existing where !wanted.contains(where: { $0.id == domain.identifier.rawValue }) {
            try? await NSFileProviderManager.remove(domain)
        }
        for server in wanted {
            // Two servers may share a folder name; the host tells them apart.
            let twin = wanted.contains { $0.id != server.id && $0.name == server.name }
            let name = twin ? "\(server.name) (\(server.host))" : server.name
            if let domain = existing.first(where: { $0.identifier.rawValue == server.id }), domain.displayName == name { continue }
            try? await NSFileProviderManager.add(NSFileProviderDomain(identifier: NSFileProviderDomainIdentifier(server.id), displayName: name))
        }
    }
}
