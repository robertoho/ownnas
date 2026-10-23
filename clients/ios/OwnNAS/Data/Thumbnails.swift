import SwiftUI
import UIKit

/// Thumbnails by key, kept in memory, fetched a few at a time.
@MainActor
final class Thumbnails {
    static let shared = Thumbnails()

    private let cache: NSCache<NSString, UIImage> = {
        let cache = NSCache<NSString, UIImage>()
        cache.totalCostLimit = 48 * 1024 * 1024
        return cache
    }()
    private var running = 0
    private var waiting: [CheckedContinuation<Void, Never>] = []

    func cached(_ key: String) -> UIImage? { cache.object(forKey: key as NSString) }

    func remember(_ image: UIImage, _ key: String) {
        cache.setObject(image, forKey: key as NSString, cost: Int(image.size.width * image.size.height * image.scale * image.scale * 4))
    }

    /// A thumbnail (`nil` when the server has none), with at most four fetches at once.
    func load(_ hub: Hub, server: String, key: String, fetch: @escaping (Server) throws -> Data) async -> UIImage? {
        if let image = cached(key) { return image }
        if running >= 4 {
            await withCheckedContinuation { waiting.append($0) }
        } else {
            running += 1
        }
        defer {
            if waiting.isEmpty { running -= 1 } else { waiting.removeFirst().resume() }
        }
        if Task.isCancelled { return nil }
        guard let data = try? await hub.call(server, fetch), let image = UIImage(data: data) else { return nil }
        let prepared = await image.byPreparingForDisplay() ?? image
        remember(prepared, key)
        return prepared
    }
}

/// A server picture (a file's thumbnail, by default) filling its frame, or a placeholder.
struct ServerImage<Placeholder: View>: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let key: String
    var fetch: ((Server) throws -> Data)?
    @ViewBuilder var placeholder: () -> Placeholder
    @State private var image: UIImage?

    init(server: String, path: String, key: String? = nil, @ViewBuilder placeholder: @escaping () -> Placeholder) {
        self.server = server
        self.key = key ?? "\(server):\(path)"
        self.fetch = { try $0.thumbnail(path: path) }
        self.placeholder = placeholder
    }

    init(server: String, key: String, fetch: @escaping (Server) throws -> Data, @ViewBuilder placeholder: @escaping () -> Placeholder) {
        self.server = server
        self.key = key
        self.fetch = fetch
        self.placeholder = placeholder
    }

    var body: some View {
        ZStack {
            if let image = image ?? Thumbnails.shared.cached(key) {
                Image(uiImage: image).resizable().scaledToFill()
            } else {
                placeholder()
            }
        }
        .task(id: key) {
            guard image == nil, Thumbnails.shared.cached(key) == nil, let fetch else { return }
            image = await Thumbnails.shared.load(hub, server: server, key: key, fetch: fetch)
        }
    }
}
