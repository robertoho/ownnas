import SwiftUI

/// What can be done to one or more selected files and folders, as in the web app's menu.
enum BulkAction: CaseIterable {
    case share, moveTo, copyTo, cut, copy, rename, duplicate, tag, protect, unprotect, versions
    case convertImage, resize, joinPdfs, pdfExtract, pdfSplit, pdfRotate, addToAlbum
    case compress, extract, extractHere, newFolderWith, restore, trash

    func label(_ entries: [Entry]) -> String {
        switch self {
        case .share: return "Share…"
        case .moveTo: return "Move to…"
        case .copyTo: return "Copy to…"
        case .cut: return "Cut"
        case .copy: return "Copy"
        case .rename: return "Rename"
        case .duplicate: return "Duplicate"
        case .tag: return "Tag…"
        case .protect: return "Protect from deletion"
        case .unprotect: return "Allow deletion"
        case .versions: return "Versions…"
        case .convertImage: return "Convert to…"
        case .resize: return "Resize…"
        case .joinPdfs: return "Join PDFs"
        case .pdfExtract: return "Extract pages…"
        case .pdfSplit: return "Split into pages"
        case .pdfRotate: return "Rotate pages…"
        case .addToAlbum: return "Add to album…"
        case .compress: return "Compress to…"
        case .extract: return "Extract"
        case .extractHere: return "Extract here"
        case .newFolderWith: return entries.count == 1 ? "New folder with this" : "New folder with selection"
        case .restore: return "Restore"
        case .trash: return entries.allSatisfy { inTrash($0.path) } ? "Delete permanently" : "Move to Trash"
        }
    }

    var systemImage: String {
        switch self {
        case .share: return "square.and.arrow.up"
        case .moveTo: return "folder"
        case .copyTo: return "doc.on.doc"
        case .cut: return "scissors"
        case .copy: return "doc.on.clipboard"
        case .rename: return "pencil"
        case .duplicate: return "plus.square.on.square"
        case .tag: return "tag"
        case .protect: return "lock"
        case .unprotect: return "lock.open"
        case .versions: return "clock.arrow.circlepath"
        case .convertImage: return "photo.badge.arrow.down"
        case .resize: return "arrow.down.right.and.arrow.up.left"
        case .joinPdfs: return "doc.on.doc.fill"
        case .pdfExtract: return "doc.text.below.ecg"
        case .pdfSplit: return "square.split.2x1"
        case .pdfRotate: return "rotate.right"
        case .addToAlbum: return "rectangle.stack.badge.plus"
        case .compress: return "doc.zipper"
        case .extract, .extractHere: return "arrow.up.bin"
        case .newFolderWith: return "folder.badge.plus"
        case .restore: return "arrow.uturn.backward"
        case .trash: return "trash"
        }
    }

    var destructive: Bool { self == .trash }
}

/// OwnNAS's own Trash folder, which cannot be moved, copied, or renamed.
func isTrashFolder(_ entry: Entry) -> Bool {
    entry.kind == "trash-folder" || entry.path == ".ownnas-trash" || (entry.path == "Trash" && entry.dir)
}

/// Photos the server can resize (as the web app's list).
private func resizable(_ entry: Entry) -> Bool {
    ["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff"].contains((entry.name as NSString).pathExtension.lowercased())
}

/// The actions that fit `entries`, in menu order.
func bulkActions(_ entries: [Entry], writable: Bool) -> [BulkAction] {
    guard !entries.isEmpty, !entries.contains(where: isTrashFolder) else { return [] }
    let one = entries.count == 1 ? entries[0] : nil
    var actions: [BulkAction] = []
    if entries.contains(where: { !$0.dir }) { actions.append(.share) }
    guard writable else {
        if let one, !one.dir { actions.append(.versions) }
        return actions
    }
    if entries.allSatisfy({ inTrash($0.path) }) { return actions + [.restore, .trash] }
    actions += [.moveTo, .copyTo, .cut, .copy]
    if one != nil { actions.append(.rename) }
    actions += [.duplicate, .tag]
    actions.append(entries.allSatisfy { $0.protected == "self" || $0.protected == "contents" } ? .unprotect : .protect)
    if let one, !one.dir { actions.append(.versions) }
    if entries.allSatisfy({ !$0.dir && $0.kind == "image" }) {
        actions.append(.convertImage)
        if entries.allSatisfy(resizable) { actions.append(.resize) }
    }
    if entries.allSatisfy({ !$0.dir && $0.kind == "pdf" }) {
        if entries.count >= 2 { actions.append(.joinPdfs) }
        if one != nil { actions += [.pdfExtract, .pdfSplit, .pdfRotate] }
    }
    if entries.allSatisfy({ !$0.dir && ($0.kind == "image" || $0.kind == "video") }) { actions.append(.addToAlbum) }
    actions.append(.compress)
    if let one, !one.dir, one.kind == "archive" { actions += [.extract, .extractHere] }
    actions += [.newFolderWith, .trash]
    return actions
}

