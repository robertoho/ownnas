import PhotosUI
import SwiftUI

/// One folder on a server: open, share, upload, rename, and move to the Trash.
struct BrowserView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let path: String

    @State private var folder: Folder?
    @State private var problem: Error?
    @State private var account: Account?
    @State private var terminals: [TerminalInfo] = []
    @StateObject private var opener = Opener()
    @StateObject private var uploader = Uploader()
    @State private var prompt: NamePrompt?
    @State private var trashing: Entry?
    @State private var pickedPhotos: [PhotosPickerItem] = []
    @State private var choosingPhotos = false
    @State private var choosingFiles = false
    @State private var message: String?

    private var saved: SavedServer? { hub.server(server) }
    private var writable: Bool { account?.readonly == false }
    private var title: String { specialFolderLabel((path as NSString).lastPathComponent).isEmpty ? (saved?.name ?? "") : specialFolderLabel((path as NSString).lastPathComponent) }

    var body: some View {
        content
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { toolbar }
            .refreshable { await load() }
            .task(id: hub.revision) { await load() }
            .task(id: account?.terminal) { await followTerminals() }
            .opener(opener)
            .uploads(uploader)
            .namePrompt($prompt)
            .photosPicker(isPresented: $choosingPhotos, selection: $pickedPhotos, matching: .any(of: [.images, .videos]), preferredItemEncoding: .current)
            .onChange(of: pickedPhotos) { uploadPhotos() }
            .fileImporter(isPresented: $choosingFiles, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
                guard case let .success(urls) = result else { return }
                uploader.upload(hub, server: server, dir: path, files: urls.map { .init(name: $0.lastPathComponent, file: $0, temporary: false) })
            }
            .confirmationDialog("Move “\(trashing?.name ?? "")” to the Trash?", isPresented: Binding(get: { trashing != nil }, set: { if !$0 { trashing = nil } }), titleVisibility: .visible) {
                Button(inTrash(trashing?.path ?? "") ? "Delete forever" : "Move to Trash", role: .destructive) {
                    if let entry = trashing { act { try await hub.call(server) { try $0.trash(path: entry.path) } } }
                    trashing = nil
                }
            } message: {
                Text(inTrash(trashing?.path ?? "") ? "It is already in the Trash, so it is deleted for good." : "You can restore it from the Trash in the web app.")
            }
            .alert(message ?? "", isPresented: Binding(get: { message != nil }, set: { if !$0 { message = nil } })) { Button("OK", role: .cancel) {} }
    }

    @ViewBuilder private var content: some View {
        if let folder {
            List {
                if folder.entries.isEmpty {
                    Text(writable ? "This folder is empty. Tap + to add files." : "This folder is empty")
                        .foregroundStyle(.secondary).frame(maxWidth: .infinity).listRowBackground(Color.clear)
                }
                ForEach(sorted(folder.entries), id: \.path) { entry in
                    row(entry)
                }
                if folder.truncated {
                    Text("Showing the first \(folder.entries.count) items. Open this folder in the browser to see the rest.")
                        .font(.footnote).foregroundStyle(.secondary)
                }
            }
            .listStyle(.plain)
        } else if let problem {
            ContentUnavailableView {
                Label(userMessage(problem), systemImage: "exclamationmark.triangle")
            } actions: {
                if case ClientError.Unauthorized = problem {
                    Button("Sign in") { hub.path = [] }
                } else {
                    Button("Try again") { Task { await load() } }
                }
            }
        } else {
            ProgressView()
        }
    }

    private func row(_ entry: Entry) -> some View {
        let here = entry.dir ? terminals.filter { $0.path == entry.path } : []
        return Button {
            if entry.dir { hub.path.append(.browse(server: server, path: entry.path)) } else { opener.open(hub, server: server, entry: entry) }
        } label: {
            HStack(spacing: 12) {
                EntryIcon(server: server, entry: entry)
                    .overlay(alignment: .bottomTrailing) {
                        if !here.isEmpty {
                            Image(systemName: "terminal.fill").font(.system(size: 10)).padding(3)
                                .background(Circle().fill(here.contains { $0.agent.attention != nil } ? Color.orange : Color.accentColor))
                                .foregroundStyle(.white).offset(x: 4, y: 4)
                        }
                    }
                VStack(alignment: .leading, spacing: 2) {
                    Text(specialFolderLabel(entry.name)).foregroundStyle(.primary).lineLimit(1)
                    Text(details(entry, terminals: here)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
        }
        .contextMenu { menu(entry) }
        .swipeActions {
            if writable {
                Button("Trash", systemImage: "trash", role: .destructive) { trashing = entry }
                Button("Rename", systemImage: "pencil") { rename(entry) }.tint(.blue)
            }
        }
    }

    @ViewBuilder private func menu(_ entry: Entry) -> some View {
        if !entry.dir {
            Button("Share…", systemImage: "square.and.arrow.up") { opener.open(hub, server: server, entry: entry, share: true) }
        }
        if entry.dir, account?.terminal == true {
            Button("Open terminal here", systemImage: "terminal") { openTerminal(entry.path) }
        }
        if writable {
            Button("Rename…", systemImage: "pencil") { rename(entry) }
            Button("Move to Trash", systemImage: "trash", role: .destructive) { trashing = entry }
        }
    }

    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            if writable {
                Menu {
                    Button("Photos and videos", systemImage: "photo.on.rectangle") { choosingPhotos = true }
                    Button("Files", systemImage: "doc") { choosingFiles = true }
                    Button("New folder", systemImage: "folder.badge.plus") {
                        prompt = NamePrompt(title: "New folder", confirm: "Create") { name in
                            act { _ = try await hub.call(server) { try $0.createFolder(parent: path, name: name) } }
                        }
                    }
                } label: { Image(systemName: "plus") }
            }
            Menu {
                Button("Search", systemImage: "magnifyingglass") { hub.path.append(.search(server: server, path: path)) }
                Button("Gallery", systemImage: "photo.on.rectangle.angled") { hub.path.append(.gallery(server: server)) }
                if account?.terminal == true {
                    let attention = terminals.filter { $0.agent.attention != nil || !$0.running }.count
                    Button(attention > 0 ? "Terminals (\(attention) need you)" : "Terminals", systemImage: "terminal") {
                        hub.path.append(.terminals(server: server, path: path))
                    }
                }
            } label: { Image(systemName: "ellipsis.circle") }
        }
    }

    private func load() async {
        do {
            if account == nil { account = try? await hub.account(server) }
            folder = try await hub.call(server) { [path] in try $0.list(path: path) }
            problem = nil
        } catch {
            problem = error
        }
    }

    /// Badges for folders with open terminals, as the web app shows them.
    private func followTerminals() async {
        guard account?.terminal == true else { return }
        while !Task.isCancelled {
            if let found = try? await hub.call(server, { try $0.terminals() }) { terminals = found }
            try? await Task.sleep(for: .seconds(3))
        }
    }

    private func rename(_ entry: Entry) {
        prompt = NamePrompt(title: "Rename", initial: entry.name, confirm: "Rename") { name in
            guard name != entry.name else { return }
            act { _ = try await hub.call(server) { try $0.rename(path: entry.path, name: name) } }
        }
    }

    private func openTerminal(_ folder: String) {
        Task {
            do {
                let info = try await hub.call(server) { try $0.openTerminal(path: folder, rows: 40, cols: 80) }
                hub.path.append(.terminal(server: server, id: info.id))
            } catch ClientError.Locked {
                hub.path.append(.terminals(server: server, path: folder))
            } catch {
                message = userMessage(error)
            }
        }
    }

    private func act(_ action: @escaping () async throws -> Void) {
        Task {
            do {
                try await action()
                hub.changed()
            } catch {
                message = userMessage(error)
            }
        }
    }

    private func uploadPhotos() {
        let items = pickedPhotos
        guard !items.isEmpty else { return }
        pickedPhotos = []
        Task {
            var files: [Uploader.Pending] = []
            for item in items {
                if let picked = try? await item.loadTransferable(type: PickedFile.self) {
                    files.append(.init(name: picked.url.lastPathComponent, file: picked.url, temporary: true))
                }
            }
            if files.count < items.count { message = "\(items.count - files.count) could not be read and were skipped" }
            uploader.upload(hub, server: server, dir: path, files: files)
        }
    }

    private func sorted(_ entries: [Entry]) -> [Entry] {
        entries.sorted { a, b in
            if a.dir != b.dir { return a.dir }
            return a.name.localizedStandardCompare(b.name) == .orderedAscending
        }
    }

    private func details(_ entry: Entry, terminals: [TerminalInfo]) -> String {
        let modified = Date(timeIntervalSince1970: Double(entry.modified) / 1000).formatted(.relative(presentation: .named))
        var text = entry.dir ? "Folder · \(modified)" : "\(bytes(entry.size)) · \(modified)"
        let running = terminals.filter(\.running)
        if let status = running.compactMap({ agentStatus($0) }).first { text += " · \(status)" }
        else if running.count == 1 { text += " · terminal open" }
        else if running.count > 1 { text += " · \(running.count) terminals open" }
        return text
    }
}

