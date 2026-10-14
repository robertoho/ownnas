package app.ownnas.android.ui

import android.Manifest
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
import androidx.compose.material3.Button
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
                    else -> Card(Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            folder?.let { Text("Saved to “$it/${vm.backup.deviceName()}”", style = MaterialTheme.typography.titleSmall) }
                            if (state.running) {
                                Text("Backing up ${state.done} of ${state.total}…")
                                LinearProgressIndicator(progress = { if (state.total == 0) 0f else state.done.toFloat() / state.total }, modifier = Modifier.fillMaxWidth())
                            } else {
                                val last = if (state.lastRun == 0L) "Not backed up yet" else "${DateUtils.getRelativeTimeSpanString(state.lastRun)}: ${state.lastResult.orEmpty()}"
                                Text(last)
                            }
                            Text("${state.backedUp} photos and videos backed up from this phone", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            FilledTonalButton(onClick = { vm.backup.backUpNow() }, enabled = !state.running) { Text("Back up now") }
                        }
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
