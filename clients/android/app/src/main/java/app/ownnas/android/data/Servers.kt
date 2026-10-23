package app.ownnas.android.data

import android.content.Context
import android.provider.DocumentsContract
import app.ownnas.core.Account
import app.ownnas.core.ClientException
import app.ownnas.core.Server
import app.ownnas.core.Transfer
import app.ownnas.core.normalizeAddress
import app.ownnas.core.signIn
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.withContext
import java.io.File
import java.util.concurrent.ConcurrentHashMap

/**
 * The signed-in servers, shared by the app's screens and the documents provider that shows
 * them in the system file picker. Every call blocks; screens use [call] to run off the main thread.
 */
class Servers(private val context: Context, val store: ServerStore) {
    private val connections = ConcurrentHashMap<String, Server>()
    private val accounts = ConcurrentHashMap<String, Account>()
    private val changes = MutableStateFlow(0)
    private val gallery = context.getSharedPreferences("gallery", Context.MODE_PRIVATE)

    /** Bumped after every change made on a server from this phone, so open folders reload. */
    val revision: StateFlow<Int> = changes.asStateFlow()

    private fun connection(id: String): Server = connections.getOrPut(id) {
        val saved = store.find(id) ?: throw ClientException.NotFound()
        val token = store.token(id) ?: throw ClientException.Unauthorized()
        Server(saved.address, token, saved.trust.caPem?.toByteArray(), saved.trust.pinned).also {
            it.setGalleryWorkspace(galleryWorkspace(id))
        }
    }

    /** The Gallery workspace last shown for a server (`null`: every photo). */
    fun galleryWorkspace(id: String): Long? = gallery.getLong("workspace.$id", -1).takeIf { it >= 0 }

    /** Shows one workspace in that server's Gallery from now on, and remembers it. */
    fun setGalleryWorkspace(id: String, workspace: Long?) {
        gallery.edit().apply { if (workspace == null) remove("workspace.$id") else putLong("workspace.$id", workspace) }.apply()
        connections[id]?.setGalleryWorkspace(workspace)
    }

    /** Runs a server call. A rejected token is forgotten, so the app asks to sign in again. */
    fun <T> blocking(id: String, block: (Server) -> T): T = try {
        block(connection(id))
    } catch (err: ClientException.Unauthorized) {
        connections.remove(id)?.close()
        accounts.remove(id)
        store.forgetToken(id)
        notifyRoots()
        throw err
    }

    suspend fun <T> call(id: String, block: (Server) -> T): T = withContext(Dispatchers.IO) { blocking(id, block) }

    /** Who is signed in, and whether the server takes changes. Fetched once per app run. */
    fun account(id: String): Account = accounts[id] ?: blocking(id) { it.account() }.also { accounts[id] = it }

    fun signIn(address: String, username: String, password: String, trust: Trust): SavedServer {
        val normalized = normalizeAddress(address)
        val ca = trust.caPem?.toByteArray()
        val token = signIn(normalized, username.trim(), password, ca, trust.pinned)
        val server = Server(normalized, token, ca, trust.pinned)
        val account = server.account()
        val saved = store.save(normalized, account.username, account.rootName, token, trust)
        connections.put(saved.id, server)?.close()
        accounts[saved.id] = account
        notifyRoots()
        return saved
    }

    /** Forgets a server here and ends its session there, best effort. */
    fun remove(id: String) {
        gallery.edit().remove("workspace.$id").apply()
        accounts.remove(id)
        val server = connections.remove(id)
        store.remove(id)
        notifyRoots()
        server?.let {
            runCatching { it.signOut() }
            it.close()
        }
    }

    /** Call after changing `dir` on a server, from the app or another app through the picker. */
    fun changed(id: String, dir: String) {
        changes.update { it + 1 }
        context.contentResolver.notifyChange(
            DocumentsContract.buildChildDocumentsUri(DOCUMENTS_AUTHORITY(context), documentId(id, dir)),
            null,
        )
    }

    private fun notifyRoots() {
        context.contentResolver.notifyChange(DocumentsContract.buildRootsUri(DOCUMENTS_AUTHORITY(context)), null)
    }

    /**
     * A local copy of a server file for other apps to read, downloaded unless an unchanged copy
     * is already cached. `modified` is in milliseconds.
     */
    fun cachedCopy(id: String, path: String, size: Long, modified: Long, tracker: Transfer): File {
        val file = File(context.cacheDir, "opened/$id/$path")
        if (file.isFile && file.length() == size && file.lastModified() == modified) return file
        file.parentFile?.mkdirs()
        blocking(id) { it.download(path, file.absolutePath, tracker) }
        file.setLastModified(modified)
        return file
    }

    companion object {
        @Suppress("FunctionName")
        fun DOCUMENTS_AUTHORITY(context: Context) = "${context.packageName}.documents"

        /** Documents are named `<server id>:<path>`; the server id never contains a colon. */
        fun documentId(id: String, path: String) = "$id:$path"

        fun parseDocumentId(documentId: String): Pair<String, String> {
            val colon = documentId.indexOf(':')
            require(colon > 0) { "Not an OwnNAS document: $documentId" }
            return documentId.substring(0, colon) to documentId.substring(colon + 1)
        }
    }
}
