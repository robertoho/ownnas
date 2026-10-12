package app.ownnas.android.data

import android.content.res.AssetFileDescriptor
import android.database.Cursor
import android.database.MatrixCursor
import android.graphics.Point
import android.os.CancellationSignal
import android.os.Handler
import android.os.HandlerThread
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract.Document
import android.provider.DocumentsContract.Root
import android.provider.DocumentsProvider
import android.webkit.MimeTypeMap
import app.ownnas.android.OwnNasApp
import app.ownnas.android.R
import app.ownnas.core.ClientException
import app.ownnas.core.Conflict
import app.ownnas.core.Entry
import app.ownnas.core.Transfer
import app.ownnas.core.UploadResult
import java.io.File
import java.io.FileNotFoundException
import java.util.concurrent.ConcurrentHashMap

/**
 * Shows each signed-in server in the system file picker and Files app, so other apps can open,
 * save, rename, and delete files on it. Documents are `<server id>:<path>`.
 */
class OwnNasDocuments : DocumentsProvider() {
    private val servers by lazy { (context!!.applicationContext as OwnNasApp).servers }

    /** What listings said about each path, so single documents need no extra request. */
    private val known = ConcurrentHashMap<String, Entry>()

    /** Uploads files that other apps finished writing, off the binder threads. */
    private val writer by lazy { Handler(HandlerThread("ownnas-documents").apply { start() }.looper) }

    override fun onCreate() = true

    override fun queryRoots(projection: Array<out String>?): Cursor {
        val cursor = MatrixCursor(projection ?: ROOT_COLUMNS)
        for (server in servers.store.servers.value) {
            if (servers.store.token(server.id) == null) continue
            cursor.newRow().apply {
                add(Root.COLUMN_ROOT_ID, server.id)
                add(Root.COLUMN_DOCUMENT_ID, Servers.documentId(server.id, ""))
                add(Root.COLUMN_TITLE, server.name)
                add(Root.COLUMN_SUMMARY, "${server.username} · ${server.address.substringAfter("://")}")
                add(Root.COLUMN_ICON, R.mipmap.ic_launcher)
                add(Root.COLUMN_FLAGS, Root.FLAG_SUPPORTS_CREATE or Root.FLAG_SUPPORTS_IS_CHILD)
            }
        }
        return cursor
    }

    override fun queryDocument(documentId: String, projection: Array<out String>?): Cursor {
        val (id, path) = Servers.parseDocumentId(documentId)
        val cursor = MatrixCursor(projection ?: DOCUMENT_COLUMNS)
        if (path.isEmpty()) {
            val server = servers.store.find(id) ?: throw FileNotFoundException(documentId)
            cursor.newRow().apply {
                add(Document.COLUMN_DOCUMENT_ID, documentId)
                add(Document.COLUMN_DISPLAY_NAME, server.name)
                add(Document.COLUMN_MIME_TYPE, Document.MIME_TYPE_DIR)
                add(Document.COLUMN_FLAGS, if (writable(id)) Document.FLAG_DIR_SUPPORTS_CREATE else 0)
            }
        } else {
            addRow(cursor, id, entry(id, path))
        }
        return cursor
    }

    override fun queryChildDocuments(parentDocumentId: String, projection: Array<out String>?, sortOrder: String?): Cursor {
        val (id, path) = Servers.parseDocumentId(parentDocumentId)
        val folder = network { servers.blocking(id) { it.list(path) } }
        val cursor = MatrixCursor(projection ?: DOCUMENT_COLUMNS)
        val writable = writable(id)
        for (entry in folder.entries) {
            known["$id:${entry.path}"] = entry
            addRow(cursor, id, entry, writable)
        }
        // The picker reloads when the app or another picker changes this folder.
        cursor.setNotificationUri(context!!.contentResolver, android.provider.DocumentsContract.buildChildDocumentsUri(Servers.DOCUMENTS_AUTHORITY(context!!), parentDocumentId))
        return cursor
    }

    override fun isChildDocument(parentDocumentId: String, documentId: String): Boolean {
        val (parentServer, parentPath) = Servers.parseDocumentId(parentDocumentId)
        val (server, path) = Servers.parseDocumentId(documentId)
        return parentServer == server && (parentPath.isEmpty() || path.startsWith("$parentPath/"))
    }

    override fun openDocument(documentId: String, mode: String, signal: CancellationSignal?): ParcelFileDescriptor {
        val (id, path) = Servers.parseDocumentId(documentId)
        val access = ParcelFileDescriptor.parseMode(mode)
        val truncate = (access and ParcelFileDescriptor.MODE_TRUNCATE) != 0
        val writeOnly = (access and ParcelFileDescriptor.MODE_READ_WRITE) == 0 && (access and ParcelFileDescriptor.MODE_WRITE_ONLY) != 0
        val replacing = truncate || writeOnly
        val file = if (replacing) {
            // The other app writes the whole file, so the old bytes are not needed.
            File(context!!.cacheDir, "opened/$id/$path").apply { parentFile?.mkdirs() }
        } else {
            val entry = entry(id, path)
            network { servers.cachedCopy(id, path, entry.size.toLong(), entry.modified, Cancellable(signal)) }
        }
        if (access == ParcelFileDescriptor.MODE_READ_ONLY) return ParcelFileDescriptor.open(file, access)
        // Another app is saving into this file: send it back to the server once it is closed.
        return ParcelFileDescriptor.open(file, access, writer) { error ->
            if (error == null) save(id, path, file)
        }
    }

    override fun openDocumentThumbnail(documentId: String, sizeHint: Point?, signal: CancellationSignal?): AssetFileDescriptor {
        val (id, path) = Servers.parseDocumentId(documentId)
        val bytes = network { servers.blocking(id) { it.thumbnail(path) } }
        val file = File(context!!.cacheDir, "thumbs/${documentId.hashCode()}.png")
        file.parentFile?.mkdirs()
        file.writeBytes(bytes)
        return AssetFileDescriptor(ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY), 0, file.length())
    }

