import AVKit
import SwiftUI
import UIKit

/// Photos full screen: swipe between them, pinch or double-tap to zoom; videos play.
struct PhotoViewer: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let items: [MediaItem]
    let start: Int
    @State private var index = 0
    @State private var chrome = true
    @State private var details: PhotoDetails?
    @State private var sharing: ShareItem?
    @State private var note: String?
    @State private var smart = SmartFeatures(search: false, faces: false)

    private var current: MediaItem? { items[safe: index] }

    var body: some View {
        TabView(selection: $index) {
            ForEach(Array(items.enumerated()), id: \.offset) { offset, item in
                PhotoPage(server: server, item: item, active: offset == index)
                    .tag(offset)
                    .onTapGesture { withAnimation { chrome.toggle() } }
            }
        }
        .tabViewStyle(.page(indexDisplayMode: .never))
        .background(Color.black.ignoresSafeArea())
        .navigationTitle(current?.takenLabel ?? current?.name ?? "")
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(.black.opacity(0.6), for: .navigationBar, .bottomBar)
        .toolbarColorScheme(.dark, for: .navigationBar, .bottomBar)
        .toolbar(chrome ? .visible : .hidden, for: .navigationBar, .bottomBar)
        .toolbar {
            ToolbarItemGroup(placement: .bottomBar) {
                Button("Share", systemImage: "square.and.arrow.up") { share() }
                Spacer()
                Button("Details", systemImage: "info.circle") { showDetails() }
                Spacer()
                Menu {
                    if smart.search, let current {
                        Button("Find similar photos", systemImage: "sparkle.magnifyingglass") { hub.path.append(.similar(server: server, path: current.path)) }
                    }
                    if let current {
                        Button("Show in folder", systemImage: "folder") {
                            hub.path.append(.browse(server: server, path: (current.path as NSString).deletingLastPathComponent))
                        }
                        Button(current.hidden ? "Show in Photos" : "Hide from Photos", systemImage: current.hidden ? "eye" : "eye.slash") {
                            let path = current.path, hide = !current.hidden
                            Task {
                                do {
                                    _ = try await hub.call(server) { try $0.hideFromTimeline(paths: [path], hidden: hide) }
                                    note = hide ? "Hidden from Photos" : "Back in Photos"
                                } catch { note = userMessage(error) }
                            }
                        }
                    }
                } label: { Image(systemName: "ellipsis.circle") }
            }
        }
        .sheet(item: $sharing) { ShareSheet(items: [$0.url]).presentationDetents([.medium, .large]) }
        .sheet(item: Binding(get: { details.map(DetailsItem.init) }, set: { if $0 == nil { details = nil } })) { item in
            DetailsSheet(details: item.details)
        }
        .alert(note ?? "", isPresented: Binding(get: { note != nil }, set: { if !$0 { note = nil } })) { Button("OK", role: .cancel) {} }
        .onAppear { index = start }
        .task { if let found = try? await hub.call(server, { try $0.smartFeatures() }) { smart = found } }
    }

    private func share() {
        guard let current else { return }
        Task {
            do { sharing = ShareItem(url: try await originalFile(hub, server: server, item: current)) } catch { note = userMessage(error) }
        }
    }

    private func showDetails() {
        guard let current else { return }
        let path = current.path
        Task {
            do { details = try await hub.call(server) { try $0.photoDetails(path: path) } } catch { note = userMessage(error) }
        }
    }
}

private struct DetailsItem: Identifiable {
    let details: PhotoDetails
    var id: String { details.path }
}

/// Everything the server knows about a photo: file, camera, place, albums, tags.
private struct DetailsSheet: View {
    let details: PhotoDetails

    var body: some View {
        NavigationStack {
            List {
                if !details.albums.isEmpty {
                    Section("Albums") { ForEach(details.albums, id: \.id) { Text($0.name) } }
                }
                if !details.tags.isEmpty {
                    Section("Tags") { Text(details.tags.joined(separator: ", ")) }
                }
                ForEach(details.sections, id: \.title) { section in
                    Section(section.title) {
                        ForEach(Array(section.rows.enumerated()), id: \.offset) { _, row in
                            LabeledContent(row.label, value: row.time.map { Date(timeIntervalSince1970: TimeInterval($0)).formatted(date: .abbreviated, time: .shortened) } ?? row.value)
                        }
                    }
                }
            }
            .navigationTitle(details.name)
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium, .large])
    }
}

