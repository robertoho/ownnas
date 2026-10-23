import MapKit
import SwiftUI

/// A server's Gallery: photos by date (with On this day), albums, people, places, and tags, in
/// the workspace chosen in the title.
struct GalleryView: View {
    @EnvironmentObject private var hub: Hub
    let server: String

    enum Tab: String, CaseIterable { case photos = "Photos", albums = "Albums", people = "People", places = "Places", tags = "Tags" }

    @State private var tab: Tab = .photos
    @State private var smart = SmartFeatures(search: false, faces: false)
    @State private var workspaces: [Workspace] = []
    @State private var workspace: Int64?
    @State private var searching = false
    @State private var words = ""

    private var tabs: [Tab] { Tab.allCases.filter { $0 != .people || smart.faces } }

    var body: some View {
        VStack(spacing: 0) {
            Picker("Show", selection: $tab) {
                ForEach(tabs, id: \.self) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented).padding(.horizontal).padding(.bottom, 8)
            Group {
                switch tab {
                case .photos: TimelineView(server: server)
                case .albums: AlbumsTab(server: server)
                case .people: PeopleTab(server: server)
                case .places: PlacesTab(server: server)
                case .tags: TagsTab(server: server)
                }
            }
            // Every tab reloads in another workspace.
            .id(workspace)
            .frame(maxHeight: .infinity)
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) { workspaceMenu }
            ToolbarItemGroup(placement: .primaryAction) {
                if smart.search {
                    Button("Search photos", systemImage: "magnifyingglass") { searching = true }
                }
                Menu {
                    Button("Hidden photos", systemImage: "eye.slash") { hub.path.append(.hidden(server: server)) }
                } label: { Image(systemName: "ellipsis.circle") }
            }
        }
        .alert("Search photos", isPresented: $searching) {
            TextField("Dog on the beach…", text: $words)
            Button("Search") {
                let typed = words.trimmingCharacters(in: .whitespaces)
                if !typed.isEmpty { hub.path.append(.photoSearch(server: server, words: typed)) }
            }
            Button("Cancel", role: .cancel) {}
        } message: { Text("Describe what the photos show.") }
        .task {
            workspace = hub.galleryWorkspace(server)
            if let found = try? await hub.call(server, { try $0.smartFeatures() }) { smart = found }
            if let found = try? await hub.call(server, { try $0.workspaces() }) {
                workspaces = found
                if let chosen = workspace, !found.contains(where: { $0.id == chosen }) { choose(nil) }
            }
        }
    }

    /// The title: the workspace showing, and a menu to show another (or every photo).
    @ViewBuilder private var workspaceMenu: some View {
        let showing = workspaces.first { $0.id == workspace }
        if workspaces.isEmpty {
            Text("Gallery").font(.headline)
        } else {
            Menu {
                Picker("Workspace", selection: Binding(get: { workspace }, set: { choose($0) })) {
                    Text("All photos").tag(Int64?.none)
                    ForEach(workspaces, id: \.id) { space in
                        Text("\(space.name)  \(space.photos)").tag(Optional(space.id))
                    }
                }
            } label: {
                HStack(spacing: 4) {
                    Text(showing?.name ?? "Gallery").font(.headline)
                    Image(systemName: "chevron.down").font(.caption.bold())
                }
                .foregroundStyle(.primary)
            }
        }
    }

    private func choose(_ id: Int64?) {
        hub.setGalleryWorkspace(server, id)
        workspace = id
    }
}

// MARK: Photos

