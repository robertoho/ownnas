import SwiftUI

/// How a folder is shown, as the web app's view buttons.
enum BrowserLayout: String, CaseIterable {
    case list, grid, masonry, columns

    var label: String {
        switch self {
        case .list: return "List"
        case .grid: return "Icons"
        case .masonry: return "Masonry"
        case .columns: return "Columns"
        }
    }

    var systemImage: String {
        switch self {
        case .list: return "list.bullet"
        case .grid: return "square.grid.2x2"
        case .masonry: return "rectangle.3.group"
        case .columns: return "rectangle.split.3x1"
        }
    }
}

/// Columns need room: an iPhone on its side, a wide or unfolded iPhone, or an iPad. Measured
/// from the window, not the size class, so every iPhone's landscape counts.
let wideScreenWidth: CGFloat = 600

/// Tile widths for Icons and Masonry: small, medium, large.
let tileWidths: [CGFloat] = [96, 136, 196]
private let tileLabels = ["Small", "Medium", "Large"]

/// The toolbar's view menu: layouts, and the tile size for Icons and Masonry.
struct LayoutMenu: View {
    let chosen: BrowserLayout
    let wide: Bool
    @Binding var tile: Int
    let choose: (BrowserLayout) -> Void

    private var shown: BrowserLayout { chosen == .columns && !wide ? .list : chosen }

    var body: some View {
        Menu {
            Picker("View", selection: Binding(get: { chosen }, set: { choose($0) })) {
                ForEach(BrowserLayout.allCases, id: \.self) { layout in
                    if layout == .columns && !wide {
                        Label("Columns (turn sideways)", systemImage: layout.systemImage).tag(layout)
                    } else {
                        Label(layout.label, systemImage: layout.systemImage).tag(layout)
                    }
                }
            }
            if shown == .grid || shown == .masonry {
                Picker("Tile size", selection: $tile) {
                    ForEach(tileLabels.indices, id: \.self) { Text("\(tileLabels[$0]) tiles").tag($0) }
                }
            }
        } label: {
            Image(systemName: shown.systemImage)
        }
        .accessibilityLabel("View: \(shown.label)")
    }
}

/// A thumbnail with its own shape (for Masonry), from the same cache as the list's.
struct ThumbnailImage: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let entry: Entry
    /// Keeps the photo's shape instead of filling a square.
    let natural: Bool
    @State private var image: UIImage?

    private var key: String { "\(server):\(entry.path):\(entry.modified)" }

    var body: some View {
        let shown = image ?? Thumbnails.shared.cached(key)
        Group {
            if let shown {
                if natural {
                    Image(uiImage: shown).resizable().aspectRatio(shown.size.width / max(shown.size.height, 1), contentMode: .fit)
                } else {
                    Color.clear.aspectRatio(1, contentMode: .fit).overlay { Image(uiImage: shown).resizable().scaledToFill() }.clipped()
                }
            } else {
                Color(uiColor: .tertiarySystemFill)
                    .aspectRatio(1, contentMode: .fit)
                    .overlay {
                        Image(systemName: symbol(for: entry))
                            .font(.system(size: 34))
                            .foregroundStyle(entry.dir ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
                    }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .task(id: key) {
            guard entry.thumb, image == nil, Thumbnails.shared.cached(key) == nil else { return }
            let path = entry.path
            image = await Thumbnails.shared.load(hub, server: server, key: key) { try $0.thumbnail(path: path) }
        }
    }
}

/// One entry as a tile: a large thumbnail or the kind's symbol, the name, and its details.
struct EntryTile: View {
    let server: String
    let entry: Entry
    let natural: Bool
    let chosen: Bool
    let selecting: Bool
    let details: String
    let terminalMark: Color?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ThumbnailImage(server: server, entry: entry, natural: natural)
                .overlay(alignment: .topLeading) {
                    if selecting {
                        Image(systemName: chosen ? "checkmark.circle.fill" : "circle")
                            .font(.title3)
                            .foregroundStyle(chosen ? AnyShapeStyle(.tint) : AnyShapeStyle(.white))
                            .background(Circle().fill(.black.opacity(chosen ? 0 : 0.25)))
                            .padding(6)
                    }
                }
                .overlay(alignment: .bottomTrailing) {
                    if let terminalMark {
                        Image(systemName: "terminal.fill").font(.system(size: 10)).padding(4)
                            .background(Circle().fill(terminalMark)).foregroundStyle(.white).padding(4)
                    }
                }
            Text(specialFolderLabel(entry.name)).font(.footnote).lineLimit(2).foregroundStyle(.primary)
            Text(details).font(.caption2).lineLimit(1).foregroundStyle(.secondary)
        }
        .padding(6)
        .background(RoundedRectangle(cornerRadius: 12).fill(chosen ? AnyShapeStyle(Color.accentColor.opacity(0.18)) : AnyShapeStyle(.clear)))
        .contentShape(RoundedRectangle(cornerRadius: 12))
    }
}

/// Splits entries into `count` columns, in reading order, for Masonry.
func masonryColumns<T>(_ items: [T], count: Int) -> [[T]] {
    var columns = Array(repeating: [T](), count: max(count, 1))
    for (index, item) in items.enumerated() { columns[index % columns.count].append(item) }
    return columns
}

/// The folders above `path`, from the top: "Docs/sub" → "", "Docs".
func ancestorPaths(_ path: String) -> [String] {
    guard !path.isEmpty else { return [] }
    let parts = path.split(separator: "/").map(String.init)
    return [""] + (0..<(parts.count - 1)).map { parts[0...$0].joined(separator: "/") }
}

/// A folder above the one shown, in the column view, with the way down highlighted.
struct AboveColumn: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let serverName: String
    let path: String
    /// The entry in this folder that leads to the folder shown.
    let next: String
    let open: (Entry) -> Void
    @State private var entries: [Entry]?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(path.isEmpty ? serverName : specialFolderLabel((path as NSString).lastPathComponent))
                .font(.caption.weight(.semibold)).foregroundStyle(.secondary).lineLimit(1)
                .padding(.horizontal, 12).padding(.top, 6).padding(.bottom, 2)
            if let entries {
                ScrollViewReader { scroller in
                    List(entries, id: \.path) { entry in
                        Button { open(entry) } label: {
                            HStack(spacing: 8) {
                                EntryIcon(server: server, entry: entry, size: 24)
                                Text(specialFolderLabel(entry.name)).font(.subheadline).lineLimit(1).foregroundStyle(.primary)
                                Spacer(minLength: 0)
                                if entry.dir { Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary) }
                            }
                        }
                        .listRowBackground(entry.path == next ? Color.accentColor.opacity(0.18) : Color.clear)
                        .listRowInsets(EdgeInsets(top: 4, leading: 10, bottom: 4, trailing: 10))
                        .id(entry.path)
                    }
                    .listStyle(.plain)
                    .environment(\.defaultMinListRowHeight, 32)
                    .onAppear { scroller.scrollTo(next, anchor: .center) }
                }
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task(id: path) {
            let folder = path
            if let listed = try? await hub.call(server, { try $0.list(path: folder) }) {
                entries = listed.entries.sorted { a, b in
                    if a.dir != b.dir { return a.dir }
                    return a.name.localizedStandardCompare(b.name) == .orderedAscending
                }
            }
        }
    }
}
