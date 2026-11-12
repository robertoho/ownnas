package app.ownnas.android.ui

import android.app.Application
import android.net.Uri
import android.webkit.MimeTypeMap
import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import app.ownnas.android.OwnNasApp
import app.ownnas.android.data.SavedServer
import app.ownnas.android.data.TerminalShortcut
import app.ownnas.android.data.Trust
import app.ownnas.core.Account
import app.ownnas.core.ClientException
import app.ownnas.core.Conflict
import app.ownnas.core.Entry
import app.ownnas.core.UploadResult
import app.ownnas.core.Folder
import app.ownnas.core.Server
import app.ownnas.core.serverFingerprint
import app.ownnas.core.TerminalInfo
import app.ownnas.core.TerminalListener
import app.ownnas.core.TerminalSession
import app.ownnas.core.normalizeAddress
import app.ownnas.core.signIn
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext
import java.io.File
import java.util.concurrent.ConcurrentHashMap

sealed interface Screen {
    data object Servers : Screen

    /** Add a server, or sign in again to [existing]. */
    data class SignIn(val existing: SavedServer? = null) : Screen

    data class Browse(val serverId: String, val path: String) : Screen

    /** Searching a folder and the folders inside it. */
    data class Search(val serverId: String, val path: String) : Screen

    /** The server's terminals; new ones open in `path`. */
    data class Terminals(val serverId: String, val path: String) : Screen

    /** `startup` runs once the shell is ready, for terminals opened by a shortcut. */
    data class Terminal(val serverId: String, val terminalId: String, val startup: String? = null) : Screen

    data object Sync : Screen

    /** Files with the same contents in a folder and below. */
    data class Duplicates(val serverId: String, val path: String) : Screen

    /** Sorting a folder's files into folders by date, kind, and so on. */
    data class SortFolder(val serverId: String, val path: String) : Screen

    /** What people did on the server lately. */
    data class Activity(val serverId: String) : Screen

    /** A server's Gallery: photos by date, albums, people, and tags. */
    data class Gallery(val serverId: String, val tab: String = "photos") : Screen

    data class GalleryAlbum(val serverId: String, val albumId: Long) : Screen

    data class GalleryPerson(val serverId: String, val personId: Long) : Screen

    data class GalleryTag(val serverId: String, val tag: String) : Screen

    /** Search results: photos that show `words`, or that look like the photo at `similarTo`. */
    data class GalleryResults(val serverId: String, val words: String? = null, val similarTo: String? = null) : Screen

    data class GalleryHidden(val serverId: String) : Screen

    /** The photos taken inside a box on the map, named `name`. */
    data class GalleryPlace(val serverId: String, val name: String, val west: Double, val south: Double, val east: Double, val north: Double) : Screen

    /** Photos full screen, swiping through the list the Gallery handed over ([AppViewModel.viewer]). */
    data class PhotoViewer(val serverId: String, val start: Int, val list: Long) : Screen

    /** Backing this phone's photos up to a server. */
    data object Backup : Screen
}

class AppViewModel(application: Application) : AndroidViewModel(application) {
    private val hub = (application as OwnNasApp).servers
    val syncs = (application as OwnNasApp).syncs
    val shortcuts = (application as OwnNasApp).shortcuts
    val backup = (application as OwnNasApp).backup
    private val store = hub.store
    val servers = store.servers

    /** The back stack; the last screen is showing. */
    val stack = mutableStateListOf<Screen>(Screen.Servers)

    private val folders = ConcurrentHashMap<Pair<String, String>, Folder>()
    private val thumbnails = object : LruCache<String, ImageBitmap>(THUMBNAIL_CACHE_BYTES) {
        override fun sizeOf(key: String, value: ImageBitmap) = value.width * value.height * 4
    }
    private val thumbnailSlots = Semaphore(4)

    /** Bumped after every change made on a server from this phone, so open folders reload. */
    val revision: StateFlow<Int> = hub.revision

