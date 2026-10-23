import PhotosUI
import QuickLook
import SwiftUI
import UniformTypeIdentifiers

/// Fetches files to preview (Quick Look) or share, with a progress sheet that can cancel.
@MainActor
final class Opener: ObservableObject {
    struct Fetch: Identifiable {
        let id = UUID()
        let name: String
        let size: UInt64
        let share: Bool
        let tracker: Tracker
    }

    @Published var fetching: Fetch?
    @Published var done: UInt64 = 0
    @Published var preview: URL?
    @Published var sharing: ShareItem?
    @Published var problem: String?

    func open(_ hub: Hub, server: String, entry: Entry, share: Bool = false) {
        var job: Fetch!
        let tracker = Tracker { [weak self] done in
            Task { @MainActor in if self?.fetching?.id == job.id { self?.done = done } }
        }
        job = Fetch(name: entry.name, size: entry.size, share: share, tracker: tracker)
        fetching = job
        done = 0
        Task {
            do {
                let file = try await hub.cachedCopy(server, path: entry.path, size: entry.size, modified: entry.modified, transfer: tracker)
                guard fetching?.id == job.id else { return }
                fetching = nil
                if share { sharing = ShareItem(url: file) } else { preview = file }
            } catch {
                if fetching?.id == job.id { fetching = nil }
                if !isCancel(error) { problem = userMessage(error) }
            }
        }
    }

    func cancel() {
        fetching?.tracker.cancel()
        fetching = nil
    }
}

struct ShareItem: Identifiable {
    let id = UUID()
    let url: URL
}

/// The system share sheet.
struct ShareSheet: UIViewControllerRepresentable {
    let items: [Any]
    func makeUIViewController(context: Context) -> UIActivityViewController { UIActivityViewController(activityItems: items, applicationActivities: nil) }
    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}

extension View {
    /// The opener's progress, Quick Look preview, share sheet, and errors.
    func opener(_ opener: Opener) -> some View {
        self
            .quickLookPreview(Binding(get: { opener.preview }, set: { opener.preview = $0 }))
            .sheet(item: Binding(get: { opener.sharing }, set: { opener.sharing = $0 })) { item in
                ShareSheet(items: [item.url]).presentationDetents([.medium, .large])
            }
            .sheet(item: Binding(get: { opener.fetching }, set: { if $0 == nil { opener.cancel() } })) { job in
                FetchSheet(job: job, opener: opener).presentationDetents([.height(200)]).interactiveDismissDisabled()
            }
            .alert("Could not open the file", isPresented: Binding(get: { opener.problem != nil }, set: { if !$0 { opener.problem = nil } })) {
                Button("OK", role: .cancel) {}
            } message: { Text(opener.problem ?? "") }
    }
}

private struct FetchSheet: View {
    let job: Opener.Fetch
    @ObservedObject var opener: Opener

    var body: some View {
        VStack(spacing: 14) {
            Text(job.share ? "Preparing \(job.name)…" : "Opening \(job.name)…").font(.headline).lineLimit(1)
            ProgressView(value: job.size > 0 ? min(Double(opener.done) / Double(job.size), 1) : 0)
            Text("\(bytes(opener.done)) of \(bytes(job.size))").font(.caption).foregroundStyle(.secondary)
            Button("Cancel", role: .cancel) { opener.cancel() }
        }
        .padding(24)
    }
}

func bytes(_ count: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(clamping: count), countStyle: .file)
}

// MARK: Uploads

/// Uploads files into a folder one at a time, asking what to do when a name is taken, as the
/// web app does.
@MainActor
final class Uploader: ObservableObject {
    struct Pending {
        let name: String
        let file: URL
        /// Copied into the app's temporary folder (deleted afterwards), or a picked document.
        let temporary: Bool
    }

    struct Status {
        var name: String
        var index: Int
        var total: Int
        var done: UInt64
        var size: UInt64
    }

    struct Question: Identifiable {
        let id = UUID()
        let name: String
        let existing: Existing
        let answer: (Conflict?) -> Void
    }

