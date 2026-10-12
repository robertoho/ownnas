package app.ownnas.android.data

import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.Settings
import androidx.core.content.edit
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import app.ownnas.android.OwnNasApp
import app.ownnas.android.ui.userMessage
import app.ownnas.core.ClientException
import app.ownnas.core.SyncSummary
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID
import java.util.concurrent.TimeUnit

/** A folder on the phone kept in two-way sync with a folder on a server. */
data class SyncPair(
    val id: String,
    val serverId: String,
    /** Folder on the server, relative to its shared folder. */
    val remote: String,
    /** Absolute path of the folder on the phone. */
    val local: String,
    /** Milliseconds since the Unix epoch of the last finished run, or 0. */
    val lastRun: Long = 0,
    /** What the last run did, or why it failed. */
    val lastResult: String? = null,
    val lastNotices: List<String> = emptyList(),
)

/** Sync pairs, their runs, and the background schedule. */
class Syncs(private val context: Context, private val servers: Servers) {
    private val prefs = context.getSharedPreferences("sync", Context.MODE_PRIVATE)
    private val list = MutableStateFlow(load())
    val pairs: StateFlow<List<SyncPair>> = list.asStateFlow()
    private val runningIds = MutableStateFlow<Set<String>>(emptySet())
    val running: StateFlow<Set<String>> = runningIds.asStateFlow()
    private val lock = Mutex()

    fun add(serverId: String, remote: String, local: String): SyncPair {
        val pair = SyncPair(UUID.randomUUID().toString(), serverId, remote, local)
        save(list.value + pair)
        schedule()
        return pair
    }

    fun remove(id: String) {
        save(list.value.filterNot { it.id == id })
        schedule()
    }

    /** Runs one pair, or all of them; one run at a time, whether from the app or the schedule. */
    suspend fun run(only: String? = null) = lock.withLock {
        for (pair in list.value.filter { only == null || it.id == only }) {
            if (servers.store.find(pair.serverId) == null) continue
            runningIds.update { it + pair.id }
            val (result, notices) = try {
                val summary = servers.call(pair.serverId) { it.syncFolder(pair.local, pair.remote, deviceName()) }
                describe(summary) to summary.notices
            } catch (err: ClientException.Unauthorized) {
                "Sign in to the server again to keep syncing" to emptyList()
            } catch (err: Exception) {
                err.userMessage() to emptyList()
            } finally {
                runningIds.update { it - pair.id }
            }
            update(pair.id) { it.copy(lastRun = System.currentTimeMillis(), lastResult = result, lastNotices = notices) }
            servers.changed(pair.serverId, pair.remote)
        }
    }

    /** Every 15 minutes while a network is up, as long as there is something to sync. */
    fun schedule() {
        val work = WorkManager.getInstance(context)
        if (list.value.isEmpty()) {
            work.cancelUniqueWork(WORK)
            return
        }
        val request = PeriodicWorkRequestBuilder<SyncWorker>(15, TimeUnit.MINUTES)
            .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
            .build()
        work.enqueueUniquePeriodicWork(WORK, ExistingPeriodicWorkPolicy.KEEP, request)
    }

    private fun deviceName(): String =
        Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)?.takeIf { it.isNotBlank() } ?: Build.MODEL

    private fun update(id: String, change: (SyncPair) -> SyncPair) = save(list.value.map { if (it.id == id) change(it) else it })

    private fun save(pairs: List<SyncPair>) {
        val array = JSONArray(
            pairs.map { pair ->
                JSONObject()
                    .put("id", pair.id)
                    .put("serverId", pair.serverId)
                    .put("remote", pair.remote)
                    .put("local", pair.local)
                    .put("lastRun", pair.lastRun)
                    .put("lastResult", pair.lastResult ?: "")
                    .put("lastNotices", JSONArray(pair.lastNotices))
            },
        )
        prefs.edit { putString(KEY, array.toString()) }
        list.value = pairs
    }

    private fun load(): List<SyncPair> {
        val array = runCatching { JSONArray(prefs.getString(KEY, "[]")) }.getOrDefault(JSONArray())
        return (0 until array.length()).map { index ->
            val item = array.getJSONObject(index)
            val notices = item.optJSONArray("lastNotices")
            SyncPair(
                id = item.getString("id"),
                serverId = item.getString("serverId"),
                remote = item.getString("remote"),
                local = item.getString("local"),
                lastRun = item.optLong("lastRun"),
                lastResult = item.optString("lastResult").takeIf { it.isNotEmpty() },
                lastNotices = if (notices == null) emptyList() else (0 until notices.length()).map(notices::getString),
            )
        }
    }

    companion object {
        private const val KEY = "pairs"
        private const val WORK = "ownnas-sync"

        /** Whether the app may read and write folders by path, which sync needs. */
        fun hasStorageAccess(context: Context): Boolean = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            Environment.isExternalStorageManager()
        } else {
            context.checkSelfPermission(android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == android.content.pm.PackageManager.PERMISSION_GRANTED
        }

        /** The path of a folder picked with the system folder picker, on internal storage or an SD card. */
        fun pathOf(tree: Uri): String? {
            if (tree.authority != "com.android.externalstorage.documents") return null
            val id = DocumentsContract.getTreeDocumentId(tree)
            val volume = id.substringBefore(':')
            val relative = id.substringAfter(':', "")
            val base = if (volume == "primary") Environment.getExternalStorageDirectory().path else "/storage/$volume"
            return if (relative.isEmpty()) base else "$base/$relative"
        }

        fun describe(summary: SyncSummary): String {
            val parts = listOfNotNull(
                summary.uploaded.takeIf { it > 0u }?.let { "$it uploaded" },
                summary.downloaded.takeIf { it > 0u }?.let { "$it downloaded" },
                summary.deletedOnServer.takeIf { it > 0u }?.let { "$it moved to the server's Trash" },
                summary.deletedHere.takeIf { it > 0u }?.let { "$it removed here" },
                summary.folders.takeIf { it > 0u }?.let { "$it folders" },
                summary.conflicts.takeIf { it > 0u }?.let { "$it conflicts kept as copies" },
                summary.skipped.takeIf { it > 0u }?.let { "$it skipped" },
                summary.failed.takeIf { it > 0u }?.let { "$it failed" },
                summary.held.takeIf { it > 0u }?.let { "$it changes held (server is read-only)" },
            )
            return if (parts.isEmpty()) "Up to date" else parts.joinToString(", ")
        }
    }
}

/** Runs every sync pair in the background. */
class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        if (!Syncs.hasStorageAccess(applicationContext)) return Result.success()
        (applicationContext as OwnNasApp).syncs.run()
        return Result.success()
    }
}