/// Every photo by the day it was taken, newest first, loaded a month at a time, with On this
/// day above.
struct TimelineView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    @State private var days: [DayCount]?
    @State private var problem: String?
    @State private var memories: [MemoryYear] = []
    @State private var loaded: [String: [String: [MediaItem]]] = [:]
    @State private var loading: Set<String> = []
    @StateObject private var selection = PhotoSelection()

    var body: some View {
        Group {
            if let problem {
                ContentUnavailableView(problem, systemImage: "exclamationmark.triangle")
            } else if let days, days.isEmpty {
                ContentUnavailableView("No photos or videos yet", systemImage: "photo.on.rectangle", description: Text("Photos anywhere in the library show up here, newest first."))
            } else if let days {
                ScrollView {
                    if !memories.isEmpty { OnThisDayRow(server: server, years: memories) }
                    LazyVGrid(columns: photoColumns, spacing: 2, pinnedViews: [.sectionHeaders]) {
                        ForEach(days, id: \.day) { day in
                            Section {
                                ForEach(0..<Int(day.count), id: \.self) { index in
                                    cell(day: day.day, index: index)
                                }
                            } header: {
                                Text(dayLabel(day.day)).font(.subheadline.weight(.semibold))
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.horizontal, 8).padding(.vertical, 6).background(.bar)
                            }
                        }
                    }
                }
            } else {
                ProgressView()
            }
        }
        .photoSelection(selection, server: server)
        .task {
            do { days = try await hub.call(server) { try $0.galleryDays() }.days } catch { problem = userMessage(error) }
            let today = Calendar.current.dateComponents([.year, .month, .day], from: .now)
            if let found = try? await hub.call(server, { try $0.onThisDay(month: UInt32(today.month!), day: UInt32(today.day!), year: Int32(today.year!)) }) {
                memories = found
            }
        }
    }

    @ViewBuilder private func cell(day: String, index: Int) -> some View {
        let month = String(day.prefix(7))
        if let item = loaded[month]?[day]?[safe: index] {
            PhotoTile(server: server, item: item, selection: selection) {
                let all = (days ?? []).flatMap { d in loaded[String(d.day.prefix(7))]?[d.day] ?? [] }
                hub.openViewer(server, all, start: all.firstIndex { $0.path == item.path } ?? 0)
            }
        } else {
            Rectangle().fill(Color(.secondarySystemBackground)).aspectRatio(1, contentMode: .fit)
                .onAppear { load(month) }
        }
    }

    private func load(_ month: String) {
        guard loaded[month] == nil, !loading.contains(month) else { return }
        loading.insert(month)
        let formatter = ISO8601DateFormatter()
        guard let start = formatter.date(from: "\(month)-01T00:00:00Z"),
              let end = Calendar(identifier: .gregorian).date(byAdding: .month, value: 1, to: start) else { return }
        let (from, to) = (Int64(start.timeIntervalSince1970), Int64(end.timeIntervalSince1970))
        Task {
            defer { loading.remove(month) }
            guard let items = try? await hub.call(server, { try $0.galleryBetween(from: from, to: to) }) else { return }
            loaded[month] = Dictionary(grouping: items) { $0.day ?? "" }
        }
    }
}

/// Photos taken on this day in earlier years, a card a year.
struct OnThisDayRow: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let years: [MemoryYear]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("On this day").font(.title3.bold()).padding(.horizontal, 12)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(years, id: \.year) { year in
                        let ago = Calendar.current.component(.year, from: .now) - Int(year.year)
                        PictureCard(server: server, path: year.items.first?.path, title: ago == 1 ? "1 year ago" : "\(ago) years ago", subtitle: "\(year.year) · \(photos(year.count))")
                            .frame(width: 124)
                            .onTapGesture { hub.openViewer(server, year.items, start: 0) }
                    }
                }
                .padding(.horizontal, 12)
            }
        }
        .padding(.vertical, 8)
    }
}

/// A photo with a title over its bottom edge.
struct PictureCard: View {
    let server: String
    let path: String?
    let title: String
    let subtitle: String

    var body: some View {
        Color(.secondarySystemBackground)
            .aspectRatio(0.8, contentMode: .fit)
            .overlay {
                if let path { ServerImage(server: server, path: path, key: "card:\(server):\(path)") { Color.clear } }
            }
            .overlay(alignment: .bottomLeading) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).font(.subheadline.bold()).lineLimit(2)
                    Text(subtitle).font(.caption).opacity(0.85).lineLimit(1)
                }
                .foregroundStyle(.white).padding(10).frame(maxWidth: .infinity, alignment: .leading)
                .background(LinearGradient(colors: [.clear, .black.opacity(0.75)], startPoint: .top, endPoint: .bottom))
            }
            .clipShape(RoundedRectangle(cornerRadius: 14))
            .contentShape(RoundedRectangle(cornerRadius: 14))
    }
}

// MARK: Albums