    private val queue = ArrayDeque<PendingUpload>()
    private val uploadState = MutableStateFlow<UploadStatus?>(null)
    val uploads: StateFlow<UploadStatus?> = uploadState.asStateFlow()
    private var uploader: Job? = null
    private var uploadTracker: Tracker? = null
    private var lastUploadError: String? = null
    /** Items cut or copied in one folder, waiting to be pasted in another. */
    private val clipState = MutableStateFlow<Clip?>(null)
    val clipboard: StateFlow<Clip?> = clipState.asStateFlow()

    fun clip(items: Clip?) {
        clipState.value = items
    }

    private val conflictState = MutableStateFlow<ConflictPrompt?>(null)
    val conflict: StateFlow<ConflictPrompt?> = conflictState.asStateFlow()

    fun push(screen: Screen) {
        stack.add(screen)
    }

    /** False when there is nothing left to go back to. */
    fun pop(): Boolean {
        if (stack.size <= 1) return false
        stack.removeAt(stack.lastIndex)
        return true
    }

    fun server(id: String): SavedServer? = store.find(id)

    internal suspend fun <T> call(id: String, block: (Server) -> T): T = hub.call(id, block)

    fun galleryWorkspace(id: String): Long? = hub.galleryWorkspace(id)

    fun setGalleryWorkspace(id: String, workspace: Long?) = hub.setGalleryWorkspace(id, workspace)

    /** The photos the viewer swipes through, by list id (kept while the viewer is open). */
    val viewerLists = HashMap<Long, List<app.ownnas.core.MediaItem>>()
    private var nextList = 0L

    fun viewer(serverId: String, items: List<app.ownnas.core.MediaItem>, start: Int) {
        val id = nextList++
        viewerLists[id] = items
        push(Screen.PhotoViewer(serverId, start, id))
    }

    /** A thumbnail by library path. `null` when the server has none. */
    suspend fun thumbnailOf(id: String, path: String, key: String): ImageBitmap? = thumbnailSlots.withPermit {
        thumbnails.get(key) ?: runCatching {
            val bytes = call(id) { it.thumbnail(path) }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
        }.getOrNull()?.also { thumbnails.put(key, it) }
    }

    /** A face cut out of its photo, for the People screens. */
    suspend fun facePicture(id: String, faceId: Long): ImageBitmap? {
        val key = "face:$id:$faceId"
        return thumbnails.get(key) ?: thumbnailSlots.withPermit {
            runCatching {
                val bytes = call(id) { it.facePicture(faceId) }
                BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
            }.getOrNull()?.also { thumbnails.put(key, it) }
        }
    }

    /** The whole photo (downloaded once into the cache), scaled to at most `maxPx`. */
    suspend fun fullPhoto(id: String, path: String, size: Long, maxPx: Int): ImageBitmap? = withContext(Dispatchers.IO) {
        runCatching {
            val file = hub.cachedCopy(id, path, size, 0L, object : app.ownnas.core.Transfer {
                override fun keepGoing(done: ULong) = true
            })
            decodeScaled(file, maxPx)
        }.getOrNull()
    }

    suspend fun signIn(address: String, username: String, password: String, trust: Trust): SavedServer =
        withContext(Dispatchers.IO) { hub.signIn(address, username, password, trust) }

    /** The certificate fingerprint an `https://` server presents, before trusting it. */
    suspend fun fingerprint(address: String): String =
        withContext(Dispatchers.IO) { serverFingerprint(normalizeAddress(address)) }

    fun syncNow(pairId: String? = null) {
        viewModelScope.launch { syncs.run(pairId) }
    }

    fun remove(id: String) {
        folders.keys.removeAll { it.first == id }
        syncs.pairs.value.filter { it.serverId == id }.forEach { syncs.remove(it.id) }
        shortcuts.forgetServer(id)
        backup.forgetServer(id)
        viewModelScope.launch(Dispatchers.IO) { hub.remove(id) }
    }

    /** Who is signed in, and whether the server takes changes. Fetched once per app run. */
    suspend fun account(id: String): Account = withContext(Dispatchers.IO) { hub.account(id) }

    private suspend fun <T> change(id: String, dir: String, block: (Server) -> T): T =
        call(id, block).also { hub.changed(id, dir) }

    suspend fun createFolder(id: String, parent: String, name: String): String =
        change(id, parent) { it.createFolder(parent, name.trim()) }

