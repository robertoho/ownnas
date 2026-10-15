package app.ownnas.android.data

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Intent
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
import app.ownnas.android.MainActivity
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
    /** Backups are halted until resumed (the server choice is kept). */
    val paused: Boolean = false,
)

data class BackupState(
    val running: Boolean = false,
    val done: Int = 0,
    val total: Int = 0,
    /** Photos and videos on the server: sent by this phone, or found already there. */
    val backedUp: Int = 0,
    /** Photos and videos still to send, as of the last run. */
    val left: Int = 0,
    /** Milliseconds since the Unix epoch of the last finished run, or 0. */
    val lastRun: Long = 0,
    val lastResult: String? = null,
    /** The file being sent now. */
    val current: String? = null,
    /** When the running (or last) run started, and the bytes it has sent. */
    val runStarted: Long = 0,
    val runBytes: Long = 0,
    /** The last finished run: how long it took, what it sent, and how many files. */
    val lastDurationMs: Long = 0,
    val lastBytes: Long = 0,
    val lastSent: Int = 0,
    /** Everything this phone has sent to the current server. */
    val totalBytes: Long = 0,
    val totalSent: Int = 0,
)

/** A photo or video a run sent, or found on the server already. */
data class BackedUpItem(val uri: Uri, val name: String, val size: Long, val at: Long, val skipped: Boolean, val blocked: Boolean = false)

class Backup(private val context: Context) {
    private val prefs = context.getSharedPreferences("backup", Context.MODE_PRIVATE)
    private val doneFile = File(context.filesDir, "backup-done.txt")
    private val recentFile = File(context.filesDir, "backup-recent.tsv")
    /** Photos the server said were deleted there: not sent, and not counted as backed up. */
    private val blockedFile = File(context.filesDir, "backup-blocked.txt")
    private val settingsFlow = MutableStateFlow(loadSettings())
    val settings: StateFlow<BackupSettings> = settingsFlow.asStateFlow()
    private val stateFlow = MutableStateFlow(
        BackupState(
            backedUp = countDone(),
            left = prefs.getInt("left", 0),
            lastRun = prefs.getLong("lastRun", 0),
            lastResult = prefs.getString("lastResult", null),
            runStarted = prefs.getLong("runStarted", 0),
            lastDurationMs = prefs.getLong("lastDuration", 0),
            lastBytes = prefs.getLong("lastBytes", 0),
            lastSent = prefs.getInt("lastSent", 0),
            totalBytes = prefs.getLong("totalBytes", 0),
            totalSent = prefs.getInt("totalSent", 0),
        ),
    )
    val state: StateFlow<BackupState> = stateFlow.asStateFlow()

    private fun loadSettings() = BackupSettings(
        serverId = prefs.getString("server", null),
        wifiOnly = prefs.getBoolean("wifiOnly", true),
        videos = prefs.getBoolean("videos", true),
        paused = prefs.getBoolean("paused", false),
    )

    fun update(change: (BackupSettings) -> BackupSettings) {
        val next = change(settingsFlow.value)
        prefs.edit {
            putString("server", next.serverId)
            putBoolean("wifiOnly", next.wifiOnly)
            putBoolean("videos", next.videos)
            putBoolean("paused", next.paused)
        }
        // Another server keeps its own copies: start counting again.
        if (next.serverId != settingsFlow.value.serverId) {
            doneFile.delete()
            recentFile.delete()
            blockedFile.delete()
            prefs.edit {
                for (key in listOf("left", "lastSent", "totalSent")) remove(key)
                for (key in listOf("lastRun", "runStarted", "lastDuration", "lastBytes", "totalBytes")) remove(key)
                remove("lastResult")
            }
            stateFlow.value = BackupState()
        }
        settingsFlow.value = next
        schedule()
    }

    fun forgetServer(serverId: String) {
        if (settingsFlow.value.serverId == serverId) update { it.copy(serverId = null) }
    }