struct AlbumsTab: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    @State private var page: AlbumPage?
    @State private var problem: String?
    @State private var prompt: NamePrompt?

    var body: some View {
        Group {
            if let problem {
                ContentUnavailableView(problem, systemImage: "exclamationmark.triangle")
            } else if let page, page.albums.isEmpty {
                ContentUnavailableView {
                    Label("No albums yet", systemImage: "rectangle.stack")
                } description: {
                    Text("Albums gather photos from anywhere in the library without moving them, and can hold other albums, like folders.")
                } actions: {
                    Button("New album") { newAlbum() }
                }
            } else if let page {
                ScrollView {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 12)], spacing: 16) {
                        ForEach(page.albums, id: \.id) { album in
                            AlbumCard(server: server, album: album).onTapGesture { hub.path.append(.album(server: server, id: album.id)) }
                        }
                    }
                    .padding(12)
                }
            } else {
                ProgressView()
            }
        }
        .toolbar {
            ToolbarItem(placement: .bottomBar) { Button("New album", systemImage: "plus") { newAlbum() } }
        }
        .namePrompt($prompt)
        .task { await load() }
    }

    private func load() async {
        do { page = try await hub.call(server) { try $0.album(id: nil) } } catch { problem = userMessage(error) }
    }

    private func newAlbum() {
        prompt = NamePrompt(title: "New album", confirm: "Create") { name in
            Task {
                do {
                    let album = try await hub.call(server) { try $0.createAlbum(name: name, parentId: nil) }
                    await load()
                    hub.path.append(.album(server: server, id: album.id))
                } catch { problem = userMessage(error) }
            }
        }
    }
}

struct AlbumCard: View {
    let server: String
    let album: Album

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Color(.secondarySystemBackground)
                .aspectRatio(1, contentMode: .fit)
                .overlay {
                    if let cover = album.cover {
                        ServerImage(server: server, path: cover, key: "cover:\(server):\(cover)") { Color.clear }
                    } else {
                        Image(systemName: "photo.on.rectangle").font(.largeTitle).foregroundStyle(.secondary)
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: 14))
            Text(album.name).font(.subheadline.weight(.semibold)).lineLimit(1)
            Text([album.count > 0 ? photos(album.count) : nil, album.albums > 0 ? "\(album.albums) album\(album.albums == 1 ? "" : "s")" : nil].compactMap { $0 }.joined(separator: " · ").ifEmpty("Empty"))
                .font(.caption).foregroundStyle(.secondary)
        }
        .contentShape(Rectangle())
    }
}

struct AlbumView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let id: Int64
    @State private var page: AlbumPage?
    @State private var problem: String?
    @State private var prompt: NamePrompt?
    @State private var deleting = false
    @StateObject private var selection = PhotoSelection()

    var body: some View {
        PhotoGrid(server: server, items: page?.items, empty: "No photos in this album yet. Select photos in the Gallery and choose Add to album.", selection: selection) {
            if let inside = page?.albums, !inside.isEmpty {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 12)], spacing: 16) {
                    ForEach(inside, id: \.id) { child in
                        AlbumCard(server: server, album: child).onTapGesture { hub.path.append(.album(server: server, id: child.id)) }
                    }
                }
                .padding(12)
                Text("Photos").font(.headline).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 12)
            }
        }
        .photoSelection(selection, server: server, album: page?.album, onChanged: { Task { await load() } })
        .navigationTitle(page?.album?.name ?? "Album")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Menu {
                    Button("New album inside", systemImage: "plus.rectangle.on.rectangle") {
                        prompt = NamePrompt(title: "New album inside “\(page?.album?.name ?? "")”", confirm: "Create") { name in
                            change {
                                let made = try await hub.call(server) { try $0.createAlbum(name: name, parentId: id) }
                                hub.path.append(.album(server: server, id: made.id))
                            }
                        }
                    }
                    Button("Rename", systemImage: "pencil") {
                        prompt = NamePrompt(title: "Rename album", initial: page?.album?.name ?? "", confirm: "Rename") { name in
                            change { try await hub.call(server) { try $0.renameAlbum(id: id, name: name) } }
                        }
                    }
                    Button("Delete album", systemImage: "trash", role: .destructive) { deleting = true }
                } label: { Image(systemName: "ellipsis.circle") }
            }
        }
        .namePrompt($prompt)
        .confirmationDialog("Delete “\(page?.album?.name ?? "")”?", isPresented: $deleting, titleVisibility: .visible) {
            Button("Delete album", role: .destructive) {
                change {
                    try await hub.call(server) { try $0.deleteAlbum(id: id) }
                    hub.path.removeLast()
                }
            }
        } message: {
            Text("The album\((page?.album?.albums ?? 0) > 0 ? " and the albums inside it" : "") goes; the photos stay where they are.")
        }
        .alert(problem ?? "", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) { Button("OK", role: .cancel) {} }
        .task { await load() }
    }

    private func load() async {
        do { page = try await hub.call(server) { try $0.album(id: id) } } catch { problem = userMessage(error) }
    }

    private func change(_ action: @escaping () async throws -> Void) {
        Task {
            do { try await action(); await load() } catch { problem = userMessage(error) }
        }
    }
}

