package app.ownnas.android.data

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.ContentUris
import android.content.Context
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.database.Cursor
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.provider.Settings
import androidx.core.app.NotificationCompat
import androidx.core.content.edit
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import app.ownnas.android.OwnNasApp
import app.ownnas.android.R
import app.ownnas.android.ui.userMessage
import app.ownnas.core.ClientException
import app.ownnas.core.Transfer
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File
import java.util.concurrent.TimeUnit

/** Backing this phone's photos and videos up to a server's media upload folder (one-way). */
data class BackupSettings(
    /** The server to back up to, or null when backups are off. */
    val serverId: String? = null,
    val wifiOnly: Boolean = true,
    val videos: Boolean = true,
)

data class BackupState(
    val running: Boolean = false,
    val done: Int = 0,
    val total: Int = 0,
    /** Photos and videos backed up so far, over all runs. */
    val backedUp: Int = 0,
    /** Milliseconds since the Unix epoch of the last finished run, or 0. */
    val lastRun: Long = 0,
    val lastResult: String? = null,
)

class Backup(private val context: Context) {
    private val prefs = context.getSharedPreferences("backup", Context.MODE_PRIVATE)
    private val doneFile = File(context.filesDir, "backup-done.txt")
    private val settingsFlow = MutableStateFlow(loadSettings())
    val settings: StateFlow<BackupSettings> = settingsFlow.asStateFlow()
    private val stateFlow = MutableStateFlow(BackupState(backedUp = prefs.getInt("count", 0), lastRun = prefs.getLong("lastRun", 0), lastResult = prefs.getString("lastResult", null)))
    val state: StateFlow<BackupState> = stateFlow.asStateFlow()

    private fun loadSettings() = BackupSettings(
        serverId = prefs.getString("server", null),
        wifiOnly = prefs.getBoolean("wifiOnly", true),
        videos = prefs.getBoolean("videos", true),
    )

    fun update(change: (BackupSettings) -> BackupSettings) {
        val next = change(settingsFlow.value)
        prefs.edit {
            putString("server", next.serverId)
            putBoolean("wifiOnly", next.wifiOnly)
            putBoolean("videos", next.videos)
        }
        // Another server keeps its own copies: start counting again.
        if (next.serverId != settingsFlow.value.serverId) {
            doneFile.delete()
            prefs.edit { putInt("count", 0) }
            stateFlow.value = BackupState()
        }
        settingsFlow.value = next
        schedule()
    }

    fun forgetServer(serverId: String) {
        if (settingsFlow.value.serverId == serverId) update { it.copy(serverId = null) }
    }

    /** Hourly, and soon after new photos appear; nothing when backups are off. */
    fun schedule() {
        val work = WorkManager.getInstance(context)
        val settings = settingsFlow.value
        if (settings.serverId == null) {
            work.cancelUniqueWork(PERIODIC)
            work.cancelUniqueWork(TRIGGERED)
            return
        }
        val periodic = PeriodicWorkRequestBuilder<BackupWorker>(1, TimeUnit.HOURS).setConstraints(constraints(settings)).build()
        work.enqueueUniquePeriodicWork(PERIODIC, ExistingPeriodicWorkPolicy.UPDATE, periodic)
        watchForNewMedia()
    }

    /** A run that starts when the phone's photo library changes. */
    fun watchForNewMedia() {
        val settings = settingsFlow.value
        if (settings.serverId == null) return
        val trigger = Constraints.Builder()
            .setRequiredNetworkType(if (settings.wifiOnly) NetworkType.UNMETERED else NetworkType.CONNECTED)
            .addContentUriTrigger(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, true)
            .addContentUriTrigger(MediaStore.Video.Media.EXTERNAL_CONTENT_URI, true)
            .setTriggerContentUpdateDelay(30, TimeUnit.SECONDS)
            .build()
        val request = OneTimeWorkRequestBuilder<BackupWorker>().setConstraints(trigger).build()
        WorkManager.getInstance(context).enqueueUniqueWork(TRIGGERED, ExistingWorkPolicy.KEEP, request)
    }