/// "3 items", or the name when there is one.
func describe(_ entries: [Entry]) -> String {
    entries.count == 1 ? entries[0].name : "\(entries.count) items"
}

/// Runs an action over several items one at a time, with progress, and says how it went.
@MainActor
final class BulkRunner: ObservableObject {
    struct Job: Identifiable {
        let id = UUID()
        let title: String
        let total: Int
    }

    @Published var job: Job?
    @Published var done = 0
    @Published var current = ""
    @Published var summary: String?
    private var stopped = false
    private var tracker: Tracker?

    var running: Bool { job != nil }

    /// `finished` is the past tense for the summary, such as "Moved to Docs:".
    func run(_ title: String, finished: String, items: [Entry], hub: Hub, op: @escaping (Entry) async throws -> Void) {
        guard !items.isEmpty else { return }
        job = Job(title: title, total: items.count)
        done = 0
        stopped = false
        Task {
            var ok = 0
            var failures: [String] = []
            for item in items {
                if stopped { break }
                current = item.name
                do {
                    try await op(item)
                    ok += 1
                } catch {
                    failures.append("\(item.name): \(userMessage(error))")
                }
                done += 1
            }
            job = nil
            hub.changed()
            var parts: [String] = []
            if ok > 0 { parts.append("\(finished) \(items.count == 1 && ok == 1 ? items[0].name : (ok == 1 ? "1 item" : "\(ok) items"))") }
            let skipped = items.count - ok - failures.count
            if skipped > 0 { parts.append("stopped before \(skipped)") }
            if let first = failures.first { parts.append(failures.count == 1 ? first : "\(failures.count) failed (\(first))") }
            summary = parts.joined(separator: ". ")
        }
    }

    /// Runs one step with a spinner; `op` returns what to say when it is done.
    func once(_ title: String, hub: Hub, op: @escaping () async throws -> String) {
        job = Job(title: title, total: 1)
        current = ""
        Task {
            do {
                summary = try await op()
            } catch {
                summary = userMessage(error)
            }
            job = nil
            hub.changed()
        }
    }

    /// Downloads the files among `items` for the share sheet.
    func fetch(_ items: [Entry], server: String, hub: Hub) async -> [URL] {
        let files = items.filter { !$0.dir }
        job = Job(title: "Preparing to share", total: files.count)
        done = 0
        stopped = false
        defer { job = nil; tracker = nil }
        var urls: [URL] = []
        for item in files {
            if stopped { return [] }
            current = item.name
            let tracker = Tracker { _ in }
            self.tracker = tracker
            do {
                urls.append(try await hub.cachedCopy(server, path: item.path, size: item.size, modified: item.modified, transfer: tracker))
            } catch {
                if !isCancel(error) { summary = "\(item.name): \(userMessage(error))" }
                return []
            }
            done += 1
        }
        return urls
    }

    func stop() {
        stopped = true
        tracker?.cancel()
    }
}

private struct BulkProgressSheet: View {
    let job: BulkRunner.Job
    @ObservedObject var runner: BulkRunner

    var body: some View {
        VStack(spacing: 14) {
            Text(job.title).font(.headline).lineLimit(1)
            Text(runner.current).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
            if job.total > 1 {
                ProgressView(value: Double(runner.done), total: Double(job.total))
                Text("\(runner.done) of \(job.total)").font(.caption).foregroundStyle(.secondary)
            } else {
                ProgressView()
            }
            if job.total > 1 || job.title == "Preparing to share" {
                Button("Stop", role: .cancel) { runner.stop() }
            }
        }
        .padding(24)
    }
}

