import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// Adds a server, or signs in again to one. A server whose HTTPS certificate is not publicly
/// trusted shows its fingerprint to compare and pin, or takes the `ca.crt` from
/// `ownnas tls generate`.
struct SignInView: View {
    @EnvironmentObject private var hub: MacHub
    let existing: SavedServer?
    let close: () -> Void

    @State private var address = ""
    @State private var username = ""
    @State private var password = ""
    @State private var caPem: String?
    @State private var pinned: [String] = []
    @State private var working = false
    @State private var problem: String?
    @State private var fingerprint: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Your OwnNAS server's files show in Finder's sidebar. They download when you open them, and changes go back to the server.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Form {
                TextField("Server address", text: $address, prompt: Text("https://nas.example:8787"))
                    .textContentType(.URL)
                TextField("Username", text: $username).textContentType(.username)
                SecureField("Password", text: $password).textContentType(.password)
                    .onSubmit(signIn)
                LabeledContent("Certificate") {
                    Button(caPem == nil ? "Trust a private certificate authority…" : "Certificate authority chosen ✓", action: chooseCA)
                }
            }
            if let problem {
                Text(problem).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer()
                Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                if working { ProgressView().controlSize(.small) }
                Button("Sign in", action: signIn)
                    .keyboardShortcut(.defaultAction)
                    .disabled(working || address.isEmpty || username.isEmpty || password.isEmpty)
            }
        }
        .padding(20)
        .frame(width: 440)
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
    }

    private func chooseCA() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.x509Certificate, .data, .text]
        panel.message = "Choose the ca.crt from “ownnas tls generate”"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        if let text = try? String(contentsOf: url, encoding: .utf8), text.contains("BEGIN CERTIFICATE") {
            caPem = text
            problem = nil
        } else {
            problem = "That is not a PEM certificate (ca.crt)."
        }
    }

    private func signIn() {
        guard !working, !address.isEmpty, !username.isEmpty, !password.isEmpty else { return }
        working = true
        problem = nil
        Task {
            defer { working = false }
            do {
                try await hub.signIn(address: address, username: username, password: password, caPem: caPem, pinned: pinned)
                close()
            } catch ClientError.Untrusted {
                let typed = address
                do {
                    fingerprint = try await Task.detached { try serverFingerprint(address: try normalizeAddress(input: typed)) }.value
                } catch {
                    problem = userMessage(error)
                }
            } catch {
                problem = userMessage(error)
            }
        }
    }
}