    /** Halts backups, stopping one that is running; [resume] carries on where it left off. */
    fun pause() = update { it.copy(paused = true) }

    fun resume() {
        update { it.copy(paused = false) }
        backUpNow()
    }

    /** Hourly, and soon after new photos appear; nothing when backups are off or paused. */
    fun schedule() {
        val work = WorkManager.getInstance(context)
        val settings = settingsFlow.value
        if (settings.serverId == null || settings.paused) {
            work.cancelUniqueWork(PERIODIC)
            work.cancelUniqueWork(TRIGGERED)
            work.cancelUniqueWork(NOW)
            return
        }
        val periodic = PeriodicWorkRequestBuilder<BackupWorker>(1, TimeUnit.HOURS).setConstraints(constraints(settings)).build()
        work.enqueueUniquePeriodicWork(PERIODIC, ExistingPeriodicWorkPolicy.UPDATE, periodic)
        watchForNewMedia()
    }

    /** A run that starts when the phone's photo library changes. */
    fun watchForNewMedia() {
        val settings = settingsFlow.value
        if (settings.serverId == null || settings.paused) return
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
        if (settings.serverId == null || settings.paused) return
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

    internal fun start(total: Int) {
        val now = System.currentTimeMillis()
        prefs.edit { putLong("runStarted", now) }
        setState { it.copy(running = true, done = 0, total = total, left = total, runStarted = now, runBytes = 0, current = null) }
    }

    /** Ends a run. [sent] files and [bytes] were sent by a run that got under way. */
    internal fun finish(result: String, left: Int? = null, sent: Int = 0, bytes: Long = 0) {
        val now = System.currentTimeMillis()
        val ran = stateFlow.value.running
        val duration = if (ran) now - stateFlow.value.runStarted else stateFlow.value.lastDurationMs
        prefs.edit {
            putLong("lastRun", now)
            putString("lastResult", result)
            if (left != null) putInt("left", left)
            if (ran) {
                putLong("lastDuration", duration)
                putLong("lastBytes", bytes)
                putInt("lastSent", sent)
                putLong("totalBytes", prefs.getLong("totalBytes", 0) + bytes)
                putInt("totalSent", prefs.getInt("totalSent", 0) + sent)
            }
        }
        trimRecent()
        setState {
            if (!ran) {
                it.copy(backedUp = countDone(), left = left ?: it.left, lastRun = now, lastResult = result)
            } else {
                it.copy(
                    running = false, current = null, backedUp = countDone(), left = left ?: it.left, lastRun = now, lastResult = result,
                    lastDurationMs = duration, lastBytes = bytes, lastSent = sent,
                    totalBytes = prefs.getLong("totalBytes", 0), totalSent = prefs.getInt("totalSent", 0),
                )
            }
        }
    }

    /** The photos and videos runs handled lately, newest first. */
    fun recent(): List<BackedUpItem> = if (!recentFile.isFile) emptyList() else recentFile.readLines().mapNotNull { line ->
        val parts = line.split('\t')
        if (parts.size < 5) return@mapNotNull null
        BackedUpItem(Uri.parse(parts[0]), parts[1], parts[2].toLongOrNull() ?: 0, parts[3].toLongOrNull() ?: 0, parts[4] != "0", parts[4] == "2")
    }.asReversed()

    /** `blocked`: deleted on the server, so it is not backed up again. */
    internal fun noteRecent(uri: Uri, name: String, size: Long, skipped: Boolean, blocked: Boolean) {
        val clean = name.replace('\t', ' ').replace('\n', ' ')
        val status = when {
            blocked -> 2
            skipped -> 1
            else -> 0
        }
        recentFile.appendText("$uri\t$clean\t$size\t${System.currentTimeMillis()}\t$status\n")
    }

    private fun trimRecent() {
        if (!recentFile.isFile) return
        val lines = recentFile.readLines()
        if (lines.size > RECENT * 2) recentFile.writeText(lines.takeLast(RECENT).joinToString("") { "$it\n" })
    }

    /** Keys of the photos on the server already ("<store id>:<size>:<modified>"). */
    internal fun doneKeys(): MutableSet<String> = if (doneFile.isFile) doneFile.readLines().filter { it.isNotBlank() }.toMutableSet() else mutableSetOf()

    private fun countDone(): Int = doneKeys().size

    internal fun blockedKeys(): Set<String> = if (blockedFile.isFile) blockedFile.readLines().filter { it.isNotBlank() }.toSet() else emptySet()

    internal fun markBlocked(key: String) {
        blockedFile.appendText("$key\n")
        setState { it.copy(left = (it.left - 1).coerceAtLeast(0)) }
    }

    /** Photos were allowed again on the server: ask about every blocked one on the next run. */
    fun forgetBlocked() {
        blockedFile.delete()
        prefs.edit { remove("blockedOnServer") }
    }

    /** Notices photos allowed again on the server (the web app, or another phone). */
    internal fun checkBlocked(onServer: Int) {
        if (onServer < prefs.getInt("blockedOnServer", 0)) blockedFile.delete()
        prefs.edit { putInt("blockedOnServer", onServer) }
    }

    internal fun markDone(key: String) {
        doneFile.appendText("$key\n")
        setState { it.copy(backedUp = it.backedUp + 1, left = (it.left - 1).coerceAtLeast(0)) }
    }

    companion object {
        private const val PERIODIC = "ownnas-backup"
        private const val TRIGGERED = "ownnas-backup-new-media"
        private const val NOW = "ownnas-backup-now"
        const val CHANNEL = "backup"
        const val NOTIFICATION = 7101
        private const val RECENT = 500
        /** Opens the backup screen from the notification. */
        const val SHOW = "app.ownnas.android.SHOW_BACKUP"
        const val PAUSE = "app.ownnas.android.PAUSE_BACKUP"
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
            // Cancelled (paused, or stopped by the system) part-way through a file.
            val backup = app.backup
            val state = backup.state.value
            if (state.running) {
                backup.finish(if (backup.settings.value.paused) "Paused" else "Interrupted; it carries on later", state.left, state.done, state.runBytes)
            }
            running.unlock()
        }
    }

