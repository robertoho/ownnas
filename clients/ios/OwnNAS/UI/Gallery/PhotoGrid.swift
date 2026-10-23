import SwiftUI

/// Photos picked for an action. Selecting starts with Select in the toolbar or a long press.
@MainActor
final class PhotoSelection: ObservableObject {
    @Published var active = false
    @Published var items: [String: MediaItem] = [:]

    func toggle(_ item: MediaItem) {
        if items[item.path] != nil { items[item.path] = nil } else { items[item.path] = item }
    }

    func clear() {
        items = [:]
        active = false
    }
}

/// One square photo: its thumbnail, a play mark for videos, and a tick when selecting.
struct PhotoTile: View {
    let server: String
    let item: MediaItem
    @ObservedObject var selection: PhotoSelection
    let open: () -> Void

    var body: some View {
        let selected = selection.items[item.path] != nil
        Color(.secondarySystemBackground)
            .aspectRatio(1, contentMode: .fit)
            .overlay {
                if item.missing {
                    Text("Missing").font(.caption).foregroundStyle(.red)
                } else {
                    ServerImage(server: server, path: item.path, key: "g:\(server):\(item.path):\(item.takenAt ?? 0):\(item.size ?? 0)") {
                        Text(item.name).font(.caption2).foregroundStyle(.secondary).lineLimit(3).padding(4)
                    }
                }
            }
            .clipped()
            .overlay(alignment: .bottomTrailing) {
                if item.kind == "video" {
                    Image(systemName: "play.fill").font(.caption).foregroundStyle(.white).padding(5)
                        .background(Circle().fill(.black.opacity(0.5))).padding(4)
                }
            }
            .overlay(alignment: .bottomLeading) {
                if item.hidden {
                    Image(systemName: "eye.slash").font(.caption2).foregroundStyle(.white).padding(4)
                        .background(Circle().fill(.black.opacity(0.5))).padding(4)
                }
            }
            .overlay(alignment: .topLeading) {
                if selection.active {
                    Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                        .font(.title3).foregroundStyle(selected ? Color.accentColor : .white)
                        .background(Circle().fill(selected ? .white : .black.opacity(0.25))).padding(5)
                }
            }
            .scaleEffect(selected ? 0.9 : 1)
            .contentShape(Rectangle())
            .onTapGesture {
                if selection.active { selection.toggle(item) } else if !item.missing { open() }
            }
            .onLongPressGesture {
                selection.active = true
                selection.toggle(item)
            }
            .animation(.easeOut(duration: 0.12), value: selected)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(item.kind == "video" ? "Video \(item.name)" : item.name)
            .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
    }
}