    fun backUpNow() {
        val settings = settingsFlow.value
        if (settings.serverId == null) return
        val request = OneTimeWorkRequestBuilder<BackupWorker>()
            .setConstraints(constraints(settings))
            .setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
            .build()
        WorkManager.getInstance(context).enqueueUniqueWork(NOW, ExistingWorkPolicy.KEEP, request)
    }

    private fun constraints(settings: BackupSettings) =
        Constraints.Builder().setRequiredNetworkType(if (settings.wifiOnly) NetworkType.UNMETERED else NetworkType.CONNECTED).build()

    fun hasPermission(): Boolean = permissions().all { context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }

    fun permissions(): List<String> = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        listOfNotNull(Manifest.permission.READ_MEDIA_IMAGES, if (settingsFlow.value.videos) Manifest.permission.READ_MEDIA_VIDEO else null)
    } else {
        listOf(Manifest.permission.READ_EXTERNAL_STORAGE)
    }

    fun deviceName(): String =
        Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)?.takeIf { it.isNotBlank() } ?: Build.MODEL

    internal fun setState(change: (BackupState) -> BackupState) {
        stateFlow.value = change(stateFlow.value)
    }

    internal fun finish(result: String, added: Int) {
        val count = prefs.getInt("count", 0) + added
        val now = System.currentTimeMillis()
        prefs.edit {
            putInt("count", count)
            putLong("lastRun", now)
            putString("lastResult", result)
        }
        setState { it.copy(running = false, backedUp = count, lastRun = now, lastResult = result) }
    }

    /** Keys of the photos already backed up ("<store id>:<size>:<modified>"). */
    internal fun doneKeys(): MutableSet<String> = if (doneFile.isFile) doneFile.readLines().toMutableSet() else mutableSetOf()

    internal fun markDone(key: String) = doneFile.appendText("$key\n")

    companion object {
        private const val PERIODIC = "ownnas-backup"
        private const val TRIGGERED = "ownnas-backup-new-media"
        private const val NOW = "ownnas-backup-now"
        const val CHANNEL = "backup"
        const val NOTIFICATION = 7101
    }
}

/** One photo or video in the phone's library. */
private data class Media(val uri: Uri, val key: String, val name: String, val size: Long, val takenMs: Long)

