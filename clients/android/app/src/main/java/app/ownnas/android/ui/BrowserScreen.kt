package app.ownnas.android.ui

import android.content.ActivityNotFoundException
import android.content.Context
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.InsertDriveFile
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.outlined.Audiotrack
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Inventory2
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.FolderZip
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.Movie
import androidx.compose.material.icons.outlined.PictureAsPdf
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.ownnas.core.ClientException
import app.ownnas.core.Entry
import app.ownnas.core.Folder
import kotlinx.coroutines.launch

/** A dialog the browser is showing. */
private sealed interface Dialog {
    data object NewFolder : Dialog
    data class Rename(val entry: Entry) : Dialog
    data class Trash(val entry: Entry) : Dialog
}

/** A file being fetched to open or share. */
private class Fetch(val entry: Entry, val share: Boolean) {
    val tracker = Tracker { done = it }
    var done by mutableStateOf(0L)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BrowserScreen(vm: AppViewModel, serverId: String, path: String) {
    val server = vm.server(serverId) ?: return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var folder by remember { mutableStateOf(vm.cachedFolder(serverId, path)) }
    var error by remember { mutableStateOf<Throwable?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    var reload by remember { mutableIntStateOf(0) }
    var fetching by remember { mutableStateOf<Fetch?>(null) }
    var dialog by remember { mutableStateOf<Dialog?>(null) }
    var adding by remember { mutableStateOf(false) }
    val revision by vm.revision.collectAsStateWithLifecycle()
    val uploads by vm.uploads.collectAsStateWithLifecycle()
    val account by produceState<app.ownnas.core.Account?>(null, serverId) {
        value = runCatching { vm.account(serverId) }.getOrNull()
    }
    val writable = account?.readonly == false

    LaunchedEffect(reload, revision) {
        refreshing = true
        try {
            folder = vm.list(serverId, path)
            error = null
        } catch (err: Exception) {
            error = err
        } finally {
            refreshing = false
        }
    }

    // Keyed on a plain flag: clearing the finished status must not restart this and cancel the message.
    val uploading = uploads?.active == true
    LaunchedEffect(uploading) {
        if (!uploading) vm.takeUploadSummary()?.let { snackbar.showSnackbar(it) }
    }

    fun report(message: String) {
        scope.launch { snackbar.showSnackbar(message) }
    }

    fun upload(uris: List<android.net.Uri>) {
        if (uris.isEmpty()) return
        val (pending, unreadable) = context.contentResolver.pendingUploads(serverId, path, uris)
        vm.upload(pending)
        if (unreadable > 0) report("$unreadable could not be read and were skipped")
    }

    val pickMedia = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(), ::upload)
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments(), ::upload)

    fun fetch(entry: Entry, share: Boolean) {
        val job = Fetch(entry, share)
        fetching = job
        scope.launch {
            try {
                val file = vm.download(serverId, entry, job.tracker)
                if (fetching === job) {
                    if (share) shareWithAnotherApp(context, file) else openWithAnotherApp(context, file)
                }
            } catch (err: ActivityNotFoundException) {
                report("No app on this phone can open ${entry.name}")
            } catch (err: Exception) {
                if (!err.isCancel()) report(err.userMessage())
            } finally {
                if (fetching === job) fetching = null
            }
        }
    }

    fun open(entry: Entry) {
        if (entry.dir) vm.push(Screen.Browse(serverId, entry.path)) else fetch(entry, share = false)
    }