/// A grid of photos (and a header above them) that opens them in the viewer.
struct PhotoGrid<Header: View>: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let items: [MediaItem]?
    let empty: String
    @ObservedObject var selection: PhotoSelection
    @ViewBuilder var header: () -> Header

    var body: some View {
        if let items {
            ScrollView {
                header()
                if items.isEmpty {
                    Text(empty).foregroundStyle(.secondary).multilineTextAlignment(.center).padding(32)
                }
                LazyVGrid(columns: photoColumns, spacing: 2) {
                    ForEach(items, id: \.path) { item in
                        PhotoTile(server: server, item: item, selection: selection) {
                            let shown = items.filter { !$0.missing }
                            hub.openViewer(server, shown, start: shown.firstIndex { $0.path == item.path } ?? 0)
                        }
                    }
                }
            }
        } else {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

/// A titled page of photos loaded with one call (a tag, a search, a place…).
struct LoadedPhotosView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let title: String
    var subtitle: String?
    let empty: String
    let load: (Server) throws -> [MediaItem]
    @State private var items: [MediaItem]?
    @State private var problem: String?
    @StateObject private var selection = PhotoSelection()

    var body: some View {
        PhotoGrid(server: server, items: items, empty: problem ?? empty, selection: selection) {
            if let subtitle { Text(subtitle).font(.footnote).foregroundStyle(.secondary).padding(.top, 4) }
        }
        .photoSelection(selection, server: server, onChanged: { Task { await reload() } })
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .task { await reload() }
    }

    private func reload() async {
        do { items = try await hub.call(server, load) } catch { items = []; problem = userMessage(error) }
    }
}

extension View {
    /// Select in the toolbar, and what can be done with the photos picked: albums, tags, hiding,
    /// finding similar ones, deleting from the server.
    func photoSelection(_ selection: PhotoSelection, server: String, album: Album? = nil, person: Person? = nil, onChanged: @escaping () -> Void = {}) -> some View {
        modifier(SelectionActions(selection: selection, server: server, album: album, person: person, onChanged: onChanged))
    }
}

private struct SelectionActions: ViewModifier {
    @EnvironmentObject private var hub: Hub
    @ObservedObject var selection: PhotoSelection
    let server: String
    let album: Album?
    let person: Person?
    let onChanged: () -> Void
    @State private var picking = false
    @State private var prompt: NamePrompt?
    @State private var deleting = false
    @State private var note: String?

    private var picked: [MediaItem] { Array(selection.items.values) }
    private var paths: [String] { picked.filter { !$0.missing }.map(\.path) }

    func body(content: Content) -> some View {
        content
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button(selection.active ? "Done" : "Select") { if selection.active { selection.clear() } else { selection.active = true } }
                }
                if selection.active {
                    ToolbarItemGroup(placement: .bottomBar) {
                        Text("\(picked.count) selected").font(.footnote)
                        Spacer()
                        Button("Add to album", systemImage: "rectangle.stack.badge.plus") { picking = true }.disabled(paths.isEmpty)
                        Menu {
                            Button("Tag…", systemImage: "tag") {
                                prompt = NamePrompt(title: "Tag \(paths.count) photo\(paths.count == 1 ? "" : "s")", confirm: "Tag") { tag in
                                    act("Tagged “\(tag)”") { try $0.tagPhotos(paths: paths, tag: tag, add: true) }
                                }
                            }
                            if picked.contains(where: { !$0.hidden }) {
                                Button("Hide from Photos", systemImage: "eye.slash") {
                                    act("Hidden from Photos (still in their folders and albums)") { try _ = $0.hideFromTimeline(paths: paths, hidden: true) }
                                }
                            }
                            if picked.contains(where: \.hidden) {
                                let hidden = picked.filter(\.hidden).map(\.path)
                                Button("Show in Photos", systemImage: "eye") { act("Back in Photos") { try _ = $0.hideFromTimeline(paths: hidden, hidden: false) } }
                            }
                            if picked.count == 1, let one = picked.first {
                                Button("Find similar photos", systemImage: "sparkle.magnifyingglass") {
                                    selection.clear()
                                    hub.path.append(.similar(server: server, path: one.path))
                                }
                            }
                            if let album {
                                let all = picked.map(\.path)
                                Button("Remove from album", systemImage: "minus.circle") { act("Removed from “\(album.name)”") { try _ = $0.changeAlbum(albumId: album.id, paths: all, add: false) } }
                            }
                            if person != nil {
                                let faces = picked.compactMap(\.faceId)
                                Button("Not \(person?.name ?? "this person")", systemImage: "person.crop.circle.badge.xmark") { act("Taken out") { try $0.moveFaces(faces: faces, to: nil) } }
                            }
                            if !paths.isEmpty {
                                Button("Delete from server…", systemImage: "trash", role: .destructive) { deleting = true }
                            }
                        } label: { Image(systemName: "ellipsis.circle") }
                    }
                }
            }
            .toolbar(selection.active ? .visible : .automatic, for: .bottomBar)
            .sheet(isPresented: $picking) {
                AlbumPicker(server: server, title: "Add \(paths.count) to…") { albumId in
                    picking = false
                    act("Added to the album") { try _ = $0.changeAlbum(albumId: albumId, paths: paths, add: true) }
                }
            }
            .namePrompt($prompt)
            .confirmationDialog("Delete \(paths.count == 1 ? "this photo" : "\(paths.count) photos") from the server?", isPresented: $deleting, titleVisibility: .visible) {
                Button("Delete", role: .destructive) {
                    act("Deleted; phones won't back \(paths.count == 1 ? "it" : "them") up again") { try _ = $0.deleteAndBlock(paths: paths) }
                }
            } message: {
                Text("They go to the server's Trash, and phones never back them up again.")
            }
            .alert(note ?? "", isPresented: Binding(get: { note != nil }, set: { if !$0 { note = nil } })) { Button("OK", role: .cancel) {} }
    }

    private func act(_ done: String, _ call: @escaping (Server) throws -> Void) {
        Task {
            do {
                try await hub.call(server, call)
                selection.clear()
                onChanged()
                note = done
            } catch {
                note = userMessage(error)
            }
        }
    }
}

/// Choosing an album to add photos to, with a way to make a new one.
struct AlbumPicker: View {
    @EnvironmentObject private var hub: Hub
    @Environment(\.dismiss) private var dismiss
    let server: String
    let title: String
    let pick: (Int64) -> Void
    @State private var albums: [Album]?
    @State private var filter = ""
    @State private var prompt: NamePrompt?
    @State private var problem: String?

    var body: some View {
        NavigationStack {
            List {
                Button("New album…", systemImage: "plus") {
                    prompt = NamePrompt(title: "New album", confirm: "Create") { name in
                        Task {
                            do { pick(try await hub.call(server) { try $0.createAlbum(name: name, parentId: nil) }.id) } catch { problem = userMessage(error) }
                        }
                    }
                }
                if let problem { Text(problem).foregroundStyle(.red) }
                ForEach(shown, id: \.id) { album in
                    Button { pick(album.id) } label: {
                        VStack(alignment: .leading) {
                            Text(album.name).foregroundStyle(.primary)
                            if let parent = path(album) { Text(parent).font(.caption).foregroundStyle(.secondary) }
                        }
                    }
                }
            }
            .searchable(text: $filter, prompt: "Find an album")
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
            .namePrompt($prompt)
            .task {
                do { albums = try await hub.call(server) { try $0.allAlbums() } } catch { problem = userMessage(error) }
            }
        }
    }

    private var shown: [Album] {
        let all = albums ?? []
        return filter.isEmpty ? all : all.filter { $0.name.localizedCaseInsensitiveContains(filter) }
    }

    /// The albums an album is inside, "Trips › Italy".
    private func path(_ album: Album) -> String? {
        var names: [String] = []
        var parent = album.parentId
        while let id = parent, let found = albums?.first(where: { $0.id == id }) {
            names.insert(found.name, at: 0)
            parent = found.parentId
        }
        return names.isEmpty ? nil : names.joined(separator: " › ")
    }
}
