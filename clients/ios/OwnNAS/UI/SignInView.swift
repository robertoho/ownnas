import SwiftUI
import UniformTypeIdentifiers

/// Adds a server, or signs in again to an existing one. A server whose HTTPS certificate is not
/// publicly trusted shows its fingerprint to compare and pin, or takes the `ca.crt` from
/// `ownnas tls generate`.
struct SignInView: View {
    @EnvironmentObject private var hub: Hub
    @Environment(\.dismiss) private var dismiss
    let existing: SavedServer?

    @State private var address = ""
    @State private var username = ""
    @State private var password = ""
    @State private var caPem: String?
    @State private var pinned: [String] = []
    @State private var working = false
    @State private var problem: String?
    @State private var fingerprint: String?
    @State private var choosingCA = false

    var body: some View {
        Form {
            Section {
                TextField("Server address", text: $address)
                    .textContentType(.URL).keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                TextField("Username", text: $username)
                    .textContentType(.username).textInputAutocapitalization(.never).autocorrectionDisabled()
                SecureField("Password", text: $password).textContentType(.password)
                    .onSubmit(signIn)
            } footer: {
                Text("Use https:// when the server is outside your home network or VPN.")
            }
            Section {
                Button(caPem == nil ? "Trust a private certificate authority…" : "Certificate authority chosen ✓") { choosingCA = true }
            } footer: {
                Text("For servers using the certificate from “ownnas tls generate”: choose its ca.crt.")
            }
            if let problem {
                Section { Text(problem).foregroundStyle(.red) }
            }
        }
        .navigationTitle(existing == nil ? "Add server" : "Sign in")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            ToolbarItem(placement: .confirmationAction) {
                if working { ProgressView() } else {
                    Button("Sign in", action: signIn).disabled(address.isEmpty || username.isEmpty || password.isEmpty)
                }
            }
        }
        .onAppear {
            guard let existing, address.isEmpty else { return }
            address = existing.address
            username = existing.username
            caPem = existing.caPem
            pinned = existing.pinned
        }
        .alert("Is this your server?", isPresented: Binding(get: { fingerprint != nil }, set: { if !$0 { fingerprint = nil } })) {
            Button("Trust it") {
                if let fingerprint { pinned.append(fingerprint) }
                fingerprint = nil
                signIn()
            }
            Button("Cancel", role: .cancel) { fingerprint = nil }
        } message: {
            Text("Its certificate is not publicly trusted. Compare this fingerprint with the one “ownnas serve” prints:\n\n\(fingerprint ?? "")")
        }
        .fileImporter(isPresented: $choosingCA, allowedContentTypes: [.x509Certificate, .data, .text]) { result in
            guard case let .success(url) = result else { return }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            if let text = try? String(contentsOf: url, encoding: .utf8), text.contains("BEGIN CERTIFICATE") {
                caPem = text
                problem = nil
            } else {
                problem = "That is not a PEM certificate (ca.crt)."
            }
        }
    }

    private func signIn() {
        guard !working else { return }
        working = true
        problem = nil
        Task {
            defer { working = false }
            do {
                let saved = try await hub.signIn(address: address, username: username, password: password, caPem: caPem, pinned: pinned)
                dismiss()
                hub.path = [.browse(server: saved.id, path: "")]
            } catch ClientError.Untrusted {
                let typed = address
                do {
                    fingerprint = try await Task.detached { try serverFingerprint(address: try normalizeAddress(input: typed)) }.value
                } catch {
                    problem = userMessage(error)
                }
            } catch ClientError.Unauthorized {
                problem = "Wrong username or password"
            } catch {
                problem = userMessage(error)
            }
        }
    }
}