extension View {
    /// The runner's progress sheet and summary.
    func bulkRunner(_ runner: BulkRunner) -> some View {
        self
            // An overlay rather than a sheet: a sheet cannot open while the folder picker is
            // still closing, and its closing would swallow the summary alert.
            .overlay {
                if let job = runner.job {
                    ZStack {
                        Color.black.opacity(0.25).ignoresSafeArea()
                        BulkProgressSheet(job: job, runner: runner)
                            .frame(maxWidth: 340)
                            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 20))
                            .padding(32)
                    }
                    .transition(.opacity)
                }
            }
            .animation(.default, value: runner.job?.id)
            .alert(runner.summary ?? "", isPresented: Binding(get: { !(runner.summary ?? "").isEmpty }, set: { if !$0 { runner.summary = nil } })) {
                Button("OK", role: .cancel) {}
            }
    }
}

/// Where a move or copy goes: browses the server's folders from `start`, can make a new one,
/// and does not offer the moving folders themselves or anything inside them.
struct FolderPicker: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let serverName: String
    let start: String
    let title: String
    let confirm: String
    let moving: [Entry]
    /// A folder that cannot be chosen, such as where the items already are for a move.
    let unavailable: String?
    let onPick: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var at: String
    @State private var folder: Folder?
    @State private var problem: String?
    @State private var prompt: NamePrompt?

    init(server: String, serverName: String, start: String, title: String, confirm: String, moving: [Entry], unavailable: String?, onPick: @escaping (String) -> Void) {
        self.server = server
        self.serverName = serverName
        self.start = start
        self.title = title
        self.confirm = confirm
        self.moving = moving
        self.unavailable = unavailable
        self.onPick = onPick
        _at = State(initialValue: start)
    }

    private func blocked(_ path: String) -> Bool {
        moving.filter(\.dir).contains { path == $0.path || path.hasPrefix($0.path + "/") }
    }

    private var location: String {
        at.isEmpty ? serverName : serverName + "/" + at.split(separator: "/").map { specialFolderLabel(String($0)) }.joined(separator: "/")
    }

    var body: some View {
        NavigationStack {
            Group {
                if let problem {
                    ContentUnavailableView(problem, systemImage: "exclamationmark.triangle")
                } else if let folder {
                    let dirs = folder.entries.filter { $0.dir && !isTrashFolder($0) }
                        .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
                    List {
                        if !at.isEmpty {
                            Button {
                                at = (at as NSString).deletingLastPathComponent
                            } label: {
                                Label("Up a folder", systemImage: "arrow.turn.left.up")
                            }
                        }
                        ForEach(dirs, id: \.path) { dir in
                            let off = blocked(dir.path)
                            Button {
                                at = dir.path
                            } label: {
                                HStack(spacing: 12) {
                                    Image(systemName: "folder.fill").foregroundStyle(off ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.tint))
                                    VStack(alignment: .leading) {
                                        Text(specialFolderLabel(dir.name)).foregroundStyle(off ? .secondary : .primary)
                                        if off { Text("Being moved").font(.caption).foregroundStyle(.secondary) }
                                    }
                                }
                            }
                            .disabled(off)
                        }
                        if dirs.isEmpty {
                            Text("No folders here").foregroundStyle(.secondary)
                        }
                    }
                    .listStyle(.plain)
                } else {
                    ProgressView()
                }
            }
            .safeAreaInset(edge: .top) {
                Text(location).font(.footnote).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal).padding(.vertical, 6).background(.bar)
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(confirm) {
                        dismiss()
                        onPick(at)
                    }
                    .disabled(folder == nil || at == unavailable || blocked(at))
                }
                ToolbarItem(placement: .bottomBar) {
                    Button("New folder", systemImage: "folder.badge.plus") {
                        prompt = NamePrompt(title: "New folder", confirm: "Create") { name in
                            Task {
                                do {
                                    let parent = at
                                    at = try await hub.call(server) { try $0.createFolder(parent: parent, name: name) }
                                } catch {
                                    problem = userMessage(error)
                                }
                            }
                        }
                    }
                }
            }
            .namePrompt($prompt)
            .task(id: at) {
                folder = nil
                problem = nil
                do {
                    let path = at
                    folder = try await hub.call(server) { try $0.list(path: path) }
                } catch {
                    problem = userMessage(error)
                }
            }
        }
    }
}

/// How much smaller to make photos: a size for the long side, or a share.
struct ResizeChoice {
    var percent: UInt32?
    var longest: UInt32?
    var replace = false
}