    suspend fun rename(id: String, entry: Entry, name: String): String =
        change(id, entry.path.parentPath()) { it.rename(entry.path, name.trim()) }

    suspend fun trash(id: String, entry: Entry) {
        change(id, entry.path.parentPath()) { it.trash(entry.path) }
    }

    /** Moves into the folder `dest`; returns the new path. */
    suspend fun moveTo(id: String, entry: Entry, dest: String): String =
        change(id, entry.path.parentPath()) { it.moveTo(entry.path, dest) }.also { hub.changed(id, dest) }

    /** Copies into the folder `dest`, keeping both when the name is taken; returns the copy's path. */
    suspend fun copyTo(id: String, entry: Entry, dest: String): String =
        change(id, dest) { it.copyTo(entry.path, dest) }

    suspend fun duplicate(id: String, entry: Entry): String =
        change(id, entry.path.parentPath()) { it.duplicate(entry.path) }

    /** Puts an item in the Trash back where it was. */
    suspend fun restore(id: String, entry: Entry): String =
        change(id, entry.path.parentPath()) { it.restore(entry.path) }.also { hub.changed(id, it.parentPath()) }

    /** Packs items from the folder `dir` into an archive there; returns its path. */
    suspend fun compress(id: String, dir: String, entries: List<Entry>, format: String): String =
        change(id, dir) { server -> server.compress(entries.map { it.path }, format) }

    /** Unpacks an archive into a folder beside it. */
    suspend fun extract(id: String, entry: Entry): List<String> =
        change(id, entry.path.parentPath()) { it.extract(entry.path, false) }

    suspend fun addTag(id: String, entry: Entry, tag: String): String =
        call(id) { it.addTag(entry.path, tag.trim()) }

    /** Unpacks an archive loose into the folder it is in. */
    suspend fun extractHere(id: String, entry: Entry): List<String> =
        change(id, entry.path.parentPath()) { it.extract(entry.path, true) }

    suspend fun setProtection(id: String, dir: String, entries: List<Entry>, protect: Boolean, contents: Boolean) =
        change(id, dir) { server -> server.setProtection(entries.map { it.path }, protect, contents) }

    suspend fun convertImage(id: String, entry: Entry, format: String): String =
        change(id, entry.path.parentPath()) { it.convertImage(entry.path, format) }

    /** False when the photo was that small already. */
    suspend fun resizeImage(id: String, entry: Entry, choice: ResizeChoice): Boolean =
        change(id, entry.path.parentPath()) { it.resizeImage(entry.path, choice.percent?.toUInt(), choice.longest?.toUInt(), choice.replace) }

    suspend fun pdfMerge(id: String, entries: List<Entry>): String =
        change(id, entries.first().path.parentPath()) { server -> server.pdfMerge(entries.map { it.path }) }

    suspend fun pdfExtract(id: String, entry: Entry, pages: String, separate: Boolean): List<String> =
        change(id, entry.path.parentPath()) { it.pdfExtract(entry.path, pages, separate) }

    suspend fun pdfSplit(id: String, entry: Entry): List<String> =
        change(id, entry.path.parentPath()) { it.pdfSplit(entry.path) }

    suspend fun pdfRotate(id: String, entry: Entry, pages: String, degrees: Long): String =
        change(id, entry.path.parentPath()) { it.pdfRotate(entry.path, pages, degrees) }

    suspend fun emptyTrash(id: String, trash: String) {
        change(id, trash) { it.emptyTrash() }
    }

    /** Queues picked files; they upload one at a time, in order, while the app is open. */
    fun upload(pending: List<PendingUpload>) {
        if (pending.isEmpty()) return
        synchronized(queue) { queue.addAll(pending) }
        uploadState.update { status ->
            val base = status?.takeIf { it.active } ?: UploadStatus(0, 0, 0, 0, null, null, 0)
            base.copy(total = base.total + pending.size)
        }
        if (uploader?.isActive != true) uploader = viewModelScope.launch(Dispatchers.IO) { drain() }
    }

    private enum class Outcome { Saved, Skipped, Failed }

