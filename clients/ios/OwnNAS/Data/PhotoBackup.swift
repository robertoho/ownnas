import BackgroundTasks
import Foundation
import Network
import Photos
import UIKit

/// Backing this phone's photos and videos up to a server's media upload folder (one way: nothing
/// on the phone or the server is ever deleted). Each photo is sent as taken (the original), to
/// `<folder>/<this phone>/<year>/<month>/`, and the server skips what it has already.
///
/// It runs when the app opens, when new photos appear while it is open, with Back up now, and
/// when iOS gives the app background time (usually at night while charging).
@MainActor
final class PhotoBackup: NSObject, ObservableObject {
    struct Settings: Codable, Equatable {
        /// The server to back up to, or nil when backups are off.
        var serverId: String?
        var wifiOnly = true
        var videos = true
        /// Halted until resumed (the server choice is kept).
        var paused = false
        /// The folder on the server for this phone (the phone's name when empty).
        var deviceName = ""
    }

    struct State: Codable {
        var running = false
        var done = 0
        var total = 0
        /// Photos and videos on the server: sent by this phone, or found already there.
        var backedUp = 0
        /// Still to send, as of the last run.
        var left = 0
        var lastRun: Date?
        var lastResult: String?
        var current: String?
        var runStarted: Date?
        var runBytes: Int64 = 0
        var lastDuration: TimeInterval = 0
        var lastBytes: Int64 = 0
        var lastSent = 0
        var totalBytes: Int64 = 0
        var totalSent = 0
    }

    /// A photo or video a run sent, or found on the server already.
    struct Item: Identifiable {
        enum Status: Int { case sent = 0, skipped = 1, blocked = 2 }
        let asset: String
        let name: String
        let size: Int64
        let at: Date
        let status: Status
        var id: String { "\(asset)\(at.timeIntervalSince1970)" }
    }

    static let taskId = "app.ownnas.ios.backup"
    private static let recentLimit = 500

    @Published private(set) var settings: Settings
    @Published private(set) var state: State
    /// The server's media upload folder, once asked.
    @Published private(set) var folder: String?
    @Published private(set) var access = PHPhotoLibrary.authorizationStatus(for: .readWrite)

    private unowned let hub: Hub
    private let defaults = UserDefaults.standard
    private let files: URL
    private var stopFlag = StopFlag()
    private let network = NetworkWatch()
    private var observing = false
    private var changeTask: Task<Void, Never>?
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid
    /// A run is under way (set before its first wait, so two never overlap).
    private var busy = false

    init(hub: Hub) {
        self.hub = hub
        files = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("backup", isDirectory: true)
        try? FileManager.default.createDirectory(at: files, withIntermediateDirectories: true)
        settings = (defaults.data(forKey: "backup.settings")).flatMap { try? JSONDecoder().decode(Settings.self, from: $0) } ?? Settings()
        var saved = (defaults.data(forKey: "backup.state")).flatMap { try? JSONDecoder().decode(State.self, from: $0) } ?? State()
        saved.running = false
        saved.current = nil
        state = saved
        super.init()
        state.backedUp = doneKeys().count
        if settings.deviceName.isEmpty { settings.deviceName = UIDevice.current.name }
        observeLibrary()
    }

    var enabled: Bool { settings.serverId != nil && !settings.paused }

    // MARK: Settings

    func update(_ change: (inout Settings) -> Void) {
        var next = settings
        change(&next)
        next.deviceName = next.deviceName.trimmingCharacters(in: .whitespacesAndNewlines)
        if next.deviceName.isEmpty { next.deviceName = UIDevice.current.name }
        // Another server keeps its own copies: start counting again.
        if next.serverId != settings.serverId {
            stop()
            for name in ["done.txt", "blocked.txt", "recent.tsv"] { try? FileManager.default.removeItem(at: files.appendingPathComponent(name)) }
            defaults.removeObject(forKey: "backup.blockedOnServer")
            state = State()
            folder = nil
        }
        if next.paused && !settings.paused { stop() }
        settings = next
        defaults.set(try? JSONEncoder().encode(next), forKey: "backup.settings")
        saveState()
        scheduleBackground()
    }

    /// Backs up to a server from now on: asks to see the photos if needed, and starts.
    func turnOn(_ serverId: String) async {
        update { $0.serverId = serverId }
        if access == .notDetermined { await askForAccess() }
        backUpNow()
    }

    func forgetServer(_ id: String) {
        if settings.serverId == id { update { $0.serverId = nil } }
    }

    func pause() { update { $0.paused = true } }

    func resume() {
        update { $0.paused = false }
        backUpNow()
    }

    // MARK: Photos access

    func askForAccess() async {
        access = await PHPhotoLibrary.requestAuthorization(for: .readWrite)
        observeLibrary()
        if canRead { backUpNow() }
    }