    override fun createDocument(parentDocumentId: String, mimeType: String, displayName: String): String {
        val (id, dir) = Servers.parseDocumentId(parentDocumentId)
        val name = freeName(id, dir, displayName.trim().ifEmpty { "Untitled" }, mimeType)
        val path = if (dir.isEmpty()) name else "$dir/$name"
        network {
            if (mimeType == Document.MIME_TYPE_DIR) {
                servers.blocking(id) { it.createFolder(dir, name) }
            } else {
                // An empty file now; the bytes arrive when the other app writes and closes it.
                val empty = File(context!!.cacheDir, "opened/$id/$path")
                empty.parentFile?.mkdirs()
                empty.writeBytes(ByteArray(0))
                upload(id, dir, name, empty, null)
            }
        }
        servers.changed(id, dir)
        return Servers.documentId(id, path)
    }

    override fun renameDocument(documentId: String, displayName: String): String {
        val (id, path) = Servers.parseDocumentId(documentId)
        val renamed = network { servers.blocking(id) { it.rename(path, displayName.trim()) } }
        known.remove(documentId)
        servers.changed(id, path.substringBeforeLast('/', ""))
        return Servers.documentId(id, renamed)
    }

    override fun deleteDocument(documentId: String) {
        val (id, path) = Servers.parseDocumentId(documentId)
        network { servers.blocking(id) { it.trash(path) } }
        known.remove(documentId)
        servers.changed(id, path.substringBeforeLast('/', ""))
    }

    override fun getDocumentType(documentId: String): String {
        val (id, path) = Servers.parseDocumentId(documentId)
        return if (path.isEmpty()) Document.MIME_TYPE_DIR else mimeType(entry(id, path))
    }

    private fun writable(id: String) = runCatching { !servers.account(id).readonly }.getOrDefault(false)

    /** A path's details, from the last listing of its folder or by listing that folder now. */
    private fun entry(id: String, path: String): Entry {
        known["$id:$path"]?.let { return it }
        val parent = path.substringBeforeLast('/', "")
        val folder = network { servers.blocking(id) { it.list(parent) } }
        folder.entries.forEach { known["$id:${it.path}"] = it }
        return known["$id:$path"] ?: throw FileNotFoundException(path)
    }