    @Published var status: Status?
    @Published var question: Question?
    @Published var summary: String?
    private var tracker: Tracker?

    func upload(_ hub: Hub, server: String, dir: String, files: [Pending]) {
        guard !files.isEmpty, status == nil else { return }
        Task {
            var saved = 0, skipped = 0
            var failures: [String] = []
            for (index, item) in files.enumerated() {
                let size = (try? FileManager.default.attributesOfItem(atPath: item.file.path)[.size] as? NSNumber)?.uint64Value ?? 0
                status = Status(name: item.name, index: index + 1, total: files.count, done: 0, size: size)
                do {
                    var choice: Conflict?
                    while true {
                        let outcome = try await send(hub, server: server, dir: dir, item: item, size: size, conflict: choice)
                        if case let .exists(existing) = outcome {
                            guard let answer = await ask(item.name, existing) else { skipped += 1; break }
                            choice = answer
                            continue
                        }
                        if case .skipped = outcome { skipped += 1 } else { saved += 1 }
                        break
                    }
                } catch {
                    if isCancel(error) { break }
                    failures.append("\(item.name): \(userMessage(error))")
                }
                if item.temporary { try? FileManager.default.removeItem(at: item.file) }
            }
            status = nil
            hub.changed()
            var parts: [String] = []
            if saved > 0 { parts.append("Uploaded \(saved)") }
            if skipped > 0 { parts.append("skipped \(skipped)") }
            if let first = failures.first { parts.append(failures.count == 1 ? first : "\(failures.count) failed (\(first))") }
            summary = parts.joined(separator: ", ")
        }
    }

    func cancel() { tracker?.cancel() }

    private func send(_ hub: Hub, server: String, dir: String, item: Pending, size: UInt64, conflict: Conflict?) async throws -> UploadResult {
        let tracker = Tracker { [weak self] done in Task { @MainActor in self?.status?.done = done } }
        self.tracker = tracker
        let scoped = item.temporary ? false : item.file.startAccessingSecurityScopedResource()
        defer { if scoped { item.file.stopAccessingSecurityScopedResource() } }
        let modified = (try? FileManager.default.attributesOfItem(atPath: item.file.path)[.modificationDate] as? Date).map { Int64($0.timeIntervalSince1970 * 1000) }
        let fd = open(item.file.path, O_RDONLY)
        guard fd >= 0 else { throw ClientError.Failed(reason: "The file could not be read") }
        // The call takes the descriptor over and closes it.
        return try await hub.call(server) { try $0.uploadFd(dir: dir, name: item.name, fd: fd, size: size, mtimeMs: modified, conflict: conflict, transfer: tracker) }
    }

    private func ask(_ name: String, _ existing: Existing) async -> Conflict? {
        await withCheckedContinuation { continuation in
            question = Question(name: name, existing: existing) { [weak self] answer in
                self?.question = nil
                continuation.resume(returning: answer)
            }
        }
    }
}

/// Picked photos and videos, copied out of the photo library with their file names.
struct PickedFile: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .item) { received in
            let folder = FileManager.default.temporaryDirectory.appendingPathComponent("uploads/\(UUID().uuidString)")
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let copy = folder.appendingPathComponent(received.file.lastPathComponent)
            try FileManager.default.copyItem(at: received.file, to: copy)
            return PickedFile(url: copy)
        }
    }
}

