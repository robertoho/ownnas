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
    @State private var trashing: [Entry]?
    @State private var editMode: EditMode = .inactive
    @State private var selection = Set<String>()
    @StateObject private var runner = BulkRunner()
    @State private var picking: Picking?
    @State private var sharingFiles: SharedFiles?
    @State private var choosing: Choosing?
    @State private var resizing: Items?
    @State private var versionsOf: Entry?
    @State private var albumFor: Items?
    @State private var pagesOf: Entry?
    @State private var rotating: Entry?
    @State private var pagesText = ""
    @State private var emptyingTrash = false
    @State private var pickedPhotos: [PhotosPickerItem] = []
    @State private var choosingPhotos = false
    @State private var choosingFiles = false
    @State private var scanning = false
    @State private var scanned: ScannedPages?
    @State private var scanReady: ScannedPages?
    @State private var message: String?

    /// Choosing where to move or copy.
    struct Picking: Identifiable {
        let id = UUID()
        let entries: [Entry]
        let copy: Bool
    }

    /// Items for a sheet.
    struct Items: Identifiable {
        let id = UUID()
        let entries: [Entry]
    }

    /// A choice asked for before an action: an archive or image format, or how to protect.
    struct Choosing {
        enum Kind { case compress, convert, protect }
        let kind: Kind
        let entries: [Entry]
    }

    struct SharedFiles: Identifiable {
        let id = UUID()
        let urls: [URL]
    }

    private var saved: SavedServer? { hub.server(server) }
    private var selecting: Bool { editMode.isEditing }
    private var selectedEntries: [Entry] { (folder?.entries ?? []).filter { selection.contains($0.path) } }
    private var writable: Bool { account?.readonly == false }
    private var title: String { specialFolderLabel((path as NSString).lastPathComponent).isEmpty ? (saved?.name ?? "") : specialFolderLabel((path as NSString).lastPathComponent) }

    var body: some View {
        content
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { toolbar }
            .navigationBarBackButtonHidden(selecting)
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
            .fullScreenCover(isPresented: $scanning, onDismiss: { scanned = scanReady; scanReady = nil }) {
                DocumentScanner { pages in
                    if !pages.isEmpty { scanReady = ScannedPages(pages: pages) }
                    scanning = false
                }
                .ignoresSafeArea()
            }
            .sheet(item: $scanned) { scan in
                ScanSheet(scan: scan) { files in uploader.upload(hub, server: server, dir: path, files: files) }
            }
            .confirmationDialog(trashTitle, isPresented: Binding(get: { trashing != nil }, set: { if !$0 { trashing = nil } }), titleVisibility: .visible) {
                let items = trashing ?? []
                let forever = items.allSatisfy { inTrash($0.path) }
                Button(forever ? "Delete forever" : "Move to Trash", role: .destructive) {
                    trashing = nil
                    endSelection()
                    runner.run(forever ? "Deleting" : "Moving to the Trash", finished: forever ? "Deleted" : "Moved to the Trash:", items: items, hub: hub) { [server] entry in
                        try await hub.call(server) { try $0.trash(path: entry.path) }
                    }
                }
            } message: {
                Text((trashing ?? []).allSatisfy { inTrash($0.path) } ? "They are already in the Trash, so they are deleted for good." : "You can restore them from the Trash.")
            }
            .bulkRunner(runner)
            .sheet(item: $picking) { pick in
                FolderPicker(
                    server: server,
                    serverName: saved?.name ?? "",
                    start: path,
                    title: (pick.copy ? "Copy " : "Move ") + describe(pick.entries),
                    confirm: pick.copy ? "Copy here" : "Move here",
                    moving: pick.entries,
                    unavailable: pick.copy ? nil : path
                ) { dest in
                    moveOrCopy(pick.entries, to: dest, copy: pick.copy)
                }
            }
            .sheet(item: $sharingFiles) { shared in
                ShareSheet(items: shared.urls).presentationDetents([.medium, .large])
            }
            .confirmationDialog(choosingTitle, isPresented: Binding(get: { choosing != nil }, set: { if !$0 { choosing = nil } }), titleVisibility: .visible) {
                if let choice = choosing {
                    switch choice.kind {
                    case .compress:
                        ForEach([("zip", "ZIP"), ("tar.gz", "TAR.GZ"), ("tar.xz", "TAR.XZ"), ("tar", "TAR")], id: \.0) { format, label in
                            Button(label) { compress(choice.entries, format: format) }
                        }
                    case .convert:
                        ForEach([("jpeg", "JPEG"), ("png", "PNG"), ("webp", "WebP"), ("gif", "GIF"), ("bmp", "BMP"), ("tiff", "TIFF")], id: \.0) { format, label in
                            Button(label) {
                                runner.run("Converting to \(label)", finished: "Converted", items: choice.entries, hub: hub) { [server] entry in
                                    _ = try await hub.call(server) { try $0.convertImage(path: entry.path, format: format) }
                                }
                            }
                        }
                    case .protect:
                        Button("Only the folder itself") { protect(choice.entries, contents: false) }
                        Button("The folder and everything in it") { protect(choice.entries, contents: true) }
                    }
                }
            }
            .sheet(item: $resizing) { items in
                ResizeSheet(count: items.entries.count) { choice in
                    runner.run("Resizing", finished: choice.replace ? "Resized" : "Made smaller copies of", items: items.entries, hub: hub) { [server] entry in
                        _ = try await hub.call(server) { try $0.resizeImage(path: entry.path, percent: choice.percent, longest: choice.longest, replace: choice.replace) }
                    }
                }
            }
            .sheet(item: Binding(get: { versionsOf.map { Items(entries: [$0]) } }, set: { if $0 == nil { versionsOf = nil } })) { items in
                VersionsSheet(server: server, entry: items.entries[0], writable: writable) { done in runner.summary = done }
            }
            .sheet(item: $albumFor) { items in
                AlbumPicker(server: server, title: "Add \(describe(items.entries)) to…") { albumId in
                    let paths = items.entries.map(\.path)
                    runner.once("Adding to the album", hub: hub) { [server] in
                        _ = try await hub.call(server) { try $0.changeAlbum(albumId: albumId, paths: paths, add: true) }
                        return "Added \(describe(items.entries)) to the album"
                    }
                }
            }
            .alert("Extract pages", isPresented: Binding(get: { pagesOf != nil }, set: { if !$0 { pagesOf = nil } })) {
                TextField("Pages, such as 1,3,5-8", text: $pagesText).keyboardType(.numbersAndPunctuation)
                Button("One PDF") { extractPages(separate: false) }
                Button("One PDF per page") { extractPages(separate: true) }
                Button("Cancel", role: .cancel) { pagesOf = nil }
            } message: { Text(pagesOf?.name ?? "") }
            .alert("Rotate pages", isPresented: Binding(get: { rotating != nil }, set: { if !$0 { rotating = nil } })) {
                TextField("Pages (empty for all)", text: $pagesText).keyboardType(.numbersAndPunctuation)
                Button("Right") { rotate(90) }
                Button("Upside down") { rotate(180) }
                Button("Left") { rotate(270) }
                Button("Cancel", role: .cancel) { rotating = nil }
            } message: { Text("\(rotating?.name ?? "") is saved as a new PDF beside it.") }
            .confirmationDialog("Empty the Trash?", isPresented: $emptyingTrash, titleVisibility: .visible) {
                Button("Empty Trash", role: .destructive) {
                    runner.once("Emptying the Trash", hub: hub) { [server] in
                        try await hub.call(server) { try $0.emptyTrash() }
                        return "The Trash is empty"
                    }
                }
            } message: { Text("Everything in it is deleted for good. This cannot be undone.") }
            .safeAreaInset(edge: .bottom) {
                if let clip = hub.clipboard, clip.server == server, writable, !selecting {
                    HStack(spacing: 12) {
                        Image(systemName: "doc.on.clipboard")
                        Text((clip.cut ? "Move " : "Copy ") + describe(clip.entries)).lineLimit(1)
                        Spacer()
                        Button("Cancel") { hub.clipboard = nil }
                        Button("Paste here") { paste(clip) }.buttonStyle(.borderedProminent)
                    }
                    .padding().background(.bar)
                }
            }
            .environment(\.editMode, $editMode)
            .onChange(of: folder?.entries.map(\.path)) {
                let present = Set(folder?.entries.map(\.path) ?? [])
                selection.formIntersection(present)
            }
            .alert(message ?? "", isPresented: Binding(get: { message != nil }, set: { if !$0 { message = nil } })) { Button("OK", role: .cancel) {} }
    }

    @ViewBuilder private var content: some View {
        if let folder {
            List(selection: $selection) {
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
        .contextMenu { if !selecting { menu(entry) } }
        .swipeActions {
            if writable && !selecting && !isTrashFolder(entry) {
                Button("Trash", systemImage: "trash", role: .destructive) { trashing = [entry] }
                Button("Rename", systemImage: "pencil") { rename(entry) }.tint(.blue)
            }
        }
    }

    @ViewBuilder private func menu(_ entry: Entry) -> some View {
        Button("Select", systemImage: "checkmark.circle") {
            withAnimation { editMode = .active }
            // The list clears its selection as editing starts, so choose the row just after.
            Task { @MainActor in
                try? await Task.sleep(for: .milliseconds(350))
                selection = [entry.path]
            }
        }
        if entry.dir, account?.terminal == true, !inTrash(entry.path) {
            Button("Open terminal here", systemImage: "terminal") { openTerminal(entry.path) }
        }
        ForEach(bulkActions([entry], writable: writable), id: \.self) { action in
            Button(action.label([entry]), systemImage: action.systemImage, role: action.destructive ? .destructive : nil) { perform(action, on: [entry]) }
        }
    }

    private var trashTitle: String {
        let items = trashing ?? []
        let what = items.count == 1 ? "“\(items[0].name)”" : "\(items.count) items"
        return items.allSatisfy { inTrash($0.path) } ? "Delete \(what) forever?" : "Move \(what) to the Trash?"
    }

    private func endSelection() {
        selection = []
        withAnimation { editMode = .inactive }
    }

    private func perform(_ action: BulkAction, on items: [Entry]) {
        switch action {
        case .share:
            if items.count == 1, let one = items.first, !one.dir {
                opener.open(hub, server: server, entry: one, share: true)
            } else {
                Task {
                    let urls = await runner.fetch(items, server: server, hub: hub)
                    if !urls.isEmpty { sharingFiles = SharedFiles(urls: urls) }
                }
            }
        case .cut, .copy:
            hub.clipboard = Clip(server: server, entries: items, cut: action == .cut)
            endSelection()
        case .protect:
            if items.contains(where: \.dir) {
                choosing = Choosing(kind: .protect, entries: items)
            } else {
                protect(items, contents: false)
            }
        case .unprotect:
            endSelection()
            let paths = items.map(\.path)
            runner.once("Allowing deletion", hub: hub) { [server] in
                try await hub.call(server) { try $0.setProtection(paths: paths, protect: false, contents: false) }
                return "\(describe(items)) can be deleted again"
            }
        case .versions:
            if let one = items.first { endSelection(); versionsOf = one }
        case .convertImage: choosing = Choosing(kind: .convert, entries: items)
        case .resize: resizing = Items(entries: items)
        case .joinPdfs:
            endSelection()
            let paths = items.map(\.path)
            runner.once("Joining \(items.count) PDFs", hub: hub) { [server] in
                let made = try await hub.call(server) { try $0.pdfMerge(paths: paths) }
                return "Made \((made as NSString).lastPathComponent)"
            }
        case .pdfExtract:
            if let one = items.first { endSelection(); pagesText = ""; pagesOf = one }
        case .pdfSplit:
            if let one = items.first {
                endSelection()
                runner.once("Splitting \(one.name)", hub: hub) { [server] in
                    let made = try await hub.call(server) { try $0.pdfSplit(path: one.path) }
                    return "Made \(made.count) PDFs"
                }
            }
        case .pdfRotate:
            if let one = items.first { endSelection(); pagesText = ""; rotating = one }
        case .addToAlbum: albumFor = Items(entries: items)
        case .extractHere:
            endSelection()
            runner.run("Extracting", finished: "Extracted", items: items, hub: hub) { [server] entry in
                _ = try await hub.call(server) { try $0.extract(path: entry.path, here: true) }
            }
        case .moveTo: picking = Picking(entries: items, copy: false)
        case .copyTo: picking = Picking(entries: items, copy: true)
        case .rename:
            if let one = items.first { endSelection(); rename(one) }
        case .duplicate:
            endSelection()
            runner.run("Duplicating", finished: "Duplicated", items: items, hub: hub) { [server] entry in
                _ = try await hub.call(server) { try $0.duplicate(path: entry.path) }
            }
        case .tag:
            prompt = NamePrompt(title: items.count == 1 ? "Tag" : "Tag \(items.count) items", confirm: "Add tag") { text in
                let tag = text.hasPrefix("#") ? String(text.dropFirst()) : text
                guard !tag.isEmpty else { return }
                endSelection()
                runner.run("Tagging #\(tag)", finished: "Tagged #\(tag):", items: items, hub: hub) { [server] entry in
                    _ = try await hub.call(server) { try $0.addTag(path: entry.path, tag: tag) }
                }
            }
        case .compress: choosing = Choosing(kind: .compress, entries: items)
        case .extract:
            endSelection()
            runner.run("Extracting", finished: "Extracted", items: items, hub: hub) { [server] entry in
                _ = try await hub.call(server) { try $0.extract(path: entry.path, here: false) }
            }
        case .newFolderWith:
            prompt = NamePrompt(title: "New folder with \(describe(items))", confirm: "Create") { name in
                endSelection()
                Task {
                    do {
                        let parent = path
                        let made = try await hub.call(server) { try $0.createFolder(parent: parent, name: name) }
                        moveOrCopy(items, to: made, copy: false, label: name)
                    } catch {
                        message = userMessage(error)
                    }
                }
            }
        case .restore:
            endSelection()
            runner.run("Restoring", finished: "Restored", items: items, hub: hub) { [server] entry in
                _ = try await hub.call(server) { try $0.restore(path: entry.path) }
            }
        case .trash:
            trashing = items
        }
    }

    private var choosingTitle: String {
        guard let choice = choosing else { return "" }
        switch choice.kind {
        case .compress: return "Compress \(describe(choice.entries)) to"
        case .convert: return "Convert \(describe(choice.entries)) to"
        case .protect: return "Protect from deletion"
        }
    }

    private func compress(_ items: [Entry], format: String) {
        endSelection()
        let paths = items.map(\.path)
        runner.once("Compressing \(describe(items))", hub: hub) { [server] in
            let made = try await hub.call(server) { try $0.compress(paths: paths, format: format) }
            return "Made \((made as NSString).lastPathComponent)"
        }
    }

    private func protect(_ items: [Entry], contents: Bool) {
        endSelection()
        let paths = items.map(\.path)
        runner.once("Protecting \(describe(items))", hub: hub) { [server] in
            try await hub.call(server) { try $0.setProtection(paths: paths, protect: true, contents: contents) }
            return "\(describe(items)) protected from deletion"
        }
    }

    private func extractPages(separate: Bool) {
        guard let entry = pagesOf else { return }
        pagesOf = nil
        let pages = pagesText.replacingOccurrences(of: " ", with: "")
        guard !pages.isEmpty else { return }
        runner.once("Extracting pages", hub: hub) { [server] in
            let made = try await hub.call(server) { try $0.pdfExtract(path: entry.path, pages: pages, separate: separate) }
            return made.count == 1 ? "Made \((made[0] as NSString).lastPathComponent)" : "Made \(made.count) PDFs"
        }
    }

    private func rotate(_ degrees: Int64) {
        guard let entry = rotating else { return }
        rotating = nil
        let pages = pagesText.replacingOccurrences(of: " ", with: "")
        runner.once("Rotating pages", hub: hub) { [server] in
            let made = try await hub.call(server) { try $0.pdfRotate(path: entry.path, pages: pages, degrees: degrees) }
            return "Made \((made as NSString).lastPathComponent)"
        }
    }

    private func paste(_ clip: Clip) {
        if clip.cut { hub.clipboard = nil }
        let dest = path
        if clip.cut {
            runner.run("Moving here", finished: "Moved here:", items: clip.entries, hub: hub) { [server] entry in
                _ = try await hub.call(server) { try $0.moveTo(path: entry.path, dest: dest) }
            }
        } else {
            runner.run("Copying here", finished: "Copied here:", items: clip.entries, hub: hub) { [server] entry in
                _ = try await hub.call(server) { try $0.copyTo(path: entry.path, dest: dest) }
            }
        }
    }

    private func moveOrCopy(_ items: [Entry], to dest: String, copy: Bool, label: String? = nil) {
        endSelection()
        let place = label ?? (dest.isEmpty ? (saved?.name ?? "") : specialFolderLabel((dest as NSString).lastPathComponent))
        if copy {
            runner.run("Copying to \(place)", finished: "Copied to \(place):", items: items, hub: hub) { [server] entry in
                _ = try await hub.call(server) { try $0.copyTo(path: entry.path, dest: dest) }
            }
        } else {
            runner.run("Moving to \(place)", finished: "Moved to \(place):", items: items, hub: hub) { [server] entry in
                _ = try await hub.call(server) { try $0.moveTo(path: entry.path, dest: dest) }
            }
        }
    }

    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        if selecting {
            ToolbarItem(placement: .topBarLeading) {
                let all = folder?.entries.map(\.path) ?? []
                if selection.count < all.count {
                    Button("Select All") { selection = Set(all) }
                } else {
                    Button("Deselect All") { selection = [] }
                }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Done") { endSelection() }
            }
            ToolbarItemGroup(placement: .bottomBar) {
                let items = selectedEntries
                let actions = bulkActions(items, writable: writable)
                let primary = [BulkAction.share, .moveTo, .copyTo, .restore, .trash].filter { actions.contains($0) }.prefix(4)
                ForEach(Array(primary), id: \.self) { action in
                    Button(action.label(items), systemImage: action.systemImage, role: action.destructive ? .destructive : nil) { perform(action, on: items) }
                        .disabled(items.isEmpty)
                }
                let more = actions.filter { !primary.contains($0) }
                if !more.isEmpty {
                    Menu {
                        ForEach(more, id: \.self) { action in
                            Button(action.label(items), systemImage: action.systemImage, role: action.destructive ? .destructive : nil) { perform(action, on: items) }
                        }
                    } label: {
                        Label("More", systemImage: "ellipsis.circle")
                    }
                    .disabled(items.isEmpty)
                }
            }
        } else {
            normalToolbar
        }
    }

    @ToolbarContentBuilder private var normalToolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            if writable {
                Menu {
                    Button("Photos and videos", systemImage: "photo.on.rectangle") { choosingPhotos = true }
                    Button("Files", systemImage: "doc") { choosingFiles = true }
                    if DocumentScanner.available {
                        Button("Scan documents", systemImage: "doc.viewfinder") { scanning = true }
                    }
                    Button("New folder", systemImage: "folder.badge.plus") {
                        prompt = NamePrompt(title: "New folder", confirm: "Create") { name in
                            act { _ = try await hub.call(server) { try $0.createFolder(parent: path, name: name) } }
                        }
                    }
                } label: { Image(systemName: "plus") }
            }
            Menu {
                if !(folder?.entries.isEmpty ?? true) {
                    Button("Select", systemImage: "checkmark.circle") { withAnimation { editMode = .active } }
                }
                if writable, path == ".ownnas-trash" || path == "Trash" {
                    Button("Empty Trash", systemImage: "trash.slash", role: .destructive) { emptyingTrash = true }
                }
                Button("Search", systemImage: "magnifyingglass") { hub.path.append(.search(server: server, path: path)) }
                Button("Gallery", systemImage: "photo.on.rectangle.angled") { hub.path.append(.gallery(server: server)) }
                Button("Photo backup", systemImage: "photo.badge.arrow.down") { hub.path.append(.backup) }
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