    private suspend fun backUp(): Result {
        val backup = app.backup
        // The new-photos trigger fires once; watch for the next ones.
        backup.watchForNewMedia()
        val settings = backup.settings.value
        val serverId = settings.serverId ?: return Result.success()
        if (settings.paused) return Result.success()
        if (!backup.hasPermission()) {
            backup.finish("Allow OwnNAS to see your photos to back them up")
            return Result.success()
        }
        val folder = try {
            app.servers.call(serverId) { it.backupFolder() }
        } catch (err: ClientException.Unauthorized) {
            backup.finish("Sign in to the server again to keep backing up")
            return Result.success()
        } catch (err: Exception) {
            backup.finish(err.userMessage())
            return Result.retry()
        }
        if (folder == null) {
            backup.finish("The server has no media upload folder yet. An administrator can set one in the web app: Settings → Gallery → Phone backups.")
            return Result.success()
        }
        runCatching { app.servers.call(serverId) { it.blockedPhotos().size } }.onSuccess { backup.checkBlocked(it) }
        val done = backup.doneKeys() + backup.blockedKeys()
        val pending = media(settings.videos).filterNot { it.key in done }
        if (pending.isEmpty()) {
            backup.finish("Up to date", 0)
            return Result.success()
        }
        backup.start(pending.size)
        runCatching { setForeground(foreground(0, pending.size, null)) }
        val device = backup.deviceName()
        var added = 0
        var failed = 0
        var firstProblem: String? = null
        var sent = 0
        var blocked = 0
        var bytes = 0L
        for ((index, item) in pending.withIndex()) {
            if (isStopped || backup.settings.value.paused) break
            backup.setState { it.copy(current = item.name) }
            if (index > 0 && index % 5 == 0) runCatching { setForeground(foreground(index, pending.size, item.name)) }
            var lastShown = 0L
            val outcome = runCatching {
                val fd = applicationContext.contentResolver.openFileDescriptor(item.uri, "r")?.detachFd() ?: error("Could not open ${item.name}")
                app.servers.call(serverId) {
                    it.backupFd(device, item.name, fd, item.size.toULong(), item.takenMs, object : Transfer {
                        override fun keepGoing(done: ULong): Boolean {
                            val now = System.currentTimeMillis()
                            if (now - lastShown > 300) {
                                lastShown = now
                                backup.setState { s -> s.copy(runBytes = bytes + done.toLong()) }
                            }
                            return !isStopped && !backup.settings.value.paused
                        }
                    })
                }
            }
            outcome.onSuccess {
                if (it.blocked) backup.markBlocked(item.key) else backup.markDone(item.key)
                backup.noteRecent(item.uri, item.name, item.size, it.skipped, it.blocked)
                sent++
                if (it.blocked) blocked++
                if (!it.skipped) {
                    added++
                    bytes += item.size
                }
                backup.setState { s -> s.copy(runBytes = bytes) }
            }.onFailure { err ->
                if (isStopped || backup.settings.value.paused) return@onFailure
                failed++
                android.util.Log.w("OwnNAS", "Backing up ${item.name} failed", err)
                if (firstProblem == null) firstProblem = "${item.name}: ${err.userMessage()}"
                // Signed out, or the server or network keeps failing: try again later instead of
                // failing every photo. A single unreadable file is just skipped.
                if (err is ClientException.Unauthorized || failed >= 5) {
                    backup.finish("Stopped after ${index} of ${pending.size}: ${err.userMessage()}", pending.size - sent, added, bytes)
                    return Result.retry()
                }
            }
            backup.setState { it.copy(done = index + 1) }
        }
        val result = when {
            backup.settings.value.paused -> "Paused"
            isStopped -> "Interrupted; it carries on later"
            failed > 0 -> "Backed up ${added} new; ${failed} could not be sent (${firstProblem})"
            else -> "Backed up ${added} new photo${if (added == 1) "" else "s"}"
        } + if (blocked > 0) "; skipped ${blocked} deleted on the server" else ""
        backup.finish(result, pending.size - sent, added, bytes)
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

    private fun foreground(done: Int, total: Int, current: String?): ForegroundInfo {
        val manager = applicationContext.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && manager.getNotificationChannel(Backup.CHANNEL) == null) {
            manager.createNotificationChannel(NotificationChannel(Backup.CHANNEL, "Photo backup", NotificationManager.IMPORTANCE_LOW))
        }
        val notification = NotificationCompat.Builder(applicationContext, Backup.CHANNEL)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle("Backing up photos")
            .setContentText(if (current == null) "$done of $total" else "$done of $total · $current")
            .setProgress(total, done, false)
            .setContentIntent(openBackup())
            .addAction(0, "Pause", pauseBackup())
            .setOngoing(true)
            .setSilent(true)
            .build()
        return foregroundInfo(notification)
    }

    private fun openBackup(): PendingIntent {
        val intent = Intent(applicationContext, MainActivity::class.java)
            .setAction(Backup.SHOW)
            .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        return PendingIntent.getActivity(applicationContext, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun pauseBackup(): PendingIntent {
        val intent = Intent(applicationContext, BackupActions::class.java).setAction(Backup.PAUSE)
        return PendingIntent.getBroadcast(applicationContext, 0, intent, PendingIntent.FLAG_IMMUTABLE)
    }

    private fun foregroundInfo(notification: android.app.Notification): ForegroundInfo {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ForegroundInfo(Backup.NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(Backup.NOTIFICATION, notification)
        }
    }
}

/** The notification's Pause button. */
class BackupActions : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Backup.PAUSE) (context.applicationContext as OwnNasApp).backup.pause()
    }
}
