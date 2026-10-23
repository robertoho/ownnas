import SwiftUI

@main
struct OwnNASApp: App {
    @StateObject private var hub = Hub()

    var body: some Scene {
        WindowGroup {
            RootView().environmentObject(hub)
        }
    }
}

/// The servers, and every screen pushed over them.
struct RootView: View {
    @EnvironmentObject private var hub: Hub

    var body: some View {
        NavigationStack(path: $hub.path) {
            ServersView()
                .navigationDestination(for: Route.self) { route in
                    switch route {
                    case let .browse(server, path): BrowserView(server: server, path: path)
                    case let .search(server, path): SearchView(server: server, path: path)
                    case let .gallery(server): GalleryView(server: server)
                    case let .album(server, id): AlbumView(server: server, id: id)
                    case let .person(server, id): PersonView(server: server, id: id)
                    case let .tag(server, tag):
                        LoadedPhotosView(server: server, title: tag, empty: "No photos or videos have this tag.") { try $0.tagged(tag: tag) }
                    case let .photoSearch(server, words):
                        LoadedPhotosView(server: server, title: "“\(words)”", empty: "Nothing matches. Try other words, in English: “red car”, “people at a table”.") { try $0.searchPhotos(words: words) }
                    case let .similar(server, path):
                        LoadedPhotosView(server: server, title: "Similar photos", subtitle: "Like \((path as NSString).lastPathComponent)", empty: "No photo looks much like this one.") { try $0.similarPhotos(path: path) }
                    case let .hidden(server):
                        LoadedPhotosView(server: server, title: "Hidden photos", subtitle: "Left out of Photos; still in their folders and albums", empty: "Nothing is hidden.") { try $0.hiddenPhotos() }
                    case let .place(server, name, west, south, east, north):
                        LoadedPhotosView(server: server, title: name, empty: "No photos here any more.") {
                            try $0.placePhotos(bounds: MapBounds(west: west, south: south, east: east, north: north), limit: 5000).items
                        }
                    case let .viewer(server, list, start): PhotoViewer(server: server, items: hub.viewerLists[list] ?? [], start: start)
                    case let .terminals(server, path): TerminalsView(server: server, path: path)
                    case let .terminal(server, id): TerminalScreenView(server: server, id: id)
                    }
                }
        }
    }
}
