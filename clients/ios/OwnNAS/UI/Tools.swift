import SwiftUI

/// A folder's name for titles, or the server's for its top.
private func folderTitle(_ path: String, _ serverName: String) -> String {
    path.isEmpty ? serverName : specialFolderLabel((path as NSString).lastPathComponent)
}

/// Files with the same contents in a folder and below. In each group the oldest copy is kept
/// and the others are ticked for the Trash; untick any to keep them too. At least one copy of
/// each group always stays.
struct DuplicatesView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let path: String

    @State private var found: Duplicates?
    @State private var problem: String?
    @State private var remove: [String: Bool] = [:]
    @State private var confirming = false
    @State private var working = false
    @State private var message: String?

    private var groups: [DuplicateGroup] { found?.groups ?? [] }
    private var chosen: [(path: String, size: UInt64)] {
        groups.flatMap { group in group.copies.filter { remove[$0.path] == true }.map { ($0.path, group.size) } }
    }

    var body: some View {
        Group {
            if let problem {
                ContentUnavailableView(problem, systemImage: "exclamationmark.triangle")
            } else if let found {
                if found.groups.isEmpty {
                    ContentUnavailableView("No duplicates here", systemImage: "checkmark.circle")
                } else {
                    List {
                        Section {
                            Text("\(found.groups.count) \(found.groups.count == 1 ? "file has" : "files have") copies, taking \(bytes(found.wasted)) extra.\(found.truncated ? " Showing the largest." : "")")
                                .font(.footnote).foregroundStyle(.secondary)
                        }
                        ForEach(found.groups, id: \.sha256) { group in
                            Section("\(bytes(group.size)) · \(group.copies.count) copies") {
                                ForEach(group.copies, id: \.path) { copy in
                                    row(copy, in: group)
                                }
                            }
                        }
                    }
                }
            } else {
                VStack(spacing: 12) {
                    ProgressView()
                    Text("Comparing files…").foregroundStyle(.secondary)
                }
            }
        }
        .navigationTitle("Duplicates")
        .navigationBarTitleDisplayMode(.inline)
        .safeAreaInset(edge: .bottom) {
            if !chosen.isEmpty {
                HStack {
                    Text("\(chosen.count) \(chosen.count == 1 ? "copy" : "copies") · \(bytes(chosen.reduce(0) { $0 + $1.size }))")
                    Spacer()
                    Button("Move to Trash") { confirming = true }.buttonStyle(.borderedProminent).disabled(working)
                }
                .padding().background(.bar)
            }
        }
        .confirmationDialog("Move \(chosen.count) \(chosen.count == 1 ? "copy" : "copies") to the Trash?", isPresented: $confirming, titleVisibility: .visible) {
            Button("Move to Trash", role: .destructive) { trash() }
        } message: {
            Text("Each is checked again first: it must still match, and a copy of it stays. You can restore them from the Trash.")
        }
        .alert(message ?? "", isPresented: Binding(get: { message != nil }, set: { if !$0 { message = nil } })) { Button("OK", role: .cancel) {} }
        .task { await load() }
    }

    private func row(_ copy: DuplicateCopy, in group: DuplicateGroup) -> some View {
        let ticked = remove[copy.path] == true
        let lastKept = !ticked && group.copies.filter { remove[$0.path] != true }.count == 1
        return Button {
            remove[copy.path] = !ticked
        } label: {
            HStack(spacing: 12) {
                Image(systemName: ticked ? "trash.circle.fill" : "checkmark.circle")
                    .foregroundStyle(ticked ? AnyShapeStyle(.red) : AnyShapeStyle(.tint))
                    .font(.title3)
                VStack(alignment: .leading, spacing: 2) {
                    Text((copy.path as NSString).lastPathComponent).foregroundStyle(.primary).lineLimit(1)
                    let folder = (copy.path as NSString).deletingLastPathComponent
                    Text("\(folder.isEmpty ? (hub.server(server)?.name ?? "") : folder) · \(Date(timeIntervalSince1970: Double(copy.modified) / 1000).formatted(.relative(presentation: .named))) · \(ticked ? "to the Trash" : "kept")")
                        .font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
            }
        }
        .disabled(lastKept)
    }

    private func load() async {
        found = nil
        problem = nil
        do {
            let result = try await hub.call(server) { [path] in try $0.duplicates(path: path) }
            var marks: [String: Bool] = [:]
            for group in result.groups {
                let keep = group.copies.min { $0.modified < $1.modified }?.path
                for copy in group.copies { marks[copy.path] = copy.path != keep }
            }
            remove = marks
            found = result
        } catch {
            problem = userMessage(error)
        }
    }

    private func trash() {
        working = true
        let choices: [DuplicateChoice] = groups.compactMap { group in
            let gone = group.copies.filter { remove[$0.path] == true }.map(\.path)
            guard !gone.isEmpty else { return nil }
            return DuplicateChoice(sha256: group.sha256, keep: group.copies.map(\.path).filter { !gone.contains($0) }, remove: gone)
        }
        Task {
            do {
                let done = try await hub.call(server) { try $0.trashDuplicates(choices: choices) }
                hub.changed()
                message = "Moved \(done.trashed) to the Trash, freeing \(bytes(done.freed))" + (done.skipped.isEmpty ? "" : ". Left \(done.skipped.count): \(done.skipped[0])")
                await load()
            } catch {
                message = userMessage(error)
            }
            working = false
        }
    }
}