// MARK: People

struct PeopleTab: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    @State private var people: [Person]?
    @State private var problem: String?

    var body: some View {
        Group {
            if let problem {
                ContentUnavailableView(problem, systemImage: "exclamationmark.triangle")
            } else if let people, people.isEmpty {
                ContentUnavailableView("No people yet", systemImage: "person.2", description: Text("Someone shows up here once they are in two photos."))
            } else if let people {
                ScrollView {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 100), spacing: 12)], spacing: 16) {
                        ForEach(people, id: \.id) { person in
                            VStack(spacing: 4) {
                                FaceCircle(server: server, face: person.coverFace, size: 88)
                                Text(person.name ?? "Add a name").font(.subheadline).lineLimit(1)
                                    .foregroundStyle(person.name == nil ? .secondary : .primary)
                                Text(photos(person.photos)).font(.caption).foregroundStyle(.secondary)
                            }
                            .onTapGesture { hub.path.append(.person(server: server, id: person.id)) }
                        }
                    }
                    .padding(12)
                }
            } else {
                ProgressView()
            }
        }
        .task {
            do { people = try await hub.call(server) { try $0.people(hidden: false) } } catch { problem = userMessage(error) }
        }
    }
}

struct FaceCircle: View {
    let server: String
    let face: Int64?
    let size: CGFloat

    var body: some View {
        Circle().fill(Color(.secondarySystemBackground))
            .overlay {
                if let face {
                    ServerImage(server: server, key: "face:\(server):\(face)", fetch: { try $0.facePicture(faceId: face) }) {
                        Image(systemName: "person.fill").foregroundStyle(.secondary)
                    }
                } else {
                    Image(systemName: "person.fill").foregroundStyle(.secondary)
                }
            }
            .frame(width: size, height: size)
            .clipShape(Circle())
    }
}

struct PersonView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let id: Int64
    @State private var page: PersonPage?
    @State private var problem: String?
    @State private var prompt: NamePrompt?
    @StateObject private var selection = PhotoSelection()

    var body: some View {
        PhotoGrid(server: server, items: page?.items, empty: "No photos left for this person.", selection: selection) { EmptyView() }
            .photoSelection(selection, server: server, person: page?.person, onChanged: { Task { await load() } })
            .navigationTitle(page?.person.name ?? "Unnamed person")
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        Button(page?.person.name == nil ? "Add a name" : "Rename", systemImage: "pencil") {
                            prompt = NamePrompt(title: "Who is this?", initial: page?.person.name ?? "", confirm: "Save") { name in
                                change(name: name)
                            }
                        }
                        Button(page?.person.hidden == true ? "Show in People" : "Hide from People", systemImage: "eye.slash") {
                            change(hidden: page?.person.hidden != true)
                        }
                    } label: { Image(systemName: "ellipsis.circle") }
                }
            }
            .namePrompt($prompt)
            .alert(problem ?? "", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) { Button("OK", role: .cancel) {} }
            .task { await load() }
    }

    private func load() async {
        do { page = try await hub.call(server) { try $0.person(id: id) } } catch { problem = userMessage(error) }
    }

    private func change(name: String? = nil, hidden: Bool? = nil) {
        Task {
            do {
                try await hub.call(server) { try $0.changePerson(id: id, name: name, hidden: hidden, into: nil) }
                if hidden == true { hub.path.removeLast() } else { await load() }
            } catch { problem = userMessage(error) }
        }
    }
}

// MARK: Places