/// Picks a size for photos and whether the smaller ones replace the originals.
struct ResizeSheet: View {
    let count: Int
    let resize: (ResizeChoice) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var preset = 1
    @State private var replace = false

    private let presets: [(String, ResizeChoice)] = [
        ("Small · 1280", ResizeChoice(longest: 1280)),
        ("Medium · 1920", ResizeChoice(longest: 1920)),
        ("Large · 2560", ResizeChoice(longest: 2560)),
        ("4K · 3840", ResizeChoice(longest: 3840)),
        ("Half · 50%", ResizeChoice(percent: 50)),
        ("Quarter · 25%", ResizeChoice(percent: 25)),
    ]

    var body: some View {
        NavigationStack {
            Form {
                Picker("Size", selection: $preset) {
                    ForEach(presets.indices, id: \.self) { Text(presets[$0].0).tag($0) }
                }
                .pickerStyle(.inline)
                Section {
                    Toggle("Replace the originals", isOn: $replace)
                } footer: {
                    Text(replace ? "The originals go to the Trash." : "Smaller copies are saved beside them. Photos already that small are left as they are.")
                }
            }
            .navigationTitle(count == 1 ? "Resize photo" : "Resize \(count) photos")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Resize") {
                        var choice = presets[preset].1
                        choice.replace = replace
                        dismiss()
                        resize(choice)
                    }
                }
            }
        }
    }
}

/// Earlier versions of a file: put one back, or delete it.
struct VersionsSheet: View {
    @EnvironmentObject private var hub: Hub
    @Environment(\.dismiss) private var dismiss
    let server: String
    let entry: Entry
    let writable: Bool
    let done: (String) -> Void
    @State private var versions: [FileVersion]?
    @State private var problem: String?
    @State private var deleting: FileVersion?

    var body: some View {
        NavigationStack {
            Group {
                if let problem {
                    ContentUnavailableView(problem, systemImage: "exclamationmark.triangle")
                } else if let versions {
                    if versions.isEmpty {
                        ContentUnavailableView("No earlier versions", systemImage: "clock", description: Text("A file gets one each time it is replaced or edited."))
                    } else {
                        List(versions, id: \.id) { version in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(Date(timeIntervalSince1970: Double(version.modified) / 1000).formatted(date: .abbreviated, time: .shortened))
                                Text("\(bytes(version.size)) · \(reason(version))\(version.by.isEmpty ? "" : " by \(version.by)")")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            .swipeActions {
                                if writable {
                                    Button("Delete", systemImage: "trash", role: .destructive) { deleting = version }
                                    Button("Restore", systemImage: "arrow.uturn.backward") { restore(version) }.tint(.blue)
                                }
                            }
                            .contextMenu {
                                if writable {
                                    Button("Restore this version", systemImage: "arrow.uturn.backward") { restore(version) }
                                    Button("Delete this version", systemImage: "trash", role: .destructive) { deleting = version }
                                }
                            }
                        }
                    }
                } else {
                    ProgressView()
                }
            }
            .navigationTitle("Versions of \(entry.name)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .task { await load() }
            .confirmationDialog("Delete this version?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible) {
                Button("Delete", role: .destructive) {
                    if let version = deleting {
                        Task {
                            do {
                                try await hub.call(server) { try $0.deleteVersion(id: version.id) }
                                await load()
                            } catch {
                                problem = userMessage(error)
                            }
                        }
                    }
                    deleting = nil
                }
            } message: { Text("It cannot be brought back.") }
        }
    }

    private func reason(_ version: FileVersion) -> String {
        switch version.reason {
        case "edit": return "edited"
        case "upload": return "replaced by an upload"
        case "sync": return "replaced by sync"
        case "restore": return "replaced by a restore"
        default: return version.reason
        }
    }

    private func load() async {
        do {
            let path = entry.path
            versions = try await hub.call(server) { try $0.versions(path: path) }
        } catch {
            problem = userMessage(error)
        }
    }

    private func restore(_ version: FileVersion) {
        Task {
            do {
                try await hub.call(server) { try $0.restoreVersion(id: version.id) }
                hub.changed()
                dismiss()
                done("Put back the version from \(Date(timeIntervalSince1970: Double(version.modified) / 1000).formatted(.relative(presentation: .named)))")
            } catch {
                problem = userMessage(error)
            }
        }
    }
}