    fun act(action: suspend () -> String?) {
        scope.launch {
            try {
                action()?.let(::report)
            } catch (err: Exception) {
                report(err.userMessage())
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(specialFolderLabel(path.substringAfterLast('/')).ifEmpty { server.name }, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        if (path.isNotEmpty()) {
                            Text(
                                "${server.name}/" + path.split('/').joinToString("/", transform = ::specialFolderLabel),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                },
                navigationIcon = {
                    IconButton(onClick = { vm.pop() }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                actions = {
                    // Shown to administrators when terminals are turned on, as in the web app.
                    if (account?.terminal == true) {
                        IconButton(onClick = { vm.push(Screen.Terminals(serverId, path)) }) {
                            Icon(Icons.Outlined.Terminal, contentDescription = "Terminals")
                        }
                    }
                },
            )
        },
        floatingActionButton = {
            if (writable && folder != null) {
                FloatingActionButton(onClick = { adding = true }) { Icon(Icons.Filled.Add, contentDescription = "Add") }
            }
        },
        bottomBar = { uploads?.takeIf { it.active }?.let { UploadBar(it, onCancel = vm::cancelUploads) } },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        PullToRefreshBox(
            isRefreshing = refreshing && folder != null,
            onRefresh = { reload++ },
            modifier = Modifier.padding(padding).fillMaxSize(),
        ) {
            val shown = folder
            val failure = error
            when {
                failure != null && shown == null -> Problem(failure, onRetry = { reload++ }, onSignIn = { vm.push(Screen.SignIn(server)) })
                shown == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
                shown.entries.isEmpty() -> Message(if (writable) "This folder is empty. Tap + to add files." else "This folder is empty")
                else -> Listing(
                    vm = vm,
                    serverId = serverId,
                    folder = shown,
                    writable = writable,
                    onOpen = ::open,
                    onAction = { action, entry ->
                        when (action) {
                            EntryAction.Share -> fetch(entry, share = true)
                            EntryAction.Rename -> dialog = Dialog.Rename(entry)
                            EntryAction.Trash -> dialog = Dialog.Trash(entry)
                        }
                    },
                )
            }
        }
    }

    if (adding) {
        AddSheet(
            onDismiss = { adding = false },
            onMedia = {
                adding = false
                pickMedia.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageAndVideo))
            },
            onFiles = {
                adding = false
                pickFiles.launch(arrayOf("*/*"))
            },
            onFolder = {
                adding = false
                dialog = Dialog.NewFolder
            },
        )
    }

    when (val shown = dialog) {
        Dialog.NewFolder -> NameDialog(
            title = "New folder",
            initial = "",
            confirm = "Create",
            onDismiss = { dialog = null },
            onConfirm = { name ->
                dialog = null
                act { vm.createFolder(serverId, path, name); null }
            },
        )
        is Dialog.Rename -> NameDialog(
            title = "Rename",
            initial = shown.entry.name,
            confirm = "Rename",
            selectStem = !shown.entry.dir,
            onDismiss = { dialog = null },
            onConfirm = { name ->
                dialog = null
                if (name != shown.entry.name) act { vm.rename(serverId, shown.entry, name); null }
            },
        )
        is Dialog.Trash -> TrashDialog(
            entry = shown.entry,
            onDismiss = { dialog = null },
            onConfirm = {
                dialog = null
                act {
                    vm.trash(serverId, shown.entry)
                    if (inTrash(shown.entry.path)) "Deleted ${shown.entry.name}" else "Moved ${shown.entry.name} to the Trash"
                }
            },
        )
        null -> {}
    }

    fetching?.let { job ->
        FetchDialog(
            name = job.entry.name,
            done = job.done,
            size = job.entry.size.toLong(),
            sharing = job.share,
            onCancel = {
                job.tracker.cancelled = true
                fetching = null
            },
        )
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Listing(
    vm: AppViewModel,
    serverId: String,
    folder: Folder,
    writable: Boolean,
    onOpen: (Entry) -> Unit,
    onAction: (EntryAction, Entry) -> Unit,
) {
    val context = LocalContext.current
    val entries = remember(folder) {
        folder.entries.sortedWith(compareBy<Entry> { !it.dir }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name })
    }
    // Room under the last row for the add button.
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 88.dp)) {
        items(entries, key = { it.path }) { entry ->
            var menu by remember { mutableStateOf(false) }
            ListItem(
                modifier = Modifier.combinedClickable(onClick = { onOpen(entry) }, onLongClick = { menu = true }),
                leadingContent = { Thumbnail(vm, serverId, entry) },
                headlineContent = { Text(specialFolderLabel(entry.name), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                supportingContent = { Text(details(context, entry), maxLines = 1) },
                trailingContent = {
                    Box {
                        IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "Actions for ${entry.name}") }
                        EntryMenu(entry, writable, expanded = menu, onDismiss = { menu = false }) { onAction(it, entry) }
                    }
                },
            )
        }
        if (folder.truncated) {
            item { Message("Showing the first ${folder.entries.size} items. Open this folder in the browser to see the rest.") }
        }
    }
}

@Composable
private fun Thumbnail(vm: AppViewModel, serverId: String, entry: Entry) {
    val modifier = Modifier.size(44.dp).clip(RoundedCornerShape(8.dp))
    val key = "$serverId:${entry.path}:${entry.modified}"
    val image by produceState(vm.cachedThumbnail(key), key) {
        if (value == null && entry.thumb) value = vm.thumbnail(serverId, entry, key)
    }
    val bitmap = image
    if (bitmap != null) {
        Image(bitmap, contentDescription = null, contentScale = ContentScale.Crop, modifier = modifier)
    } else {
        Box(modifier, contentAlignment = Alignment.Center) {
            Icon(
                iconFor(entry),
                contentDescription = null,
                tint = if (entry.dir) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(32.dp),
            )
        }
    }
}

@Composable
private fun Problem(error: Throwable, onRetry: () -> Unit, onSignIn: () -> Unit) {
    val signedOut = error is ClientException.Unauthorized
    Column(
        modifier = Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(error.userMessage(), textAlign = TextAlign.Center, style = MaterialTheme.typography.bodyLarge)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (signedOut) Button(onClick = onSignIn) { Text("Sign in") } else Button(onClick = onRetry) { Text("Try again") }
        }
    }
}