/** Sends the phone's new photos and videos to the server, newest first. Never deletes anything. */
class BackupWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    private companion object {
        val running = kotlinx.coroutines.sync.Mutex()
    }

    private val app = context.applicationContext as OwnNasApp

    override suspend fun doWork(): Result {
        // Hourly, "now", and new-photo runs can start together: one at a time.
        if (!running.tryLock()) return Result.success()
        try {
            return backUp()
        } finally {
            running.unlock()
        }
    }

    private suspend fun backUp(): Result {
        val backup = app.backup
        // The new-photos trigger fires once; watch for the next ones.
        backup.watchForNewMedia()
        val settings = backup.settings.value
        val serverId = settings.serverId ?: return Result.success()
        if (!backup.hasPermission()) {
            backup.finish("Allow OwnNAS to see your photos to back them up", 0)
            return Result.success()
        }
        val folder = try {
            app.servers.call(serverId) { it.backupFolder() }
        } catch (err: ClientException.Unauthorized) {
            backup.finish("Sign in to the server again to keep backing up", 0)
            return Result.success()
        } catch (err: Exception) {
            backup.finish(err.userMessage(), 0)
            return Result.retry()
        }
        if (folder == null) {
            backup.finish("The server has no media upload folder yet. An administrator can set one in the web app: Settings → Gallery → Phone backups.", 0)
            return Result.success()
        }
        val done = backup.doneKeys()
        val pending = media(settings.videos).filterNot { it.key in done }
        if (pending.isEmpty()) {
            backup.finish("Up to date", 0)
            return Result.success()
        }
        backup.setState { it.copy(running = true, done = 0, total = pending.size) }
        runCatching { setForeground(foreground(0, pending.size)) }
        val device = backup.deviceName()
        var added = 0
        var failed = 0
        var firstProblem: String? = null
        for ((index, item) in pending.withIndex()) {
            if (isStopped) break
            val outcome = runCatching {
                val fd = applicationContext.contentResolver.openFileDescriptor(item.uri, "r")?.detachFd() ?: error("Could not open ${item.name}")
                app.servers.call(serverId) {
                    it.backupFd(device, item.name, fd, item.size.toULong(), item.takenMs, object : Transfer {
                        override fun keepGoing(done: ULong) = !isStopped
                    })
                }
            }
            outcome.onSuccess {
                backup.markDone(item.key)
                if (!it.skipped) added++
            }.onFailure { err ->
                failed++
                android.util.Log.w("OwnNAS", "Backing up ${item.name} failed", err)
                if (firstProblem == null) firstProblem = "${item.name}: ${err.userMessage()}"
                // Signed out, or the server or network keeps failing: try again later instead of
                // failing every photo. A single unreadable file is just skipped.
                if (err is ClientException.Unauthorized || failed >= 5) {
                    backup.finish("Stopped after ${index} of ${pending.size}: ${err.userMessage()}", added)
                    return Result.retry()
                }
            }
            backup.setState { it.copy(done = index + 1) }
            if (index % 5 == 0) runCatching { setForeground(foreground(index + 1, pending.size)) }
        }
        val result = when {
            isStopped -> "Paused; it carries on later"
            failed > 0 -> "Backed up ${added} new; ${failed} could not be sent (${firstProblem})"
            else -> "Backed up ${added} new photo${if (added == 1) "" else "s"}"
        }
        backup.finish(result, added)
        return Result.success()
    }

    private fun media(videos: Boolean): List<Media> {
        val out = mutableListOf<Media>()
        val sources = listOfNotNull(
            MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
            if (videos) MediaStore.Video.Media.EXTERNAL_CONTENT_URI else null,
        )
        for (source in sources) {
            val columns = arrayOf(
                MediaStore.MediaColumns._ID,
                MediaStore.MediaColumns.DISPLAY_NAME,
                MediaStore.MediaColumns.SIZE,
                MediaStore.MediaColumns.DATE_TAKEN,
                MediaStore.MediaColumns.DATE_MODIFIED,
            )
            val selection = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) "${MediaStore.MediaColumns.IS_PENDING} = 0" else null
            applicationContext.contentResolver.query(source, columns, selection, null, null)?.use { cursor: Cursor ->
                while (cursor.moveToNext()) {
                    val id = cursor.getLong(0)
                    val name = cursor.getString(1) ?: continue
                    val size = cursor.getLong(2)
                    if (size <= 0) continue
                    val modifiedMs = cursor.getLong(4) * 1000
                    val taken = cursor.getLong(3).takeIf { it > 0 } ?: modifiedMs
                    val kind = if (source == MediaStore.Images.Media.EXTERNAL_CONTENT_URI) "i" else "v"
                    out += Media(ContentUris.withAppendedId(source, id), "$kind$id:$size:$modifiedMs", name, size, taken)
                }
            }
        }
        return out.sortedByDescending { it.takenMs }
    }

    private fun foreground(done: Int, total: Int): ForegroundInfo {
        val manager = applicationContext.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && manager.getNotificationChannel(Backup.CHANNEL) == null) {
            manager.createNotificationChannel(NotificationChannel(Backup.CHANNEL, "Photo backup", NotificationManager.IMPORTANCE_LOW))
        }
        val notification = NotificationCompat.Builder(applicationContext, Backup.CHANNEL)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle("Backing up photos")
            .setContentText("$done of $total")
            .setProgress(total, done, false)
            .setOngoing(true)
            .setSilent(true)
            .build()
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ForegroundInfo(Backup.NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(Backup.NOTIFICATION, notification)
        }
    }
}
