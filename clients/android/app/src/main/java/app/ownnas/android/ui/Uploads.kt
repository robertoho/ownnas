package app.ownnas.android.ui

import android.content.ContentResolver
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.os.SystemClock
import app.ownnas.core.ClientException
import app.ownnas.core.Conflict
import app.ownnas.core.Existing
import app.ownnas.core.Transfer
import kotlinx.coroutines.CompletableDeferred

/** Reports transfer progress at most every 100 ms and cancels when asked. */
class Tracker(private val onProgress: (Long) -> Unit) : Transfer {
    @Volatile var cancelled = false
    private var last = 0L

    override fun keepGoing(done: ULong): Boolean {
        val now = SystemClock.uptimeMillis()
        if (now - last >= 100) {
            last = now
            onProgress(done.toLong())
        }
        return !cancelled
    }
}

/** A picked file waiting to go to the server. Its descriptor is opened at pick time, while
 *  the app still holds the picker's read permission. */
class PendingUpload(
    /** Files picked together; "use this choice for the remaining files" applies within one. */
    val batch: Long,
    val serverId: String,
    val dir: String,
    val name: String,
    val size: Long?,
    val modified: Long?,
    val file: ParcelFileDescriptor,
    /** The provider's MIME type, for naming things like pasted screenshots that have no name. */
    val type: String? = null,
)

/** What the upload bar shows. */
data class UploadStatus(
    val total: Int,
    val finished: Int,
    val skipped: Int,
    val failed: Int,
    val current: String?,
    val currentSize: Long?,
    val currentDone: Long,
) {
    val active get() = finished + skipped + failed < total
}

/** Opens each picked document and reads the name, size, and modification time it offers. */
fun ContentResolver.pendingUploads(serverId: String, dir: String, uris: List<Uri>): Pair<List<PendingUpload>, Int> {
    val batch = System.nanoTime()
    var unreadable = 0
    val uploads = uris.mapNotNull { uri ->
        val file = runCatching { openFileDescriptor(uri, "r") }.getOrNull()
        if (file == null) {
            unreadable++
            return@mapNotNull null
        }
        var name: String? = null
        var size: Long? = null
        var modified: Long? = null
        runCatching {
            query(uri, null, null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) {
                    fun column(name: String) = cursor.getColumnIndex(name).takeIf { it >= 0 && !cursor.isNull(it) }
                    column(OpenableColumns.DISPLAY_NAME)?.let { name = cursor.getString(it) }
                    column(OpenableColumns.SIZE)?.let { size = cursor.getLong(it) }
                    column(DocumentsContract.Document.COLUMN_LAST_MODIFIED)?.let { modified = cursor.getLong(it) }
                    // The photo picker offers when a photo or video was taken instead.
                    if (modified == null) column(MediaStore.MediaColumns.DATE_TAKEN)?.let { modified = cursor.getLong(it) }
                }
            }
        }
        val fallbackName = uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() } ?: "upload"
        PendingUpload(
            batch,
            serverId,
            dir,
            name?.takeIf { it.isNotBlank() } ?: fallbackName,
            size?.takeIf { it >= 0 },
            modified?.takeIf { it > 0 },
            file,
            runCatching { getType(uri) }.getOrNull(),
        )
    }
    return uploads to unreadable
}

fun Throwable.isCancel() = this is ClientException.Cancelled

/** The server already has something at an upload's name; the app asks what to do, as the web app does. */
class ConflictPrompt(
    val name: String,
    val existing: Existing,
    val incomingSize: Long?,
    /** Milliseconds; without it, "Archive older" cannot be offered. */
    val incomingModified: Long?,
    /** More files from the same pick are waiting. */
    val more: Boolean,
) {
    internal val reply = CompletableDeferred<ConflictAnswer?>()

    /** `null` cancels the rest of this pick. */
    fun answer(choice: ConflictAnswer?) {
        reply.complete(choice)
    }

    /** A folder can only be kept alongside or skipped. */
    fun allows(choice: Conflict) = when (choice) {
        Conflict.KEEP_BOTH, Conflict.IGNORE -> true
        Conflict.ARCHIVE_OLDER -> !existing.dir && incomingModified != null
        Conflict.OVERWRITE, Conflict.ARCHIVE_EXISTING -> !existing.dir
    }
}

data class ConflictAnswer(val choice: Conflict, val rest: Boolean)