/// Where photos were taken, on a map. Tap a place for its photos.
struct PlacesTab: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    @State private var places: [Place]?
    @State private var problem: String?
    @State private var camera: MapCameraPosition = .automatic

    var body: some View {
        Group {
            if let problem {
                ContentUnavailableView(problem, systemImage: "exclamationmark.triangle")
            } else if let places, places.isEmpty {
                ContentUnavailableView("No places yet", systemImage: "map", description: Text("Photos taken with the camera's location on show up here, by where they were taken."))
            } else if let places {
                Map(position: $camera) {
                    ForEach(places, id: \.areaKey) { place in
                        Annotation(photos(Int64(place.count)), coordinate: CLLocationCoordinate2D(latitude: place.lat, longitude: place.lon)) {
                            PlaceBubble(server: server, place: place).onTapGesture { open(place) }
                        }
                    }
                }
                .mapStyle(.standard(pointsOfInterest: .excludingAll))
            } else {
                ProgressView()
            }
        }
        .task {
            // About a city and its surroundings, as on Android.
            do { places = try await hub.call(server) { try $0.places(zoom: 7, bounds: nil) }.places } catch { problem = userMessage(error) }
        }
    }

    private func open(_ place: Place) {
        Task {
            let named = try? await CLGeocoder().reverseGeocodeLocation(CLLocation(latitude: place.lat, longitude: place.lon)).first
            let name = [named?.locality ?? named?.administrativeArea, named?.country].compactMap { $0 }.joined(separator: ", ")
            let area = place.area
            hub.path.append(.place(server: server, name: name.isEmpty ? photos(Int64(place.count)) : name, west: area.west, south: area.south, east: area.east, north: area.north))
        }
    }
}

private struct PlaceBubble: View {
    let server: String
    let place: Place

    var body: some View {
        ServerImage(server: server, path: place.newest.path, key: "card:\(server):\(place.newest.path)") { Color(.secondarySystemBackground) }
            .frame(width: 52, height: 52)
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(.white, lineWidth: 3))
            .overlay(alignment: .topTrailing) {
                Text("\(place.count)").font(.caption2.bold()).foregroundStyle(.white)
                    .padding(.horizontal, 6).padding(.vertical, 2).background(Capsule().fill(Color.accentColor)).offset(x: 8, y: -8)
            }
            .shadow(radius: 3)
    }
}

extension Place {
    var areaKey: String { "\(area.west),\(area.north)" }
}

// MARK: Tags

struct TagsTab: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    @State private var tags: [TagCount]?
    @State private var problem: String?

    var body: some View {
        Group {
            if let problem {
                ContentUnavailableView(problem, systemImage: "exclamationmark.triangle")
            } else if let tags, tags.isEmpty {
                ContentUnavailableView("No tags yet", systemImage: "tag", description: Text("Select photos and choose Tag…"))
            } else if let tags {
                List(tags, id: \.tag) { tag in
                    Button { hub.path.append(.tag(server: server, tag: tag.tag)) } label: {
                        LabeledContent { Text("\(tag.count)") } label: { Label(tag.tag, systemImage: "tag").foregroundStyle(.primary) }
                    }
                }
                .listStyle(.plain)
            } else {
                ProgressView()
            }
        }
        .task {
            do { tags = try await hub.call(server) { try $0.tags() } } catch { problem = userMessage(error) }
        }
    }
}

// MARK: Helpers

let photoColumns = [GridItem(.adaptive(minimum: 92), spacing: 2)]

func photos(_ count: Int64) -> String { "\(count) photo\(count == 1 ? "" : "s")" }

private let dayFormat: DateFormatter = {
    let format = DateFormatter()
    format.dateStyle = .full
    format.timeZone = TimeZone(identifier: "UTC")
    return format
}()

/// "2024-07-15" as the phone writes dates.
func dayLabel(_ day: String) -> String {
    guard let date = ISO8601DateFormatter().date(from: "\(day)T00:00:00Z") else { return day }
    return dayFormat.string(from: date)
}

extension MediaItem {
    /// The day it was taken ("2024-07-15"); camera times are stored as UTC.
    var day: String? {
        guard let takenAt else { return nil }
        let date = Date(timeIntervalSince1970: TimeInterval(takenAt))
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let parts = calendar.dateComponents([.year, .month, .day], from: date)
        return String(format: "%04d-%02d-%02d", parts.year!, parts.month!, parts.day!)
    }

    var name: String { (path as NSString).lastPathComponent }
}

extension Array {
    subscript(safe index: Int) -> Element? { indices.contains(index) ? self[index] : nil }
}

extension String {
    func ifEmpty(_ other: String) -> String { isEmpty ? other : self }
}