    private fun addRow(cursor: MatrixCursor, id: String, entry: Entry, writable: Boolean = writable(id)) {
        var flags = 0
        if (writable) flags = flags or Document.FLAG_SUPPORTS_RENAME or Document.FLAG_SUPPORTS_DELETE
        flags = flags or if (entry.dir) {
            if (writable) Document.FLAG_DIR_SUPPORTS_CREATE else 0
        } else {
            (if (writable) Document.FLAG_SUPPORTS_WRITE else 0) or (if (entry.thumb) Document.FLAG_SUPPORTS_THUMBNAIL else 0)
        }
        cursor.newRow().apply {
            add(Document.COLUMN_DOCUMENT_ID, Servers.documentId(id, entry.path))
            add(Document.COLUMN_DISPLAY_NAME, entry.name)
            add(Document.COLUMN_MIME_TYPE, mimeType(entry))
            add(Document.COLUMN_SIZE, if (entry.dir) null else entry.size.toLong())
            add(Document.COLUMN_LAST_MODIFIED, entry.modified)
            add(Document.COLUMN_FLAGS, flags)
        }
    }

    /** Sends a file another app wrote back to the server, replacing what is there. */
    private fun save(id: String, path: String, file: File) {
        val dir = path.substringBeforeLast('/', "")
        runCatching { upload(id, dir, path.substringAfterLast('/'), file, Conflict.OVERWRITE) }
        file.setLastModified(0) // the server's copy is newer now; read it fresh next time
        known.remove("$id:$path")
        servers.changed(id, dir)
    }

    private fun upload(id: String, dir: String, name: String, file: File, conflict: Conflict?) {
        val fd = ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY).detachFd()
        val result = servers.blocking(id) {
            it.uploadFd(dir, name, fd, file.length().toULong(), file.lastModified() / 1000, conflict, Cancellable(null))
        }
        if (result is UploadResult.Exists) throw FileNotFoundException("$name already exists")
    }

    /**
     * `name`, or `name (2).ext`, … whichever the server does not have yet. Pickers expect the
     * new document's id back, so the name is settled here rather than by the server.
     */
    private fun freeName(id: String, dir: String, name: String, mimeType: String): String {
        val withExtension = if (mimeType == Document.MIME_TYPE_DIR || name.contains('.')) {
            name
        } else {
            MimeTypeMap.getSingleton().getExtensionFromMimeType(mimeType)?.let { "$name.$it" } ?: name
        }
        val dot = withExtension.lastIndexOf('.').takeIf { it > 0 && mimeType != Document.MIME_TYPE_DIR }
        val stem = dot?.let { withExtension.substring(0, it) } ?: withExtension
        val ext = dot?.let { withExtension.substring(it) } ?: ""
        return network {
            generateSequence(1) { it + 1 }
                .map { if (it == 1) withExtension else "$stem ($it)$ext" }
                .take(1000)
                .first { candidate -> servers.blocking(id) { it.checkUpload(dir, candidate) } == null }
        }
    }

    /** Turns server failures into what document clients expect: a missing file. */
    private fun <T> network(block: () -> T): T = try {
        block()
    } catch (err: ClientException) {
        throw FileNotFoundException(err.message ?: err.toString())
    }

    private class Cancellable(private val signal: CancellationSignal?) : Transfer {
        override fun keepGoing(done: ULong) = signal?.isCanceled != true
    }

    private companion object {
        val ROOT_COLUMNS = arrayOf(
            Root.COLUMN_ROOT_ID, Root.COLUMN_DOCUMENT_ID, Root.COLUMN_TITLE, Root.COLUMN_SUMMARY,
            Root.COLUMN_ICON, Root.COLUMN_FLAGS,
        )
        val DOCUMENT_COLUMNS = arrayOf(
            Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE,
            Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED, Document.COLUMN_FLAGS,
        )

        fun mimeType(entry: Entry): String = if (entry.dir) {
            Document.MIME_TYPE_DIR
        } else {
            MimeTypeMap.getSingleton().getMimeTypeFromExtension(entry.name.substringAfterLast('.', "").lowercase())
                ?: "application/octet-stream"
        }
    }
}
