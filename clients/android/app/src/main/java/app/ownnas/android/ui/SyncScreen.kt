package app.ownnas.android.ui

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.text.format.DateUtils
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.outlined.Sync
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.ownnas.android.data.SyncPair
import app.ownnas.android.data.Syncs

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SyncScreen(vm: AppViewModel) {
    val pairs by vm.syncs.pairs.collectAsStateWithLifecycle()
    val running by vm.syncs.running.collectAsStateWithLifecycle()
    var removing by remember { mutableStateOf<SyncPair?>(null) }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Sync") },
                navigationIcon = {
                    IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
                },
            )
        },
    ) { padding ->
        if (pairs.isEmpty()) {
            Box(Modifier.padding(padding).fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
                Text(
                    "Nothing syncs yet. Open a folder on a server and choose “Sync with this phone” from its menu.",
                    textAlign = TextAlign.Center,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        } else {
            LazyColumn(
                modifier = Modifier.padding(padding).fillMaxSize(),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                items(pairs, key = { it.id }) { pair ->
                    PairCard(vm, pair, pair.id in running, onSync = { vm.syncNow(pair.id) }, onRemove = { removing = pair })
                }
                item {
                    Text(
                        "Syncs run every 15 minutes while the phone has a network connection. Deleted files go to the " +
                            "server's Trash, or to .ownnas-sync/deleted on the phone for 30 days. When both sides change a " +
                            "file, both versions are kept.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
    removing?.let { pair ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Stop syncing?") },
            text = { Text("Both folders keep their files; they just stop being kept in step.") },
            confirmButton = { TextButton(onClick = { vm.syncs.remove(pair.id); removing = null }) { Text("Stop syncing") } },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun PairCard(vm: AppViewModel, pair: SyncPair, running: Boolean, onSync: () -> Unit, onRemove: () -> Unit) {
    val server = vm.server(pair.serverId)
    var menu by remember { mutableStateOf(false) }
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Outlined.Sync, contentDescription = null, tint = MaterialTheme.colorScheme.primary)
                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                    Text(
                        "${server?.name ?: "Removed server"}/${pair.remote}".trimEnd('/'),
                        style = MaterialTheme.typography.titleSmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        "⇄ ${pair.local.removePrefix("/storage/emulated/0/").ifEmpty { "Internal storage" }}",
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Box {
                    IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "More") }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(text = { Text("Stop syncing") }, onClick = { menu = false; onRemove() })
                    }
                }
            }
            val last = if (pair.lastRun == 0L) {
                "Not synced yet"
            } else {
                "${DateUtils.getRelativeTimeSpanString(pair.lastRun, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS)}: ${pair.lastResult ?: ""}"
            }
            Text(last, style = MaterialTheme.typography.bodyMedium)
            pair.lastNotices.take(5).forEach {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2)
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                FilledTonalButton(onClick = onSync, enabled = !running) {
                    if (running) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp) else Text("Sync now")
                }
                if (running) Text("Syncing…", modifier = Modifier.padding(start = 12.dp), style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

/**
 * "Sync with this phone" for the server folder `remote`: makes sure the app may use folders
 * by path, lets the user pick the phone's folder, explains what two-way sync does, then starts it.
 */
@Composable
fun rememberSyncSetup(vm: AppViewModel, serverId: String, remote: String): () -> Unit {
    val context = LocalContext.current
    var step by remember { mutableStateOf<SyncStep?>(null) }
    val pickFolder = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree ->
        val path = tree?.let(Syncs::pathOf)
        step = when {
            tree == null -> null
            path == null -> SyncStep.Unsupported
            else -> SyncStep.Confirm(path)
        }
    }
    val askLegacy = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) pickFolder.launch(null) else step = null
    }
    when (val current = step) {
        SyncStep.NeedsAccess -> AlertDialog(
            onDismissRequest = { step = null },
            title = { Text("Allow access to folders") },
            text = {
                Text(
                    "To keep a phone folder such as DCIM or Documents in sync, OwnNAS needs “All files access”. " +
                        "Turn it on in the next screen, then come back and choose “Sync with this phone” again.",
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    step = null
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                        context.startActivity(
                            Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:${context.packageName}")),
                        )
                    } else {
                        askLegacy.launch(android.Manifest.permission.WRITE_EXTERNAL_STORAGE)
                    }
                }) { Text("Continue") }
            },
            dismissButton = { TextButton(onClick = { step = null }) { Text("Cancel") } },
        )
        SyncStep.Unsupported -> AlertDialog(
            onDismissRequest = { step = null },
            title = { Text("Choose a folder on this phone") },
            text = { Text("That location cannot be synced. Pick a folder in the phone's storage or on its SD card.") },
            confirmButton = { TextButton(onClick = { step = null }) { Text("OK") } },
        )
        is SyncStep.Confirm -> AlertDialog(
            onDismissRequest = { step = null },
            title = { Text("Start syncing?") },
            text = {
                Text(
                    "“${current.path.removePrefix("/storage/emulated/0/")}” on this phone and “/$remote” on the server " +
                        "will hold the same files. What is in either one now is copied to the other, and later changes, " +
                        "including deletions, go both ways.",
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    step = null
                    val pair = vm.syncs.add(serverId, remote, current.path)
                    vm.push(Screen.Sync)
                    vm.syncNow(pair.id)
                }) { Text("Start syncing") }
            },
            dismissButton = { TextButton(onClick = { step = null }) { Text("Cancel") } },
        )
        null -> {}
    }
    return {
        if (Syncs.hasStorageAccess(context)) pickFolder.launch(null) else step = SyncStep.NeedsAccess
    }
}

private sealed interface SyncStep {
    data object NeedsAccess : SyncStep
    data object Unsupported : SyncStep
    data class Confirm(val path: String) : SyncStep
}