    func refreshAccess() {
        access = PHPhotoLibrary.authorizationStatus(for: .readWrite)
        observeLibrary()
    }

    private var canRead: Bool { access == .authorized || access == .limited }

    /// Runs a backup soon after new photos appear while the app is open.
    private func observeLibrary() {
        guard canRead, !observing else { return }
        observing = true
        PHPhotoLibrary.shared().register(self)
    }

    // MARK: Running

    func backUpNow() {
        Task { await run() }
    }

    /// When the app comes to the front: a backup unless one ran in the last few minutes.
    func appBecameActive() {
        refreshAccess()
        if let last = state.lastRun, Date().timeIntervalSince(last) < 5 * 60, state.lastResult == "Up to date" { return }
        backUpNow()
    }

    /// When the app leaves the front: a running backup gets a little time to finish its file,
    /// and iOS is asked for background time later.
    func appWentToBackground() {
        scheduleBackground()
        guard state.running, backgroundTask == .invalid else { return }
        backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Photo backup") { [weak self] in
            Task { @MainActor in
                self?.stop()
                self?.endBackgroundTask()
            }
        }
    }

    private func endBackgroundTask() {
        if backgroundTask != .invalid {
            UIApplication.shared.endBackgroundTask(backgroundTask)
            backgroundTask = .invalid
        }
    }

    /// Stops a running backup after the file being sent.
    func stop() {
        stopFlag.set()
    }

    func run() async {
        guard !busy, let serverId = settings.serverId, !settings.paused else { return }
        busy = true
        defer { busy = false }
        refreshAccess()
        guard canRead else {
            finish("Allow OwnNAS to see your photos to back them up")
            return
        }
        if settings.wifiOnly && network.expensive {
            finish("Waiting for Wi-Fi")
            return
        }
        let flag = StopFlag()
        stopFlag = flag
        let folder: String?
        do {
            folder = try await hub.call(serverId) { try $0.backupFolder() }
        } catch ClientError.Unauthorized {
            finish("Sign in to the server again to keep backing up")
            return
        } catch {
            finish(userMessage(error))
            return
        }
        self.folder = folder
        guard folder != nil else {
            finish("The server has no media upload folder yet. An administrator can set one in the web app: Settings → Gallery → Phone backups.")
            return
        }
        if let onServer = try? await hub.call(serverId, { try $0.blockedPhotos().count }) { checkBlocked(onServer) }
        let skip = doneKeys().union(blockedKeys())
        let pending = assets().filter { !skip.contains($0.localIdentifier) }
        guard !pending.isEmpty else {
            finish("Up to date", left: 0)
            return
        }
        start(pending.count)
        let device = settings.deviceName
        var added = 0, failed = 0, sent = 0, blocked = 0
        var bytes: Int64 = 0
        var firstProblem: String?
        var stoppedFor: String?
        for (index, asset) in pending.enumerated() {
            if flag.isSet || settings.paused { break }
            if settings.wifiOnly && network.expensive {
                stoppedFor = "Waiting for Wi-Fi"
                break
            }
            guard let resource = Self.original(of: asset) else {
                state.done = index + 1
                continue
            }
            let name = resource.originalFilename
            state.current = name
            do {
                let file = try await Self.export(resource)
                defer { try? FileManager.default.removeItem(at: file) }
                let size = (try? FileManager.default.attributesOfItem(atPath: file.path)[.size] as? NSNumber)?.int64Value ?? 0
                let fd = open(file.path, O_RDONLY)
                guard fd >= 0 else { throw ClientError.Failed(reason: "Could not read \(name)") }
                let taken = Int64(((asset.creationDate ?? asset.modificationDate ?? Date()).timeIntervalSince1970 * 1000).rounded())
                let sentBefore = bytes
                let progress = BackupProgress(flag: flag) { [weak self] done in
                    Task { @MainActor in self?.state.runBytes = sentBefore + Int64(done) }
                }
                let result = try await hub.call(serverId) {
                    try $0.backupFd(device: device, name: name, fd: fd, size: UInt64(max(size, 0)), mtimeMs: taken, transfer: progress)
                }
                if result.blocked { markBlocked(asset.localIdentifier) } else { markDone(asset.localIdentifier) }
                noteRecent(asset: asset.localIdentifier, name: name, size: size, status: result.blocked ? .blocked : result.skipped ? .skipped : .sent)
                sent += 1
                if result.blocked { blocked += 1 }
                if !result.skipped {
                    added += 1
                    bytes += size
                }
                state.runBytes = bytes
            } catch {
                if flag.isSet || settings.paused || isCancel(error) { break }
                failed += 1
                if firstProblem == nil { firstProblem = "\(name): \(userMessage(error))" }
                // Signed out, or the server or network keeps failing: try again later instead
                // of failing every photo. A single unreadable photo is just skipped.
                if case ClientError.Unauthorized = error {
                    stoppedFor = "Sign in to the server again to keep backing up"
                    break
                }
                if failed >= 5 {
                    stoppedFor = "Stopped after \(index) of \(pending.count): \(userMessage(error))"
                    break
                }
            }
            state.done = index + 1
        }
        var result: String
        if settings.paused { result = "Paused" }
        else if let stoppedFor { result = stoppedFor }
        else if flag.isSet { result = "Interrupted; it carries on later" }
        else if failed > 0 { result = "Backed up \(added) new; \(failed) could not be sent (\(firstProblem ?? ""))" }
        else { result = "Backed up \(added) new photo\(added == 1 ? "" : "s")" }
        if blocked > 0 { result += "; skipped \(blocked) deleted on the server" }
        finish(result, left: pending.count - sent, sent: added, bytes: bytes)
        endBackgroundTask()
    }

    /// The phone's photos (and videos), newest first.
    private func assets() -> [PHAsset] {
        let options = PHFetchOptions()
        options.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
        options.includeAssetSourceTypes = [.typeUserLibrary, .typeiTunesSynced, .typeCloudShared]
        if !settings.videos { options.predicate = NSPredicate(format: "mediaType == %d", PHAssetMediaType.image.rawValue) }
        else { options.predicate = NSPredicate(format: "mediaType == %d || mediaType == %d", PHAssetMediaType.image.rawValue, PHAssetMediaType.video.rawValue) }
        let result = PHAsset.fetchAssets(with: options)
        var list: [PHAsset] = []
        list.reserveCapacity(result.count)
        result.enumerateObjects { asset, _, _ in list.append(asset) }
        return list
    }

    /// The photo or video as it was taken (not an edited copy).
    nonisolated private static func original(of asset: PHAsset) -> PHAssetResource? {
        let resources = PHAssetResource.assetResources(for: asset)
        let wanted: [PHAssetResourceType] = asset.mediaType == .video ? [.video, .fullSizeVideo] : [.photo, .fullSizePhoto]
        for type in wanted {
            if let found = resources.first(where: { $0.type == type }) { return found }
        }
        return resources.first
    }

    /// Writes a photo to a temporary file, downloading it from iCloud when needed.
    nonisolated private static func export(_ resource: PHAssetResource) async throws -> URL {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("backup", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let file = folder.appendingPathComponent(UUID().uuidString + "-" + resource.originalFilename)
        let options = PHAssetResourceRequestOptions()
        options.isNetworkAccessAllowed = true
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            PHAssetResourceManager.default().writeData(for: resource, toFile: file, options: options) { error in
                if let error { continuation.resume(throwing: ClientError.Failed(reason: "Could not read the photo: \(error.localizedDescription)")) } else { continuation.resume() }
            }
        }
        return file
    }

    // MARK: Background time

    /// Lets iOS run a backup in the background now and then (it chooses when).
    func registerBackgroundTask() {
        BGTaskScheduler.shared.register(forTaskWithIdentifier: Self.taskId, using: .main) { [weak self] task in
            MainActor.assumeIsolated {
                guard let self else {
                    task.setTaskCompleted(success: true)
                    return
                }
                self.scheduleBackground()
                task.expirationHandler = { Task { @MainActor in self.stop() } }
                Task { @MainActor in
                    await self.run()
                    task.setTaskCompleted(success: true)
                }
            }
        }
    }

    func scheduleBackground() {
        guard enabled else {
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: Self.taskId)
            return
        }
        let request = BGProcessingTaskRequest(identifier: Self.taskId)
        request.requiresNetworkConnectivity = true
        request.earliestBeginDate = Date(timeIntervalSinceNow: 60 * 60)
        try? BGTaskScheduler.shared.submit(request)
    }

    // MARK: State

    private func start(_ total: Int) {
        state.running = true
        state.done = 0
        state.total = total
        state.left = total
        state.runStarted = Date()
        state.runBytes = 0
        state.current = nil
        saveState()
    }

    private func finish(_ result: String, left: Int? = nil, sent: Int = 0, bytes: Int64 = 0) {
        let ran = state.running
        state.lastRun = Date()
        state.lastResult = result
        if let left { state.left = max(left, 0) }
        if ran {
            state.lastDuration = Date().timeIntervalSince(state.runStarted ?? Date())
            state.lastBytes = bytes
            state.lastSent = sent
            state.totalBytes += bytes
            state.totalSent += sent
        }
        state.running = false
        state.current = nil
        state.backedUp = doneKeys().count
        trimRecent()
        saveState()
    }

    private func saveState() {
        defaults.set(try? JSONEncoder().encode(state), forKey: "backup.state")
    }

    private func lines(_ name: String) -> [String] {
        ((try? String(contentsOf: files.appendingPathComponent(name), encoding: .utf8)) ?? "").split(separator: "\n").map(String.init)
    }

    private func append(_ name: String, _ line: String) {
        let url = files.appendingPathComponent(name)
        if let handle = try? FileHandle(forWritingTo: url) {
            handle.seekToEndOfFile()
            handle.write(Data((line + "\n").utf8))
            try? handle.close()
        } else {
            try? Data((line + "\n").utf8).write(to: url)
        }
    }

    /// The photos on the server already, by their id in the phone's library.
    private func doneKeys() -> Set<String> { Set(lines("done.txt")) }

    /// Photos the server said were deleted there: not sent, and not counted as backed up.
    private func blockedKeys() -> Set<String> { Set(lines("blocked.txt")) }

    private func markDone(_ key: String) {
        append("done.txt", key)
        state.backedUp += 1
        state.left = max(state.left - 1, 0)
    }

    private func markBlocked(_ key: String) {
        append("blocked.txt", key)
        state.left = max(state.left - 1, 0)
    }

    /// Photos allowed again on the server: ask about every blocked one on the next run.
    func forgetBlocked() {
        try? FileManager.default.removeItem(at: files.appendingPathComponent("blocked.txt"))
        defaults.removeObject(forKey: "backup.blockedOnServer")
    }

    /// Notices photos allowed again on the server (the web app, or another phone).
    private func checkBlocked(_ onServer: Int) {
        if onServer < defaults.integer(forKey: "backup.blockedOnServer") {
            try? FileManager.default.removeItem(at: files.appendingPathComponent("blocked.txt"))
        }
        defaults.set(onServer, forKey: "backup.blockedOnServer")
    }

    private func noteRecent(asset: String, name: String, size: Int64, status: Item.Status) {
        let clean = name.replacingOccurrences(of: "\t", with: " ").replacingOccurrences(of: "\n", with: " ")
        append("recent.tsv", "\(asset)\t\(clean)\t\(size)\t\(Date().timeIntervalSince1970)\t\(status.rawValue)")
    }

    private func trimRecent() {
        let all = lines("recent.tsv")
        if all.count > Self.recentLimit * 2 {
            try? Data(all.suffix(Self.recentLimit).map { $0 + "\n" }.joined().utf8).write(to: files.appendingPathComponent("recent.tsv"))
        }
    }

    /// The photos and videos runs handled lately, newest first.
    func recent() -> [Item] {
        lines("recent.tsv").reversed().compactMap { line in
            let parts = line.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
            guard parts.count >= 5 else { return nil }
            return Item(asset: parts[0], name: parts[1], size: Int64(parts[2]) ?? 0, at: Date(timeIntervalSince1970: Double(parts[3]) ?? 0), status: Item.Status(rawValue: Int(parts[4]) ?? 0) ?? .sent)
        }
    }
}

