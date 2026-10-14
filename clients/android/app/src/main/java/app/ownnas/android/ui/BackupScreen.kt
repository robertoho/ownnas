package app.ownnas.android.ui

import android.Manifest
import android.content.Context
import android.util.Size
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import app.ownnas.android.data.BackedUpItem
import app.ownnas.android.data.BackupState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import android.os.Build
import android.text.format.DateUtils
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Card
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle

/** Backing this phone's photos and videos up to a server's media upload folder. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BackupScreen(vm: AppViewModel) {
    val settings by vm.backup.settings.collectAsStateWithLifecycle()
    val state by vm.backup.state.collectAsStateWithLifecycle()
    val servers by vm.servers.collectAsStateWithLifecycle()
    var allowed by remember { mutableStateOf(vm.backup.hasPermission()) }
    var folder by remember { mutableStateOf<String?>(null) }
    var folderError by remember { mutableStateOf<String?>(null) }
    var checked by remember { mutableIntStateOf(0) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        allowed = vm.backup.hasPermission()
        if (allowed) vm.backup.backUpNow()
    }
    fun askPermissions() {
        val wanted = vm.backup.permissions().toMutableList()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) wanted += Manifest.permission.POST_NOTIFICATIONS
        ask.launch(wanted.toTypedArray())
    }
    LaunchedEffect(settings.serverId, checked) {
        folder = null
        folderError = null
        val id = settings.serverId ?: return@LaunchedEffect
        runCatching { vm.call(id) { it.backupFolder() } }.onSuccess { found ->
            folder = found
            if (found == null) folderError = "This server has no media upload folder yet. An administrator can set one in the web app: Settings → Gallery → Phone backups."
        }.onFailure { folderError = it.userMessage() }
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Back up this phone") },
                navigationIcon = { IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") } },
            )
        },
    ) { padding ->
        Column(
            Modifier.padding(padding).fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Text(
                "New photos and videos on this phone are copied to the server, into a folder for this phone (“${vm.backup.deviceName()}”), by year and month. It only ever adds: deleting a photo on the phone does not delete the backup.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(vertical = 6.dp)) {
                    Text("Back up to", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp))
                    ChoiceRow("Off", settings.serverId == null) { vm.backup.update { it.copy(serverId = null) } }
                    for (server in servers) {
                        ChoiceRow("${server.name} (${server.username})", settings.serverId == server.id) {
                            vm.backup.update { it.copy(serverId = server.id) }
                            if (!vm.backup.hasPermission()) askPermissions() else vm.backup.backUpNow()
                        }
                    }
                }
            }
            if (settings.serverId != null) {
                Card(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(vertical = 6.dp)) {
                        SwitchRow("Only on Wi-Fi", "Keeps mobile data for other things.", settings.wifiOnly) { on -> vm.backup.update { it.copy(wifiOnly = on) } }
                        HorizontalDivider()
                        SwitchRow("Include videos", "Videos can be large.", settings.videos) { on ->
                            vm.backup.update { it.copy(videos = on) }
                            allowed = vm.backup.hasPermission()
                        }
                    }
                }
                when {
                    !allowed -> Card(Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text("OwnNAS needs to see your photos and videos to back them up.")
                            Button(onClick = { askPermissions() }) { Text("Allow access") }
                        }
                    }
                    folderError != null -> Card(Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text(folderError!!)
                            FilledTonalButton(onClick = { checked++ }) { Text("Check again") }
                        }
                    }
                    else -> {
                        StatusCard(vm, settings.paused, state, folder)
                        StatsCard(state)
                        RecentCard(vm, state.lastRun, state.backedUp)
                    }
                }
            }
        }
    }
}

@Composable
private fun ChoiceRow(label: String, selected: Boolean, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        RadioButton(selected = selected, onClick = onClick)
        Text(label)
    }
}

@Composable
private fun SwitchRow(title: String, detail: String, on: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().clickable { onChange(!on) }.padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(checked = on, onCheckedChange = onChange)
    }
}

@Composable
private fun StatusCard(vm: AppViewModel, paused: Boolean, state: BackupState, folder: String?) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            folder?.let { Text("Saved to “$it/${vm.backup.deviceName()}”", style = MaterialTheme.typography.titleSmall) }
            when {
                state.running -> {
                    Text("Backing up ${state.done} of ${state.total}…")
                    LinearProgressIndicator(progress = { if (state.total == 0) 0f else state.done.toFloat() / state.total }, modifier = Modifier.fillMaxWidth())
                    state.current?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                }
                paused -> Text("Backups are paused. Nothing is sent until you resume.")
                else -> {
                    val last = if (state.lastRun == 0L) "Not backed up yet" else "${DateUtils.getRelativeTimeSpanString(state.lastRun)}: ${state.lastResult.orEmpty()}"
                    Text(last)
                }
            }
            Text(
                buildString {
                    append("${state.backedUp} photos and videos from this phone are on the server")
                    if (state.left > 0) append("; ${state.left} still to send")
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (paused) {
                    Button(onClick = { vm.backup.resume() }) {
                        Icon(Icons.Filled.PlayArrow, contentDescription = null)
                        Text("Resume", Modifier.padding(start = 6.dp))
                    }
                } else {
                    FilledTonalButton(onClick = { vm.backup.backUpNow() }, enabled = !state.running) { Text("Back up now") }
                    OutlinedButton(onClick = { vm.backup.pause() }) {
                        Icon(Icons.Filled.Pause, contentDescription = null)
                        Text("Pause", Modifier.padding(start = 6.dp))
                    }
                }
            }
        }
    }
}

@Composable
private fun StatsCard(state: BackupState) {
    val context = LocalContext.current
    // Ticks once a second while running, so the elapsed time and speed stay current.
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(state.running) {
        while (state.running) {
            now = System.currentTimeMillis()
            delay(1000)
        }
    }
    val rows = buildList {
        if (state.running) {
            val elapsed = (now - state.runStarted).coerceAtLeast(1)
            add("This run" to "${state.done} of ${state.total} · ${bytes(state.runBytes)} in ${duration(elapsed)}")
            add("Speed" to speed(state.runBytes, elapsed))
        } else if (state.lastRun != 0L && state.runStarted != 0L) {
            add("Last run" to DateUtils.formatDateTime(context, state.runStarted, DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_SHOW_TIME or DateUtils.FORMAT_ABBREV_MONTH))
            add("Took" to duration(state.lastDurationMs))
            add("Sent" to "${state.lastSent} new · ${bytes(state.lastBytes)}")
            if (state.lastBytes > 0) add("Speed" to speed(state.lastBytes, state.lastDurationMs))
        }
        add("On the server" to "${state.backedUp} photos and videos")
        if (state.totalSent > 0) add("Sent in total" to "${state.totalSent} · ${bytes(state.totalBytes)}")
    }
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("Statistics", style = MaterialTheme.typography.titleSmall)
            for ((label, value) in rows) {
                Row(Modifier.fillMaxWidth()) {
                    Text(label, Modifier.weight(1f), color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(value)
                }
            }
        }
    }
}

@Composable
private fun RecentCard(vm: AppViewModel, lastRun: Long, backedUp: Int) {
    val context = LocalContext.current
    var items by remember { mutableStateOf<List<BackedUpItem>>(emptyList()) }
    LaunchedEffect(lastRun, backedUp / 10) {
        items = withContext(Dispatchers.IO) { vm.backup.recent().take(200) }
    }
    if (items.isEmpty()) return
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(vertical = 10.dp)) {
            Text("Recently backed up", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp))
            for (item in items) {
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    LocalThumb(context, item)
                    Column(Modifier.weight(1f).padding(start = 12.dp)) {
                        Text(item.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            "${bytes(item.size)} · ${DateUtils.getRelativeTimeSpanString(item.at)}${if (item.skipped) " · already on the server" else ""}",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun LocalThumb(context: Context, item: BackedUpItem) {
    var bitmap by remember(item.uri) { mutableStateOf<ImageBitmap?>(null) }
    LaunchedEffect(item.uri) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            bitmap = withContext(Dispatchers.IO) {
                runCatching { context.contentResolver.loadThumbnail(item.uri, Size(144, 144), null).asImageBitmap() }.getOrNull()
            }
        }
    }
    val modifier = Modifier.size(48.dp).clip(RoundedCornerShape(6.dp)).background(MaterialTheme.colorScheme.surfaceVariant)
    bitmap?.let { Image(it, contentDescription = null, modifier = modifier, contentScale = ContentScale.Crop) } ?: Box(modifier)
}

private fun bytes(n: Long): String {
    if (n < 1024) return "$n B"
    var value = n.toDouble()
    for (unit in listOf("KB", "MB", "GB", "TB")) {
        value /= 1024
        if (value < 1024 || unit == "TB") return if (value < 10) "%.1f %s".format(value, unit) else "%.0f %s".format(value, unit)
    }
    return "$n B"
}

private fun duration(ms: Long): String {
    val s = ms / 1000
    return when {
        s < 60 -> "${s}s"
        s < 3600 -> "${s / 60}m ${s % 60}s"
        else -> "${s / 3600}h ${s / 60 % 60}m"
    }
}

private fun speed(bytes: Long, ms: Long): String = "${bytes(bytes * 1000 / ms.coerceAtLeast(1))}/s"