/// Files and folders in `path` and below whose name, tags, or contents match.
struct SearchView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let path: String
    @State private var words = ""
    @State private var hits: [SearchHit]?
    @State private var searching = false
    @State private var problem: String?
    @StateObject private var opener = Opener()

    var body: some View {
        List {
            if searching {
                ProgressView().frame(maxWidth: .infinity)
            } else if let problem {
                Text(problem).foregroundStyle(.red)
            } else if let hits, hits.isEmpty {
                Text("Nothing matches “\(words)”.").foregroundStyle(.secondary)
            } else if hits == nil {
                Text("Names, tags, and the words inside documents, PDFs, and pictures. Type tag:name for one tag.")
                    .foregroundStyle(.secondary)
            }
            ForEach(hits ?? [], id: \.path) { hit in
                let entry = hit.entry
                Button {
                    if hit.dir { hub.path.append(.browse(server: server, path: hit.path)) } else { opener.open(hub, server: server, entry: entry) }
                } label: {
                    HStack(spacing: 12) {
                        EntryIcon(server: server, entry: entry)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(specialFolderLabel(hit.name)).foregroundStyle(.primary).lineLimit(1)
                            Text((hit.path as NSString).deletingLastPathComponent.isEmpty ? (hub.server(server)?.name ?? "") : (hit.path as NSString).deletingLastPathComponent)
                                .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                            if let why = hit.matchedContent.map({ "…\($0.trimmingCharacters(in: .whitespacesAndNewlines))…" }) ?? hit.matchedTag.map({ "Tagged \($0)" }) {
                                Text(why).font(.caption).foregroundStyle(.tint).lineLimit(2)
                            }
                        }
                    }
                }
            }
        }
        .listStyle(.plain)
        .navigationTitle(path.isEmpty ? "Search" : "Search \((path as NSString).lastPathComponent)")
        .navigationBarTitleDisplayMode(.inline)
        .searchable(text: $words, placement: .navigationBarDrawer(displayMode: .always), prompt: "Names, tags, or words inside")
        .onSubmit(of: .search) { search() }
        .opener(opener)
    }

    private func search() {
        let typed = words.trimmingCharacters(in: .whitespaces)
        guard !typed.isEmpty else { return }
        searching = true
        problem = nil
        Task {
            defer { searching = false }
            do { hits = try await hub.call(server) { [path] in try $0.search(path: path, words: typed) } } catch { problem = userMessage(error) }
        }
    }
}

extension SearchHit {
    /// As a folder entry, to show and open it like one.
    var entry: Entry {
        Entry(name: name, path: path, dir: dir, size: size, modified: modified * 1000, kind: kind, thumb: kind == "image" || kind == "video" || kind == "pdf")
    }
}