    private suspend fun drain() {
        lastUploadError = null
        val rest = HashMap<Long, Conflict>()
        while (true) {
            val next = synchronized(queue) { queue.removeFirstOrNull() } ?: break
            val tracker = Tracker { done -> uploadState.update { it?.copy(currentDone = done) } }
            uploadTracker = tracker
            uploadState.update { it?.copy(current = next.name, currentSize = next.size, currentDone = 0) }
            val outcome = try {
                uploadOne(next, tracker, rest)
            } catch (err: Exception) {
                if (!err.isCancel()) lastUploadError = "${next.name}: ${err.userMessage()}"
                Outcome.Failed
            } finally {
                runCatching { next.file.close() }
            }
            uploadState.update {
                it?.copy(
                    finished = it.finished + if (outcome == Outcome.Saved) 1 else 0,
                    skipped = it.skipped + if (outcome == Outcome.Skipped) 1 else 0,
                    failed = it.failed + if (outcome == Outcome.Failed) 1 else 0,
                )
            }
            if (outcome == Outcome.Saved) hub.changed(next.serverId, next.dir)
            if (tracker.cancelled) break
        }
        uploadTracker = null
        uploadState.update { it?.copy(current = null) }
    }

    /** Asks first when the name is taken, then uploads through the web app's endpoint. */
    private suspend fun uploadOne(next: PendingUpload, tracker: Tracker, rest: MutableMap<Long, Conflict>): Outcome {
        var choice: Conflict? = null
        val existing = call(next.serverId) { it.checkUpload(next.dir, next.name) }
        if (existing != null) {
            val prompt = ConflictPrompt(
                name = next.name,
                existing = existing,
                incomingSize = next.size,
                incomingModified = next.modified,
                more = synchronized(queue) { queue.any { it.batch == next.batch } },
            )
            choice = rest[next.batch]?.takeIf(prompt::allows) ?: run {
                conflictState.value = prompt
                val answer = try {
                    prompt.reply.await()
                } finally {
                    conflictState.value = null
                }
                if (answer == null) {
                    dropBatch(next.batch)
                    tracker.cancelled = true
                    return Outcome.Failed
                }
                if (answer.rest) rest[next.batch] = answer.choice
                answer.choice
            }
        }
        if (choice == Conflict.IGNORE) return Outcome.Skipped
        val result = call(next.serverId) {
            it.uploadFd(
                next.dir,
                next.name,
                next.file.detachFd(),
                next.size?.toULong(),
                next.modified,
                choice,
                tracker,
            )
        }
        return when (result) {
            UploadResult.Saved -> Outcome.Saved
            UploadResult.Skipped -> Outcome.Skipped
            // Something appeared at this name after the check; the file was already read.
            is UploadResult.Exists -> {
                lastUploadError = "${next.name}: an item with that name appeared meanwhile. Try again."
                Outcome.Failed
            }
        }
    }

    /** Drops the files still waiting from one pick, after its conflict question was cancelled. */
    private fun dropBatch(batch: Long) {
        val dropped = synchronized(queue) { queue.filter { it.batch == batch }.also { queue.removeAll(it) } }
        dropped.forEach { runCatching { it.file.close() } }
        uploadState.update { it?.copy(failed = it.failed + dropped.size) }
    }

    /** Stops the current upload and drops the ones still waiting. */
    fun cancelUploads() {
        val dropped = synchronized(queue) { queue.toList().also { queue.clear() } }
        dropped.forEach { runCatching { it.file.close() } }
        uploadTracker?.cancelled = true
        conflictState.value?.answer(null)
        uploadState.update { it?.copy(failed = it.failed + dropped.size) }
    }

    /** The summary to show once uploads finish, then forgets them. */
    fun takeUploadSummary(): String? {
        val status = uploadState.value?.takeIf { !it.active } ?: return null
        uploadState.value = null
        val uploaded = "Uploaded ${status.finished} ${if (status.finished == 1) "file" else "files"}" +
            if (status.skipped > 0) ", skipped ${status.skipped}" else ""
        return when {
            status.failed == 0 -> uploaded
            lastUploadError != null -> "$uploaded. ${status.failed} failed — $lastUploadError"
            else -> "$uploaded. ${status.failed} cancelled"
        }
    }

