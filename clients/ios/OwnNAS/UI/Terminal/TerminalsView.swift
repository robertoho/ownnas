import SwiftUI

/// The server's terminals, with what each coding agent is doing. New ones open in `path`.
struct TerminalsView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let path: String
    @State private var terminals: [TerminalInfo]?
    @State private var locked = false
    @State private var password = ""
    @State private var problem: String?
    @State private var closing: TerminalInfo?

    var body: some View {
        List {
            Section {
                Button("New terminal in \(path.isEmpty ? (hub.server(server)?.name ?? "the library") : (path as NSString).lastPathComponent)", systemImage: "plus.rectangle") { open() }
            }
            if let problem { Text(problem).foregroundStyle(.red) }
            Section("Open terminals") {
                if let terminals, terminals.isEmpty { Text("None").foregroundStyle(.secondary) }
                ForEach(terminals ?? [], id: \.id) { info in
                    Button { hub.path.append(.terminal(server: server, id: info.id)) } label: { TerminalRow(info: info) }
                        .swipeActions { Button("Close", role: .destructive) { closing = info } }
                }
            }
        }
        .navigationTitle("Terminals")
        .refreshable { await load() }
        .task {
            while !Task.isCancelled {
                await load()
                try? await Task.sleep(for: .seconds(3))
            }
        }
        .alert("Terminal password", isPresented: $locked) {
            SecureField("Password", text: $password)
            Button("Unlock") { unlock() }
            Button("Cancel", role: .cancel) { hub.path.removeLast() }
        } message: { Text("This server asks for a separate password before terminals open.") }
        .confirmationDialog("Close “\(closing.map(terminalTitle) ?? "")”?", isPresented: Binding(get: { closing != nil }, set: { if !$0 { closing = nil } }), titleVisibility: .visible) {
            Button("Close terminal", role: .destructive) {
                if let id = closing?.id { Task { try? await hub.call(server) { try $0.closeTerminal(id: id) }; await load() } }
                closing = nil
            }
        } message: { Text("Everything running in it stops.") }
    }

    private func load() async {
        do {
            terminals = try await hub.call(server) { try $0.terminals() }
            problem = nil
        } catch ClientError.Locked {
            if !locked { locked = true }
        } catch {
            problem = userMessage(error)
        }
    }

    private func open() {
        Task {
            do {
                let info = try await hub.call(server) { [path] in try $0.openTerminal(path: path, rows: 40, cols: 80) }
                hub.path.append(.terminal(server: server, id: info.id))
            } catch ClientError.Locked {
                locked = true
            } catch {
                problem = userMessage(error)
            }
        }
    }

    private func unlock() {
        let typed = password
        password = ""
        Task {
            do {
                try await hub.call(server) { try $0.unlockTerminals(password: typed) }
                await load()
            } catch {
                problem = userMessage(error)
            }
        }
    }
}

private struct TerminalRow: View {
    let info: TerminalInfo

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: info.running ? "terminal.fill" : "stop.circle.fill")
                .font(.title2)
                .foregroundStyle(info.running ? (agentColor(info) ?? .accentColor) : (info.exitCode ?? 0 == 0 ? .gray : .red))
                .frame(width: 32)
            VStack(alignment: .leading, spacing: 2) {
                Text(terminalTitle(info)).foregroundStyle(.primary).lineLimit(1)
                Text(info.running ? (agentStatus(info) ?? info.path.ifEmpty("/")) : endedText(info))
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                if let message = info.agent.lastMessage, !message.isEmpty {
                    Text(message).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
            }
        }
    }
}

func terminalTitle(_ info: TerminalInfo) -> String {
    info.name ?? info.title ?? ((info.path as NSString).lastPathComponent.ifEmpty("Terminal"))
}

/// What a terminal's coding agent is doing, "Claude Code · needs you"; nil without an agent.
func agentStatus(_ info: TerminalInfo) -> String? {
    guard let name = info.agent.name else { return nil }
    let label = info.agent.label ?? name
    // Attention outlasts the status that caused it (an agent can go idle after asking).
    if info.agent.attention != nil { return "\(label) · needs you" }
    switch info.agent.status {
    case "working": return "\(label) · working"
    case "waiting": return "\(label) · needs you"
    case "done": return "\(label) · done"
    case "error": return "\(label) · error"
    default: return "\(label) · idle"
    }
}

func agentColor(_ info: TerminalInfo) -> Color? {
    guard info.agent.name != nil else { return nil }
    if info.agent.attention != nil { return .orange }
    switch info.agent.status {
    case "working": return .blue
    case "waiting": return .orange
    case "done": return .green
    case "error": return .red
    default: return .gray
    }
}

func endedText(_ info: TerminalInfo) -> String {
    (info.exitCode ?? 0) == 0 ? "Ended" : "Ended · exit code \(info.exitCode!)"
}