/// One photo or video: the thumbnail at once, then the original.
private struct PhotoPage: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let item: MediaItem
    let active: Bool
    @State private var image: UIImage?
    @State private var video: AVPlayer?
    @State private var problem: String?

    var body: some View {
        ZStack {
            if let video {
                VideoPlayer(player: video).onDisappear { video.pause() }
            } else if let image {
                ZoomableImage(image: image)
            } else {
                ServerImage(server: server, path: item.path, key: "g:\(server):\(item.path):\(item.takenAt ?? 0):\(item.size ?? 0)") { Color.clear }
                    .scaledToFit()
                ProgressView().tint(.white)
            }
            if let problem { Text(problem).foregroundStyle(.white).padding() }
        }
        .task(id: active) {
            guard active, image == nil, video == nil else { return }
            do {
                let file = try await originalFile(hub, server: server, item: item)
                if item.kind == "video" {
                    video = AVPlayer(url: file)
                } else if let loaded = UIImage(contentsOfFile: file.path) {
                    image = await loaded.byPreparingForDisplay() ?? loaded
                } else {
                    problem = "This photo cannot be shown on the phone"
                }
            } catch {
                if !isCancel(error) { problem = userMessage(error) }
            }
        }
    }
}

/// A photo's original file, downloaded (or cached).
@MainActor
func originalFile(_ hub: Hub, server: String, item: MediaItem) async throws -> URL {
    try await hub.cachedCopy(server, path: item.path, size: UInt64(max(item.size ?? 0, 0)), modified: (item.takenAt ?? 0) * 1000, transfer: Tracker())
}

/// Pinch and double-tap zoom, as Photos does.
struct ZoomableImage: UIViewRepresentable {
    let image: UIImage

    func makeUIView(context: Context) -> UIScrollView {
        let scroll = UIScrollView()
        scroll.delegate = context.coordinator
        scroll.maximumZoomScale = 5
        scroll.showsHorizontalScrollIndicator = false
        scroll.showsVerticalScrollIndicator = false
        scroll.backgroundColor = .clear
        let view = UIImageView(image: image)
        view.contentMode = .scaleAspectFit
        view.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(view)
        NSLayoutConstraint.activate([
            view.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor),
            view.heightAnchor.constraint(equalTo: scroll.frameLayoutGuide.heightAnchor),
            view.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
            view.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
            view.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            view.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
        ])
        context.coordinator.view = view
        let doubleTap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.doubleTapped(_:)))
        doubleTap.numberOfTapsRequired = 2
        scroll.addGestureRecognizer(doubleTap)
        return scroll
    }

    func updateUIView(_ scroll: UIScrollView, context: Context) {
        context.coordinator.view?.image = image
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, UIScrollViewDelegate {
        weak var view: UIImageView?

        func viewForZooming(in scrollView: UIScrollView) -> UIView? { view }

        @objc func doubleTapped(_ gesture: UITapGestureRecognizer) {
            guard let scroll = gesture.view as? UIScrollView else { return }
            if scroll.zoomScale > 1 {
                scroll.setZoomScale(1, animated: true)
            } else {
                let point = gesture.location(in: view)
                let size = CGSize(width: scroll.bounds.width / 2.5, height: scroll.bounds.height / 2.5)
                scroll.zoom(to: CGRect(x: point.x - size.width / 2, y: point.y - size.height / 2, width: size.width, height: size.height), animated: true)
            }
        }
    }
}

extension MediaItem {
    /// When it was taken, as the camera's clock said (stored as UTC).
    var takenLabel: String? {
        guard let takenAt else { return nil }
        let format = DateFormatter()
        format.dateStyle = .medium
        format.timeStyle = .short
        if dateSource != "file" { format.timeZone = TimeZone(identifier: "UTC") }
        return format.string(from: Date(timeIntervalSince1970: TimeInterval(takenAt)))
    }
}