@Composable
private fun Message(text: String) {
    Box(Modifier.fillMaxWidth().padding(32.dp), contentAlignment = Alignment.Center) {
        Text(text, textAlign = TextAlign.Center, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** The web app's names for OwnNAS's own folders. */
fun specialFolderLabel(name: String) = when (name) {
    ".ownnas-trash", "Trash" -> "Trash"
    ".ownnas-archive", "Archive" -> "Archive"
    else -> name
}

fun inTrash(path: String) =
    path == ".ownnas-trash" || path.startsWith(".ownnas-trash/") || path == "Trash" || path.startsWith("Trash/")

private fun iconFor(entry: Entry): ImageVector = when {
    entry.kind == "trash-folder" || entry.path == ".ownnas-trash" -> Icons.Outlined.Delete
    entry.kind == "archive-folder" || entry.path == ".ownnas-archive" -> Icons.Outlined.Inventory2
    entry.dir -> Icons.Filled.Folder
    else -> when (entry.kind) {
        "image", "svg" -> Icons.Outlined.Image
        "video" -> Icons.Outlined.Movie
        "audio" -> Icons.Outlined.Audiotrack
        "pdf" -> Icons.Outlined.PictureAsPdf
        "text", "spreadsheet" -> Icons.Outlined.Description
        "archive" -> Icons.Outlined.FolderZip
        else -> Icons.AutoMirrored.Outlined.InsertDriveFile
    }
}

private fun details(context: Context, entry: Entry): String {
    val now = System.currentTimeMillis()
    val modified = if (now - entry.modified in 0 until DateUtils.MINUTE_IN_MILLIS) {
        "Just now"
    } else {
        DateUtils.getRelativeTimeSpanString(entry.modified, now, DateUtils.MINUTE_IN_MILLIS)
    }
    return if (entry.dir) "Folder · $modified" else "${Formatter.formatShortFileSize(context, entry.size.toLong())} · $modified"
}
