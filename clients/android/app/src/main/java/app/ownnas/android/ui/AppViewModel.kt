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
import app.ownnas.core.ClientException
import app.ownnas.core.Entry
import app.ownnas.core.Folder
import app.ownnas.core.Server
import app.ownnas.core.normalizeAddress
import app.ownnas.core.signIn
import kotlinx.coroutines.Dispatchers
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
    private val store = (application as OwnNasApp).store
    val servers = store.servers

    /** The back stack; the last screen is showing. */
    val stack = mutableStateListOf<Screen>(Screen.Servers)

    private val connections = ConcurrentHashMap<String, Server>()
    private val folders = ConcurrentHashMap<Pair<String, String>, Folder>()
    private val thumbnails = object : LruCache<String, ImageBitmap>(THUMBNAIL_CACHE_BYTES) {
        override fun sizeOf(key: String, value: ImageBitmap) = value.width * value.height * 4
    }
    private val thumbnailSlots = Semaphore(4)

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

    private fun connection(id: String): Server = connections.getOrPut(id) {
        val saved = store.find(id) ?: throw ClientException.NotFound()
        val token = store.token(id) ?: throw ClientException.Unauthorized()
        Server(saved.address, token, null)
    }

    /** Runs a server call off the main thread; a rejected token is forgotten so the app asks to sign in. */
    private suspend fun <T> call(id: String, block: (Server) -> T): T = withContext(Dispatchers.IO) {
        try {
            block(connection(id))
        } catch (err: ClientException.Unauthorized) {
            connections.remove(id)?.close()
            store.forgetToken(id)
            throw err
        }
    }

    suspend fun signIn(address: String, username: String, password: String): SavedServer = withContext(Dispatchers.IO) {
        val normalized = normalizeAddress(address)
        val token = signIn(normalized, username.trim(), password, null)
        val server = Server(normalized, token, null)
        val account = server.account()
        val saved = store.save(normalized, account.username, account.rootName, token)
        connections.put(saved.id, server)?.close()
        saved
    }

    fun remove(id: String) {
        val server = connections.remove(id)
        store.remove(id)
        folders.keys.removeAll { it.first == id }
        // Ending the session on the server is best effort: the token is already gone here.
        viewModelScope.launch(Dispatchers.IO) {
            server?.let { runCatching { it.signOut() }; it.close() }
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
    suspend fun download(id: String, entry: Entry): File {
        val file = File(getApplication<Application>().cacheDir, "opened/$id/${entry.path}")
        if (file.isFile && file.length() == entry.size.toLong() && file.lastModified() == entry.modified) return file
        file.parentFile?.mkdirs()
        call(id) { it.download(entry.path, file.absolutePath) }
        file.setLastModified(entry.modified)
        return file
    }

    override fun onCleared() {
        connections.values.forEach(Server::close)
        connections.clear()
    }

    private companion object {
        const val THUMBNAIL_CACHE_BYTES = 32 * 1024 * 1024
    }
}

/** Text to show for a failed server call. */
fun Throwable.userMessage(): String = when (this) {
    is ClientException.Unauthorized -> "Your session ended. Sign in again."
    is ClientException.NotFound -> "This is no longer on the server."
    is ClientException.Conflict -> "This changed on the server. Try again."
    is ClientException.Failed -> reason
    else -> message ?: "Something went wrong"
}