extension PhotoBackup: PHPhotoLibraryChangeObserver {
    nonisolated func photoLibraryDidChange(_ changeInstance: PHChange) {
        Task { @MainActor in
            // New photos arrive in bursts: wait for them to settle.
            self.changeTask?.cancel()
            self.changeTask = Task { @MainActor in
                try? await Task.sleep(for: .seconds(30))
                guard !Task.isCancelled, UIApplication.shared.applicationState == .active else { return }
                await self.run()
            }
        }
    }
}

/// Set when a backup should stop; read from the upload's thread.
final class StopFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false

    var isSet: Bool {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func set() {
        lock.lock()
        value = true
        lock.unlock()
    }
}

/// An upload's progress, a few times a second, and whether to carry on.
private final class BackupProgress: Transfer, @unchecked Sendable {
    private let flag: StopFlag
    private let report: (UInt64) -> Void
    private let lock = NSLock()
    private var shown = Date.distantPast

    init(flag: StopFlag, report: @escaping (UInt64) -> Void) {
        self.flag = flag
        self.report = report
    }

    func keepGoing(done: UInt64) -> Bool {
        lock.lock()
        let now = Date()
        let due = now.timeIntervalSince(shown) > 0.3
        if due { shown = now }
        lock.unlock()
        if due { report(done) }
        return !flag.isSet
    }
}

/// Whether the network costs money (cellular, a hotspot, or Low Data Mode).
private final class NetworkWatch: @unchecked Sendable {
    private let monitor = NWPathMonitor()
    private let lock = NSLock()
    private var path: NWPath?

    init() {
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            self.lock.lock()
            self.path = path
            self.lock.unlock()
        }
        monitor.start(queue: DispatchQueue(label: "app.ownnas.network"))
    }

    var expensive: Bool {
        lock.lock()
        defer { lock.unlock() }
        guard let path else { return false }
        return path.isExpensive || path.isConstrained
    }
}
