import Photos
import SwiftUI

/// Backing this phone's photos up to a server: where to, how, what happened, and the photos
/// deleted on the server that are not backed up again.
struct BackupView: View {
    @EnvironmentObject private var hub: Hub
    @EnvironmentObject private var backup: PhotoBackup
    @State private var recent: [PhotoBackup.Item] = []
    @State private var deviceName = ""

    private var signedIn: [SavedServer] { hub.servers.filter { hub.signedIn($0.id) } }

    var body: some View {
        List {
            Section {
                Button { backup.update { $0.serverId = nil } } label: { choice("Off", selected: backup.settings.serverId == nil) }
                ForEach(signedIn) { server in
                    Button { Task { await backup.turnOn(server.id) } } label: {
                        choice("\(server.name) · \(server.host)", selected: backup.settings.serverId == server.id)
                    }
                }
            } header: {
                Text("Back up to")
            } footer: {
                if signedIn.isEmpty { Text("Sign in to a server first.") }
            }

            if let serverId = backup.settings.serverId {
                if backup.access != .authorized && backup.access != .limited {
                    Section {
                        Text("OwnNAS needs to see your photos and videos to back them up.")
                        if backup.access == .notDetermined {
                            Button("Allow access") { Task { await backup.askForAccess() } }
                        } else {
                            Button("Open Settings") { if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) } }
                        }
                    }
                } else if backup.access == .limited {
                    Section {
                        Text("OwnNAS can see only the photos you chose, so only those are backed up.").font(.footnote)
                        Button("Choose more photos") { chooseMore() }
                        Button("Allow all photos") { if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) } }
                    }
                }
                status
                Section {
                    Toggle("Only on Wi-Fi", isOn: Binding(get: { backup.settings.wifiOnly }, set: { value in backup.update { $0.wifiOnly = value } }))
                    Toggle("Videos too", isOn: Binding(get: { backup.settings.videos }, set: { value in backup.update { $0.videos = value } }))
                    LabeledContent("Folder for this phone") {
                        TextField("iPhone", text: $deviceName)
                            .multilineTextAlignment(.trailing)
                            .onSubmit { backup.update { $0.deviceName = deviceName } }
                    }
                } footer: {
                    Text("Photos are sent as taken, newest first. Nothing on this phone is changed or deleted, and photos the server has already are not sent again. iOS decides when the app may back up in the background, usually at night while charging; opening the app backs up right away.")
                }
                stats
                recentSection
                BlockedSection(server: serverId)
            }
        }
        .navigationTitle("Photo backup")
        .onAppear {
            deviceName = backup.settings.deviceName
            recent = backup.recent()
            backup.refreshAccess()
        }
        .onChange(of: backup.state.running) { recent = backup.recent() }
        .onChange(of: backup.state.backedUp) { recent = backup.recent() }
        .onChange(of: backup.settings.serverId) { recent = backup.recent() }
        .refreshable { backup.backUpNow() }
    }

    private func choice(_ label: String, selected: Bool) -> some View {
        HStack {
            Text(label).foregroundStyle(.primary)
            Spacer()
            if selected { Image(systemName: "checkmark").foregroundStyle(.tint) }
        }
    }

    private var status: some View {
        let state = backup.state
        return Section {
            if let folder = backup.folder {
                Label("Saved to “\(folder)/\(backup.settings.deviceName)”", systemImage: "folder").font(.subheadline)
            }
            if state.running {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Backing up \(state.done) of \(state.total)…")
                    ProgressView(value: Double(state.done), total: Double(max(state.total, 1)))
                    if let current = state.current {
                        Text("\(current) · \(ByteCountFormatter.string(fromByteCount: state.runBytes, countStyle: .file)) sent")
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
            } else if backup.settings.paused {
                Text("Backups are paused. Nothing is sent until you resume.")
            } else if let result = state.lastResult {
                VStack(alignment: .leading, spacing: 4) {
                    Text(result)
                    if let last = state.lastRun {
                        Text(last, format: .relative(presentation: .named)).font(.caption).foregroundStyle(.secondary)
                    }
                }
            } else {
                Text("Not backed up yet.")
            }
            HStack {
                if backup.settings.paused {
                    Button("Resume", systemImage: "play.fill") { backup.resume() }
                } else {
                    Button("Back up now", systemImage: "arrow.up.circle") { backup.backUpNow() }.disabled(state.running)
                    Spacer()
                    Button("Pause", systemImage: "pause.fill") { backup.pause() }
                }
            }
            .buttonStyle(.borderless)
        }
    }

    private var stats: some View {
        let state = backup.state
        let bytes = { (count: Int64) in ByteCountFormatter.string(fromByteCount: count, countStyle: .file) }
        return Section("Statistics") {
            LabeledContent("On the server", value: "\(state.backedUp)")
            LabeledContent("Still to send", value: "\(state.left)")
            if state.lastRun != nil, state.lastDuration > 0 {
                let took = Duration.seconds(state.lastDuration).formatted(.units(allowed: [.hours, .minutes, .seconds], width: .abbreviated, maximumUnitCount: 2))
                LabeledContent("Last run", value: state.lastSent == 0 ? "Nothing new, in \(took)" : "\(state.lastSent) sent, \(bytes(state.lastBytes)), in \(took)")
            }
            LabeledContent("Sent from this phone", value: state.totalSent == 0 ? "None yet" : "\(state.totalSent), \(bytes(state.totalBytes))")
        }
    }

    @ViewBuilder
    private var recentSection: some View {
        if !recent.isEmpty {
            Section("Recently backed up") {
                ForEach(recent.prefix(60)) { item in
                    HStack(spacing: 12) {
                        AssetThumbnail(id: item.asset).frame(width: 44, height: 44).clipShape(RoundedRectangle(cornerRadius: 6))
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.name).lineLimit(1)
                            Text(detail(item)).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            }
        }
    }

    private func detail(_ item: PhotoBackup.Item) -> String {
        let what: String
        switch item.status {
        case .sent: what = ByteCountFormatter.string(fromByteCount: item.size, countStyle: .file)
        case .skipped: what = "Already on the server"
        case .blocked: what = "Deleted on the server; not sent"
        }
        return "\(what) · \(item.at.formatted(.relative(presentation: .named)))"
    }

    private func chooseMore() {
        guard let scene = UIApplication.shared.connectedScenes.first as? UIWindowScene,
              let root = scene.keyWindow?.rootViewController else { return }
        var top = root
        while let shown = top.presentedViewController { top = shown }
        PHPhotoLibrary.shared().presentLimitedLibraryPicker(from: top) { _ in
            Task { @MainActor in backup.backUpNow() }
        }
    }
}

/// A photo in this phone's library, small.
private struct AssetThumbnail: View {
    let id: String
    @State private var image: UIImage?

    var body: some View {
        Color(.secondarySystemBackground)
            .overlay {
                if let image { Image(uiImage: image).resizable().scaledToFill() } else { Image(systemName: "photo").foregroundStyle(.secondary) }
            }
            .clipped()
            .task(id: id) {
                guard let asset = PHAsset.fetchAssets(withLocalIdentifiers: [id], options: nil).firstObject else { return }
                let options = PHImageRequestOptions()
                options.deliveryMode = .opportunistic
                options.isNetworkAccessAllowed = false
                PHImageManager.default().requestImage(for: asset, targetSize: CGSize(width: 120, height: 120), contentMode: .aspectFill, options: options) { picture, _ in
                    if let picture { image = picture }
                }
            }
    }
}

/// Photos deleted on the server, which phones don't back up again unless allowed.
private struct BlockedSection: View {
    @EnvironmentObject private var hub: Hub
    @EnvironmentObject private var backup: PhotoBackup
    let server: String
    @State private var photos: [BlockedPhoto] = []
    @State private var problem: String?

    var body: some View {
        Group {
            if !photos.isEmpty {
                Section {
                    ForEach(photos, id: \.sha256) { photo in
                        HStack(spacing: 12) {
                            BlockedThumbnail(server: server, sha256: photo.sha256).frame(width: 44, height: 44).clipShape(RoundedRectangle(cornerRadius: 6))
                            VStack(alignment: .leading, spacing: 2) {
                                Text(photo.name).lineLimit(1)
                                Text("Deleted \(Date(timeIntervalSince1970: Double(photo.blockedAt)).formatted(date: .abbreviated, time: .omitted))\(photo.blockedBy.isEmpty ? "" : " by \(photo.blockedBy)")")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("Allow again") { Task { await allow(photo) } }.buttonStyle(.borderless).font(.subheadline)
                        }
                    }
                } header: {
                    Text("Deleted on the server")
                } footer: {
                    Text("Phones don't back these up again. Allow one to back it up on the next run.")
                }
            }
            if let problem {
                Section { Text(problem).foregroundStyle(.secondary).font(.footnote) }
            }
        }
        .task(id: server) { await load() }
        .onChange(of: backup.state.running) { if !backup.state.running { Task { await load() } } }
    }

    private func load() async {
        do {
            photos = try await hub.call(server) { try $0.blockedPhotos() }
            problem = nil
        } catch {
            problem = userMessage(error)
        }
    }

    private func allow(_ photo: BlockedPhoto) async {
        do {
            try await hub.call(server) { try $0.unblockPhoto(sha256: photo.sha256) }
            backup.forgetBlocked()
            photos.removeAll { $0.sha256 == photo.sha256 }
            backup.backUpNow()
        } catch {
            problem = userMessage(error)
        }
    }
}

private struct BlockedThumbnail: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let sha256: String
    @State private var image: UIImage?

    var body: some View {
        Color(.secondarySystemBackground)
            .overlay {
                if let image { Image(uiImage: image).resizable().scaledToFill() } else { Image(systemName: "photo").foregroundStyle(.secondary) }
            }
            .clipped()
            .task(id: sha256) {
                if let data = try? await hub.call(server, { try $0.blockedPreview(sha256: sha256) }) { image = UIImage(data: data) }
            }
    }
}