/// Sorts a folder's files into folders by date, kind, extension, or first letter.
struct SortFolderView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let path: String

    @State private var preset = "{YYYY}/{MM}"
    @State private var custom = "Photos {YYYY}"
    @State private var date = "taken"
    @State private var recursive = false
    @State private var preview: SortResult?
    @State private var problem: String?
    @State private var working = false
    @State private var done: SortResult?

    private let patterns: [(String, String)] = [
        ("{YYYY}/{MM}", "Year / month (2024/03)"),
        ("{YYYY}-{MM}", "Year-month (2024-03)"),
        ("{YYYY}/{MM}/{DD}", "Year / month / day"),
        ("{YYYY}", "Year (2024)"),
        ("{YYYY}/{MM} {MMMM}", "Year / month with its name"),
        ("{type}", "Kind (Pictures, Videos, Documents…)"),
        ("{ext}", "Extension (jpg, pdf…)"),
        ("{first}", "First letter (A, B…)"),
        ("", "My own pattern…"),
    ]

    private var pattern: String { preset.isEmpty ? custom.trimmingCharacters(in: .whitespaces) : preset }
    private var usesDate: Bool { pattern.contains("{Y") || pattern.contains("{M") || pattern.contains("{D") }

    /// Month names in the phone's language, for {MMMM}.
    private var months: [String] {
        Calendar.current.standaloneMonthSymbols.prefix(12).map { $0.prefix(1).uppercased() + $0.dropFirst() }
    }

    var body: some View {
        Form {
            Section("Folders by") {
                Picker("Folders by", selection: $preset) {
                    ForEach(patterns, id: \.1) { Text($0.1).tag($0.0) }
                }
                .pickerStyle(.inline)
                .labelsHidden()
                if preset.isEmpty {
                    TextField("Pattern", text: $custom).autocorrectionDisabled().textInputAutocapitalization(.never)
                    Text("{YYYY} year, {MM} month, {DD} day, {MMMM} month name, {type} kind, {ext} extension, {first} first letter, / for a folder inside a folder")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            if usesDate {
                Section("Dates from") {
                    Picker("Dates from", selection: $date) {
                        Text("Date taken").tag("taken")
                        Text("Modified").tag("modified")
                        Text("File name").tag("name")
                    }
                    .pickerStyle(.segmented)
                }
            }
            Section {
                Toggle("Include subfolders", isOn: $recursive)
            } footer: { Text("Files in folders inside it are sorted too.") }
            if let problem {
                Section { Text(problem).foregroundStyle(.red) }
            }
            if let preview {
                Section {
                    ForEach(preview.moves.prefix(200), id: \.from) { move in
                        VStack(alignment: .leading, spacing: 2) {
                            Text((move.from as NSString).lastPathComponent).lineLimit(1)
                            Text("→ " + relative(move.to)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
                    if preview.total > UInt64(min(preview.moves.count, 200)) {
                        Text("…and \(preview.total - UInt64(min(preview.moves.count, 200))) more").foregroundStyle(.secondary)
                    }
                } header: {
                    Text(summary(preview))
                }
            }
        }
        .disabled(working)
        .navigationTitle("Sort into folders")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItemGroup(placement: .bottomBar) {
                Button("Preview") { run(apply: false) }.disabled(pattern.isEmpty || working)
                Spacer()
                if working { ProgressView() }
                Button("Sort") { run(apply: true) }
                    .buttonStyle(.borderedProminent)
                    .disabled(pattern.isEmpty || working || (preview?.total ?? 0) == 0)
            }
        }
        .onChange(of: "\(pattern)|\(date)|\(recursive)") { preview = nil; problem = nil }
        .alert(doneTitle, isPresented: Binding(get: { done != nil }, set: { if !$0 { done = nil } })) {
            if let result = done, !result.moves.isEmpty {
                Button("Undo") { undo(result.moves) }
            }
            Button("OK", role: .cancel) {}
        }
    }

    private var doneTitle: String {
        guard let done else { return "" }
        return "Moved \(done.total) \(done.total == 1 ? "file" : "files") into \(done.folders) \(done.folders == 1 ? "folder" : "folders")" + (done.failed > 0 ? ", \(done.failed) could not be moved" : "")
    }

    private func relative(_ to: String) -> String {
        guard !path.isEmpty, to.hasPrefix(path + "/") else { return to }
        return String(to.dropFirst(path.count + 1))
    }

    private func summary(_ preview: SortResult) -> String {
        if preview.total == 0 {
            return "Nothing to move" + (preview.inPlace > 0 ? ": \(preview.inPlace) already in place" : "")
        }
        return "\(preview.total) \(preview.total == 1 ? "file goes" : "files go") into \(preview.folders) \(preview.folders == 1 ? "folder" : "folders")" + (preview.inPlace > 0 ? " (\(preview.inPlace) already in place)" : "")
    }

    private func run(apply: Bool) {
        working = true
        problem = nil
        let (pattern, date, recursive, months) = (pattern, date, recursive, months)
        Task {
            do {
                let result = try await hub.call(server) { [path] in try $0.sortFolder(path: path, pattern: pattern, date: date, recursive: recursive, months: months, apply: apply) }
                if apply {
                    preview = nil
                    hub.changed()
                    done = result
                } else {
                    preview = result
                }
            } catch {
                problem = userMessage(error)
            }
            working = false
        }
    }

    private func undo(_ moves: [SortMove]) {
        working = true
        Task {
            do {
                let back = try await hub.call(server) { try $0.undoSort(moves: moves) }
                hub.changed()
                problem = "Put back \(back) \(back == 1 ? "file" : "files")"
            } catch {
                problem = userMessage(error)
            }
            working = false
        }
    }
}

/// What people did on the server lately, newest first.
struct ActivityView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    @State private var events: [ActivityEvent]?
    @State private var problem: String?

    var body: some View {
        Group {
            if let problem {
                ContentUnavailableView(problem, systemImage: "exclamationmark.triangle")
            } else if let events {
                if events.isEmpty {
                    ContentUnavailableView("Nothing yet", systemImage: "clock")
                } else {
                    List(Array(events.enumerated()), id: \.offset) { _, event in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(event.action.replacingOccurrences(of: "-", with: " ").capitalized).font(.caption).foregroundStyle(.secondary)
                            Text(event.detail.isEmpty ? event.action : event.detail).lineLimit(2)
                            Text("\(event.username) · \(Date(timeIntervalSince1970: Double(event.createdAt) / 1000).formatted(.relative(presentation: .named)))")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    .listStyle(.plain)
                }
            } else {
                ProgressView()
            }
        }
        .navigationTitle("Activity")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            do { events = try await hub.call(server) { try $0.activity() } } catch { problem = userMessage(error) }
        }
    }
}