    suspend fun terminals(id: String): List<TerminalInfo> = call(id) { it.terminals() }

    suspend fun openTerminal(id: String, path: String): TerminalInfo = call(id) { it.openTerminal(path, 30u, 100u) }

    /**
     * Opens a shortcut: back into its terminal while that still runs (found by name), otherwise
     * a new terminal named after it, with the shortcut's command to run.
     */
    suspend fun launch(shortcut: TerminalShortcut): Screen.Terminal = call(shortcut.serverId) { server ->
        val running = server.terminals().firstOrNull { it.running && it.name == shortcut.name }
        if (running != null) {
            Screen.Terminal(shortcut.serverId, running.id)
        } else {
            val opened = server.openTerminal(shortcut.path, 30u, 100u)
            runCatching { server.renameTerminal(opened.id, shortcut.name) }
            Screen.Terminal(shortcut.serverId, opened.id, shortcut.command.takeIf { it.isNotBlank() })
        }
    }

    suspend fun renameTerminal(id: String, terminalId: String, name: String) {
        call(id) { it.renameTerminal(terminalId, name) }
    }

    /**
     * Saves a shortcut made from an open terminal and names that terminal after it, so the
     * shortcut goes straight back to it while it runs.
     */
    suspend fun saveShortcutFrom(terminalId: String, shortcut: TerminalShortcut) {
        shortcuts.save(shortcut)
        runCatching { renameTerminal(shortcut.serverId, terminalId, shortcut.name) }
    }

    suspend fun closeTerminal(id: String, terminalId: String) {
        call(id) { it.closeTerminal(terminalId) }
    }

    suspend fun unlockTerminals(id: String, password: String) {
        call(id) { it.unlockTerminals(password) }
    }

    /**
     * Uploads pasted or attached files into a terminal's temporary folder on the server and
     * returns their paths there, as the web terminal does with pasted screenshots.
     */
    suspend fun uploadToTerminal(id: String, terminalId: String, uris: List<Uri>): List<String> = withContext(Dispatchers.IO) {
        val resolver = getApplication<Application>().contentResolver
        val (pending, _) = resolver.pendingUploads(id, "", uris)
        if (pending.isEmpty()) throw ClientException.Failed("That could not be read")
        pending.map { item ->
            // Clipboard images often have a bare name like "image"; give them an extension.
            val name = if ('.' in item.name) {
                item.name
            } else {
                val extension = item.type?.let { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) }
                if (extension != null) "${item.name}.$extension" else item.name
            }
            try {
                call(id) { it.uploadToTerminal(terminalId, name, item.file.detachFd(), item.size?.toULong(), Tracker {}) }
            } finally {
                runCatching { item.file.close() }
            }
        }
    }

    suspend fun markTerminalSeen(id: String, terminalId: String) {
        runCatching { call(id) { it.markTerminalSeen(terminalId) } }
    }

    suspend fun connectTerminal(id: String, terminalId: String, listener: TerminalListener): TerminalSession =
        call(id) { it.connectTerminal(terminalId, 30u, 100u, listener) }

    /** The last listing of a folder, to show while it reloads. */
    fun cachedFolder(id: String, path: String): Folder? = folders[id to path]

    suspend fun list(id: String, path: String): Folder =
        call(id) { it.listWithHidden(path, showHidden.value) }.also { folders[id to path] = it }

    private val browserPrefs = application.getSharedPreferences("browser", android.content.Context.MODE_PRIVATE)
    private val hiddenState = MutableStateFlow(browserPrefs.getBoolean("hidden", false))
    /** Folders list hidden files (names starting with a dot), as the web app's toggle. */
    val showHidden: StateFlow<Boolean> = hiddenState.asStateFlow()

    fun setShowHidden(show: Boolean) {
        browserPrefs.edit().putBoolean("hidden", show).apply()
        hiddenState.value = show
    }

    /** Tells open folders (and the Files app) that `dir` changed. */
    fun changedFolder(id: String, dir: String) = hub.changed(id, dir)

    suspend fun folderUsage(id: String, path: String) = call(id) { it.folderUsage(path) }

    suspend fun bookmarks(id: String): List<String> = call(id) { it.bookmarks() }

    suspend fun setBookmark(id: String, path: String, marked: Boolean) = call(id) { it.setBookmark(path, marked) }

    suspend fun createFile(id: String, parent: String, name: String, kind: String): String =
        change(id, parent) { it.createFile(parent, name.trim(), kind) }

    /** A folder as a ZIP in the app's cache, ready to share. */
    suspend fun downloadZip(id: String, path: String, name: String, tracker: Tracker): java.io.File = withContext(Dispatchers.IO) {
        val file = java.io.File(getApplication<Application>().cacheDir, "opened/$id/zips/$name.zip")
        file.parentFile?.mkdirs()
        call(id) { it.downloadZip(path, file.absolutePath, tracker) }
        file
    }

    suspend fun convertVideo(id: String, entry: Entry, choice: VideoChoice): app.ownnas.core.VideoJob =
        call(id) { it.convertVideo(entry.path, choice.format, choice.height?.toUInt(), choice.speed, choice.audio) }

    suspend fun videoJob(id: String, job: String): app.ownnas.core.VideoJob = call(id) { it.videoJob(job) }

    suspend fun cancelVideoJob(id: String, job: String) {
        runCatching { call(id) { it.cancelVideoJob(job) } }
    }

    fun cachedThumbnail(key: String): ImageBitmap? = thumbnails.get(key)

    /** `null` when the server has no thumbnail for it. */
    suspend fun thumbnail(id: String, entry: Entry, key: String): ImageBitmap? = thumbnailSlots.withPermit {
        thumbnails.get(key) ?: runCatching {
            val bytes = call(id) { it.thumbnail(entry.path) }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
        }.getOrNull()?.also { thumbnails.put(key, it) }
    }

    /** Downloads a file into the cache for other apps to open, reusing an unchanged copy. */
    suspend fun download(id: String, entry: Entry, tracker: Tracker): File =
        withContext(Dispatchers.IO) { hub.cachedCopy(id, entry.path, entry.size.toLong(), entry.modified, tracker) }

    override fun onCleared() {
        cancelUploads()
    }

    private companion object {
        const val THUMBNAIL_CACHE_BYTES = 32 * 1024 * 1024
    }
}

