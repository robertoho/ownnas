package app.ownnas.android.ui

import android.app.Application
import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import app.ownnas.android.OwnNasApp
import app.ownnas.android.data.SavedServer
import app.ownnas.core.Account
import app.ownnas.core.ClientException
import app.ownnas.core.Conflict
import app.ownnas.core.Entry
import app.ownnas.core.UploadResult
import app.ownnas.core.Folder
import app.ownnas.core.Server
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
}

class AppViewModel(application: Application) : AndroidViewModel(application) {
    private val hub = (application as OwnNasApp).servers
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

    private suspend fun <T> call(id: String, block: (Server) -> T): T = hub.call(id, block)

    suspend fun signIn(address: String, username: String, password: String): SavedServer =
        withContext(Dispatchers.IO) { hub.signIn(address, username, password) }

    fun remove(id: String) {
        folders.keys.removeAll { it.first == id }
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

    /** The last listing of a folder, to show while it reloads. */
    fun cachedFolder(id: String, path: String): Folder? = folders[id to path]

    suspend fun list(id: String, path: String): Folder =
        call(id) { it.list(path) }.also { folders[id to path] = it }

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

/** The folder that holds `path` ("" at the top). */
fun String.parentPath() = substringBeforeLast('/', "")

/** Text to show for a failed server call. */
fun Throwable.userMessage(): String = when (this) {
    is ClientException.Unauthorized -> "Your session ended. Sign in again."
    is ClientException.NotFound -> "This is no longer on the server."
    is ClientException.Conflict -> "This changed on the server. Try again."
    is ClientException.Cancelled -> "Cancelled"
    is ClientException.Failed -> reason
    else -> message ?: "Something went wrong"
}
