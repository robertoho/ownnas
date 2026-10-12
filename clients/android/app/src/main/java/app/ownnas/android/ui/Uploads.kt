package app.ownnas.android.ui

import android.content.ContentResolver
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.os.SystemClock
import app.ownnas.core.ClientException
import app.ownnas.core.Transfer

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
    val serverId: String,
    val dir: String,
    val name: String,
    val size: Long?,
    val modified: Long?,
    val file: ParcelFileDescriptor,
)

/** What the upload bar shows. */
data class UploadStatus(
    val total: Int,
    val finished: Int,
    val failed: Int,
    val current: String?,
    val currentSize: Long?,
    val currentDone: Long,
) {
    val active get() = finished + failed < total
}

/** Opens each picked document and reads the name, size, and modification time it offers. */
fun ContentResolver.pendingUploads(serverId: String, dir: String, uris: List<Uri>): Pair<List<PendingUpload>, Int> {
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
                }
            }
        }
        val fallbackName = uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() } ?: "upload"
        PendingUpload(serverId, dir, name?.takeIf { it.isNotBlank() } ?: fallbackName, size?.takeIf { it >= 0 }, modified?.takeIf { it > 0 }, file)
    }
    return uploads to unreadable
}

fun Throwable.isCancel() = this is ClientException.Cancelled