/** A picture file decoded no larger than `maxPx` on its long side (HEIC too, on Android 9+). */
fun decodeScaled(file: File, maxPx: Int): ImageBitmap? {
    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.P) {
        val source = android.graphics.ImageDecoder.createSource(file)
        return runCatching {
            android.graphics.ImageDecoder.decodeBitmap(source) { decoder, info, _ ->
                val long = maxOf(info.size.width, info.size.height)
                if (long > maxPx) {
                    val scale = maxPx.toFloat() / long
                    decoder.setTargetSize((info.size.width * scale).toInt().coerceAtLeast(1), (info.size.height * scale).toInt().coerceAtLeast(1))
                }
                decoder.allocator = android.graphics.ImageDecoder.ALLOCATOR_SOFTWARE
            }.asImageBitmap()
        }.getOrNull()
    }
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeFile(file.path, bounds)
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= maxPx) sample *= 2
    return BitmapFactory.decodeFile(file.path, BitmapFactory.Options().apply { inSampleSize = sample })?.asImageBitmap()
}

/** The folder that holds `path` ("" at the top). */
fun String.parentPath() = substringBeforeLast('/', "")

/** Text to show for a failed server call. */
fun Throwable.userMessage(): String = when (this) {
    is ClientException.Unauthorized -> "Your session ended. Sign in again."
    is ClientException.NotFound -> "This is no longer on the server."
    is ClientException.Conflict -> "This changed on the server. Try again."
    is ClientException.Cancelled -> "Cancelled"
    is ClientException.Locked -> "Enter the terminal password"
    is ClientException.Untrusted -> "The server's certificate changed or is not trusted. Sign in again to check it."
    is ClientException.Failed -> reason
    else -> message ?: "Something went wrong"
}

/** Items cut or copied, waiting to be pasted into another folder of the same server. */
data class Clip(val serverId: String, val entries: List<Entry>, val cut: Boolean)