extension View {
    /// The uploader's progress bar, conflict question, and summary.
    func uploads(_ uploader: Uploader) -> some View {
        self
            .safeAreaInset(edge: .bottom) {
                if let status = uploader.status {
                    HStack(spacing: 12) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Uploading \(status.name) (\(status.index) of \(status.total))").font(.footnote).lineLimit(1)
                            ProgressView(value: status.size > 0 ? min(Double(status.done) / Double(status.size), 1) : 0)
                        }
                        Button("Cancel") { uploader.cancel() }
                    }
                    .padding().background(.bar)
                }
            }
            .confirmationDialog(
                "“\(uploader.question?.name ?? "")” is already here",
                isPresented: Binding(get: { uploader.question != nil }, set: { if !$0 { uploader.question?.answer(nil) } }),
                titleVisibility: .visible
            ) {
                if let question = uploader.question {
                    Button("Replace it") { question.answer(.overwrite) }
                    Button("Keep both") { question.answer(.keepBoth) }
                    Button("Keep the newer one") { question.answer(.archiveOlder) }
                    Button("Archive the one on the server") { question.answer(.archiveExisting) }
                    Button("Skip", role: .cancel) { question.answer(nil) }
                }
            } message: {
                if let existing = uploader.question?.existing {
                    Text("The one on the server is \(bytes(existing.size)), changed \(Date(timeIntervalSince1970: Double(existing.modified) / 1000).formatted(date: .abbreviated, time: .shortened)).")
                }
            }
            .alert(uploader.summary ?? "", isPresented: Binding(get: { !(uploader.summary ?? "").isEmpty }, set: { if !$0 { uploader.summary = nil } })) {
                Button("OK", role: .cancel) {}
            }
    }
}

/// Asks for a name (new folder, rename, album, tag…).
struct NamePrompt: Identifiable {
    let id = UUID()
    let title: String
    var initial = ""
    let confirm: String
    let action: (String) -> Void
}

extension View {
    func namePrompt(_ prompt: Binding<NamePrompt?>) -> some View {
        modifier(NamePromptModifier(prompt: prompt))
    }
}

private struct NamePromptModifier: ViewModifier {
    @Binding var prompt: NamePrompt?
    @State private var text = ""

    func body(content: Content) -> some View {
        content
            .alert(prompt?.title ?? "", isPresented: Binding(get: { prompt != nil }, set: { if !$0 { prompt = nil } })) {
                TextField("Name", text: $text)
                Button(prompt?.confirm ?? "OK") {
                    let name = text.trimmingCharacters(in: .whitespacesAndNewlines)
                    let action = prompt?.action
                    prompt = nil
                    if !name.isEmpty { action?(name) }
                }
                Button("Cancel", role: .cancel) { prompt = nil }
            }
            .onChange(of: prompt?.id) { text = prompt?.initial ?? "" }
    }
}

/// The SF Symbol for a file kind, as the web app's icons.
func symbol(for entry: Entry) -> String {
    if entry.kind == "trash-folder" || entry.path == ".ownnas-trash" { return "trash" }
    if entry.kind == "archive-folder" || entry.path == ".ownnas-archive" { return "archivebox" }
    if entry.dir { return "folder.fill" }
    switch entry.kind {
    case "image", "svg": return "photo"
    case "video": return "film"
    case "audio": return "music.note"
    case "pdf": return "doc.richtext"
    case "text", "spreadsheet": return "doc.text"
    case "archive": return "doc.zipper"
    default: return "doc"
    }
}

/// The web app's names for OwnNAS's own folders.
func specialFolderLabel(_ name: String) -> String {
    switch name {
    case ".ownnas-trash", "Trash": return "Trash"
    case ".ownnas-archive", "Archive": return "Archive"
    default: return name
    }
}

func inTrash(_ path: String) -> Bool {
    path == ".ownnas-trash" || path.hasPrefix(".ownnas-trash/") || path == "Trash" || path.hasPrefix("Trash/")
}

/// A file's thumbnail, or its kind's symbol.
struct EntryIcon: View {
    let server: String
    let entry: Entry
    var size: CGFloat = 44

    var body: some View {
        Group {
            if entry.thumb {
                ServerImage(server: server, path: entry.path, key: "\(server):\(entry.path):\(entry.modified)") { symbolView }
            } else {
                symbolView
            }
        }
        .frame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    private var symbolView: some View {
        Image(systemName: symbol(for: entry))
            .font(.system(size: size * 0.55))
            .foregroundStyle(entry.dir ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
            .frame(width: size, height: size)
    }
}
