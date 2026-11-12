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
import androidx.compose.foundation.clickable
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
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.outlined.Audiotrack
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Inventory2
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.FolderZip
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.Movie
import androidx.compose.material.icons.outlined.PictureAsPdf
import androidx.compose.material.icons.outlined.PhotoLibrary
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material.icons.outlined.Sync
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
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
import androidx.compose.foundation.background
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.SelectAll
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material.icons.outlined.DeleteForever
import androidx.compose.material.icons.outlined.ContentPaste
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.TextButton
import androidx.compose.material.icons.outlined.Bookmark
import androidx.compose.material.icons.outlined.BookmarkBorder
import androidx.compose.material.icons.outlined.Bookmarks
import androidx.compose.material.icons.outlined.DataUsage
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material.icons.automirrored.outlined.NoteAdd
import androidx.compose.material.icons.outlined.FileCopy
import androidx.compose.material.icons.outlined.History
import androidx.compose.material.icons.automirrored.outlined.DriveFileMove
import androidx.compose.material.icons.outlined.SmartToy
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Surface
import androidx.compose.ui.graphics.Color
import app.ownnas.android.ui.terminal.EndedBadge
import app.ownnas.android.ui.terminal.agentStatus
import app.ownnas.android.ui.terminal.endedText
import app.ownnas.core.TerminalInfo
import kotlinx.coroutines.delay
import app.ownnas.core.ClientException
import app.ownnas.core.Entry
import app.ownnas.core.Folder
import kotlinx.coroutines.launch

/** A dialog the browser is showing. */
private sealed interface Dialog {
    data object NewFolder : Dialog
    data class Rename(val entry: Entry) : Dialog
    data class Trash(val entries: List<Entry>) : Dialog
    data class Tag(val entries: List<Entry>) : Dialog
    data class NewFolderWith(val entries: List<Entry>) : Dialog
    /** Choosing where to move or copy. */
    data class Pick(val entries: List<Entry>, val copy: Boolean) : Dialog
    data class Compress(val entries: List<Entry>) : Dialog
    data class Convert(val entries: List<Entry>) : Dialog
    data class Resize(val entries: List<Entry>) : Dialog
    /** For folders: only the folder itself, or everything in it too. */
    data class Protect(val entries: List<Entry>) : Dialog
    data class PdfPages(val entry: Entry) : Dialog
    data class PdfRotate(val entry: Entry) : Dialog
    data class Versions(val entry: Entry) : Dialog
    data class Album(val entries: List<Entry>) : Dialog
    data object EmptyTrash : Dialog
    data class Video(val entries: List<Entry>) : Dialog
    data object NewFileKind : Dialog
    data class NewFileName(val kind: String, val label: String) : Dialog
    /** How much a folder holds; `usage` is null while it is counted. */
    data class Usage(val path: String, val usage: app.ownnas.core.FolderUsage?, val problem: String? = null) : Dialog
    data class Bookmarks(val paths: List<String>?) : Dialog
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
    // Paths of the selected rows; long-press a row to start.
    var selected by remember(serverId, path) { mutableStateOf<Set<String>>(emptySet()) }
    var bulk by remember { mutableStateOf<BulkProgress?>(null) }
    val revision by vm.revision.collectAsStateWithLifecycle()
    val uploads by vm.uploads.collectAsStateWithLifecycle()
    val clip by vm.clipboard.collectAsStateWithLifecycle()
    val showHidden by vm.showHidden.collectAsStateWithLifecycle()
    var bookmarked by remember(serverId, path) { mutableStateOf<Boolean?>(null) }
    LaunchedEffect(serverId, path) {
        bookmarked = runCatching { path in vm.bookmarks(serverId) }.getOrNull()
    }
    val account by produceState<app.ownnas.core.Account?>(null, serverId) {
        value = runCatching { vm.account(serverId) }.getOrNull()
    }
    val writable = account?.readonly == false
    val terminalsOn = account?.terminal == true

    // Follow the server's terminals for badges, as the web app does in the background.
    var terminals by remember { mutableStateOf<List<TerminalInfo>>(emptyList()) }
    LaunchedEffect(serverId, terminalsOn) {
        while (terminalsOn) {
            runCatching { vm.terminals(serverId) }.onSuccess { terminals = it }
            delay(3000)
        }
    }
    // Like the web app's badge: terminals whose agent wants attention (waiting, finished,
    // stopped) until someone opens them.
    // Ended terminals count too: they stay until closed, and the user should see what happened.
    val attention = terminals.count { it.agent.attention != null || !it.running }
    // Coding agents an administrator enabled, offered as "Open … here".
    var agents by remember(serverId) { mutableStateOf<List<app.ownnas.core.AgentLauncher>>(emptyList()) }
    LaunchedEffect(serverId, terminalsOn) {
        agents = if (terminalsOn) runCatching { vm.call(serverId) { it.agentLaunchers() } }.getOrDefault(emptyList()) else emptyList()
    }
    val terminalsByPath = remember(terminals) { terminals.groupBy { it.path } }

    LaunchedEffect(reload, revision, showHidden) {
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

    // Items that are gone after a reload drop out of the selection.
    LaunchedEffect(folder) {
        val present = folder?.entries.orEmpty().mapTo(HashSet()) { it.path }
        if (selected.any { it !in present }) selected = selected.filterTo(HashSet()) { it in present }
    }
    val selecting = selected.isNotEmpty()
    val selectedEntries = folder?.entries.orEmpty().filter { it.path in selected }
    androidx.activity.compose.BackHandler(enabled = selecting) { selected = emptySet() }

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

    val setUpSync = rememberSyncSetup(vm, serverId, path)

    val pickMedia = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(), ::upload)
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments(), ::upload)

    var scan by remember { mutableStateOf<Scan?>(null) }
    val openScanner = rememberDocumentScanner(onScanned = { scan = it }, onError = ::report)

    fun uploadScan(done: Scan, name: String, pdf: Boolean) {
        val batch = System.nanoTime()
        val files = done.files(name, pdf)
        done.discard(keep = files.map { it.second })
        vm.upload(files.map { (file, scanned) ->
            val type = if (file.endsWith(".pdf")) "application/pdf" else "image/jpeg"
            PendingUpload(batch, serverId, path, file, scanned.file.statSize.takeIf { it >= 0 }, done.taken, scanned.file, type)
        })
    }

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

    /** Opens a terminal in `folder`, starting `agent` in it when given. */
    fun openTerminalIn(folder: String, agent: app.ownnas.core.AgentLauncher? = null) {
        scope.launch {
            try {
                val info = if (agent == null) {
                    vm.openTerminal(serverId, folder)
                } else {
                    vm.call(serverId) { it.openTerminalWithAgent(folder, 30u, 100u, agent.id) }
                }
                vm.push(Screen.Terminal(serverId, info.id))
            } catch (err: ClientException.Locked) {
                // The terminals screen asks for the terminal password.
                vm.push(Screen.Terminals(serverId, folder))
            } catch (err: Exception) {
                snackbar.showSnackbar(err.userMessage())
            }
        }
    }

    /** Runs `op` on each item in turn with a progress dialog, then says how it went. */
    fun runBulk(title: String, done: String, items: List<Entry>, op: suspend (Entry) -> Unit) {
        if (items.isEmpty()) return
        val job = BulkProgress(title, items.size)
        bulk = job
        selected = emptySet()
        scope.launch {
            var ok = 0
            val failures = mutableListOf<String>()
            for (item in items) {
                if (job.stopped) break
                job.current = item.name
                try {
                    op(item)
                    ok++
                } catch (err: Exception) {
                    failures += "${item.name}: ${err.userMessage()}"
                }
                job.done++
            }
            bulk = null
            val what = if (items.size == 1 && ok == 1) items[0].name else if (ok == 1) "1 item" else "$ok items"
            val message = buildString {
                if (ok > 0) append("$done $what")
                val skipped = items.size - ok - failures.size
                if (skipped > 0) append(if (isEmpty()) "Stopped" else ", stopped before $skipped")
                if (failures.isNotEmpty()) {
                    if (isNotEmpty()) append(". ")
                    append(if (failures.size == 1) failures[0] else "${failures.size} failed (${failures[0]})")
                }
            }
            report(message)
        }
    }

    /** Downloads the selected files and hands them to the share sheet together. */
    fun shareAll(items: List<Entry>) {
        val files = items.filter { !it.dir }
        if (files.isEmpty()) return
        val job = BulkProgress("Preparing to share", files.size)
        bulk = job
        scope.launch {
            val fetched = mutableListOf<java.io.File>()
            try {
                for (item in files) {
                    if (job.stopped) break
                    job.current = item.name
                    val tracker = Tracker {}
                    job.tracker = tracker
                    fetched += vm.download(serverId, item, tracker)
                    job.done++
                }
                if (!job.stopped && fetched.isNotEmpty()) {
                    selected = emptySet()
                    shareManyWithAnotherApp(context, fetched)
                }
            } catch (err: Exception) {
                if (!err.isCancel()) report(err.userMessage())
            } finally {
                bulk = null
            }
        }
    }

    /** One step with a spinner; `op` says what to report when it is done. */
    fun runOnce(title: String, op: suspend () -> String) {
        val job = BulkProgress(title, 1)
        bulk = job
        selected = emptySet()
        scope.launch {
            try {
                report(op())
            } catch (err: Exception) {
                report(err.userMessage())
            } finally {
                bulk = null
            }
        }
    }

    fun paste(items: Clip) {
        if (items.cut) vm.clip(null)
        if (items.cut) {
            runBulk("Moving here", "Moved here:", items.entries) { vm.moveTo(serverId, it, path) }
        } else {
            runBulk("Copying here", "Copied here:", items.entries) { vm.copyTo(serverId, it, path) }
        }
    }

    /** Converts videos one at a time, following each conversion on the server. */
    fun convertVideos(items: List<Entry>, choice: VideoChoice) {
        val job = BulkProgress("Converting video", items.size)
        bulk = job
        selected = emptySet()
        scope.launch {
            var made = 0
            val failures = mutableListOf<String>()
            for (item in items) {
                if (job.stopped) break
                job.current = item.name
                job.fraction = 0f
                try {
                    var running = vm.convertVideo(serverId, item, choice)
                    job.onStop = { scope.launch { vm.cancelVideoJob(serverId, running.id) } }
                    while (!running.done && !job.stopped) {
                        delay(1000)
                        running = vm.videoJob(serverId, running.id)
                        job.fraction = running.progress.toFloat().coerceIn(0f, 1f)
                    }
                    when {
                        job.stopped -> {}
                        running.error != null -> failures += "${item.name}: ${running.error}"
                        else -> made++
                    }
                } catch (err: Exception) {
                    failures += "${item.name}: ${err.userMessage()}"
                }
                job.done++
            }
            bulk = null
            reload++
            report(
                listOfNotNull(
                    if (made > 0) (if (made == 1) "The video is ready" else "$made videos are ready") else null,
                    if (job.stopped) "Stopped" else null,
                    failures.firstOrNull()?.let { if (failures.size == 1) it else "${failures.size} failed ($it)" },
                ).joinToString(". "),
            )
        }
    }

    fun showUsage(target: String) {
        dialog = Dialog.Usage(target, null)
        scope.launch {
            val shown = runCatching { vm.folderUsage(serverId, target) }
            if ((dialog as? Dialog.Usage)?.path == target) {
                dialog = Dialog.Usage(target, shown.getOrNull(), shown.exceptionOrNull()?.userMessage())
            }
        }
    }

    fun shareZip() {
        val name = specialFolderLabel(path.substringAfterLast('/')).ifEmpty { server.name }
        val job = BulkProgress("Making $name.zip", 1)
        val tracker = Tracker { done -> job.current = android.text.format.Formatter.formatShortFileSize(context, done) }
        job.tracker = tracker
        bulk = job
        scope.launch {
            try {
                val file = vm.downloadZip(serverId, path, name, tracker)
                if (!job.stopped) shareWithAnotherApp(context, file)
            } catch (err: Exception) {
                if (!err.isCancel()) report(err.userMessage())
            } finally {
                bulk = null
            }
        }
    }

    fun onBulk(action: Bulk, items: List<Entry>) {
        when (action) {
            Bulk.ConvertVideo -> dialog = Dialog.Video(items)
            Bulk.FolderSize -> items.singleOrNull()?.let { selected = emptySet(); showUsage(it.path) }
            Bulk.Cut, Bulk.Copy -> {
                vm.clip(Clip(serverId, items, cut = action == Bulk.Cut))
                selected = emptySet()
                report("${describe(items)}: open a folder and tap Paste")
            }
            Bulk.Protect -> if (items.any { it.dir }) {
                dialog = Dialog.Protect(items)
            } else {
                runOnce("Protecting ${describe(items)}") {
                    vm.setProtection(serverId, path, items, protect = true, contents = false)
                    "${describe(items)} protected from deletion"
                }
            }
            Bulk.Unprotect -> runOnce("Allowing deletion") {
                vm.setProtection(serverId, path, items, protect = false, contents = false)
                "${describe(items)} can be deleted again"
            }
            Bulk.Versions -> items.singleOrNull()?.let { selected = emptySet(); dialog = Dialog.Versions(it) }
            Bulk.ConvertImage -> dialog = Dialog.Convert(items)
            Bulk.Resize -> dialog = Dialog.Resize(items)
            Bulk.JoinPdfs -> runOnce("Joining ${items.size} PDFs") {
                "Made ${vm.pdfMerge(serverId, items).substringAfterLast('/')}"
            }
            Bulk.PdfExtract -> items.singleOrNull()?.let { selected = emptySet(); dialog = Dialog.PdfPages(it) }
            Bulk.PdfSplit -> items.singleOrNull()?.let { entry ->
                runOnce("Splitting ${entry.name}") { "Made ${vm.pdfSplit(serverId, entry).size} PDFs" }
            }
            Bulk.PdfRotate -> items.singleOrNull()?.let { selected = emptySet(); dialog = Dialog.PdfRotate(it) }
            Bulk.AddToAlbum -> dialog = Dialog.Album(items)
            Bulk.ExtractHere -> runBulk("Extracting", "Extracted", items) { vm.extractHere(serverId, it) }
            Bulk.Share -> if (items.size == 1 && !items[0].dir) fetch(items[0], share = true) else shareAll(items)
            Bulk.MoveTo -> dialog = Dialog.Pick(items, copy = false)
            Bulk.CopyTo -> dialog = Dialog.Pick(items, copy = true)
            Bulk.Rename -> items.singleOrNull()?.let { selected = emptySet(); dialog = Dialog.Rename(it) }
            Bulk.Duplicate -> runBulk("Duplicating", "Duplicated", items) { vm.duplicate(serverId, it) }
            Bulk.Tag -> dialog = Dialog.Tag(items)
            Bulk.Compress -> dialog = Dialog.Compress(items)
            Bulk.Extract -> runBulk("Extracting", "Extracted", items) { vm.extract(serverId, it) }
            Bulk.NewFolderWith -> dialog = Dialog.NewFolderWith(items)
            Bulk.Restore -> runBulk("Restoring", "Restored", items) { vm.restore(serverId, it) }
            Bulk.Trash -> dialog = Dialog.Trash(items)
        }
    }

    Scaffold(
        topBar = {
            if (selecting) {
                TopAppBar(
                    title = { Text("${selected.size} selected") },
                    navigationIcon = {
                        IconButton(onClick = { selected = emptySet() }) { Icon(Icons.Filled.Close, contentDescription = "Clear selection") }
                    },
                    actions = {
                        val all = folder?.entries.orEmpty()
                        if (selected.size < all.size) {
                            IconButton(onClick = { selected = all.mapTo(HashSet()) { it.path } }) {
                                Icon(Icons.Outlined.SelectAll, contentDescription = "Select all")
                            }
                        }
                    },
                )
                return@Scaffold
            }
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
                    IconButton(onClick = { vm.push(Screen.Search(serverId, path)) }) {
                        Icon(Icons.Filled.Search, contentDescription = "Search")
                    }
                    IconButton(onClick = { vm.push(Screen.Gallery(serverId)) }) {
                        Icon(Icons.Outlined.PhotoLibrary, contentDescription = "Gallery")
                    }
                    // Shown to administrators when terminals are turned on, as in the web app.
                    if (terminalsOn) {
                        IconButton(onClick = { vm.push(Screen.Terminals(serverId, path)) }) {
                            BadgedBox(badge = {
                                if (attention > 0) Badge { Text(if (attention > 9) "9+" else attention.toString()) }
                            }) {
                                Icon(
                                    Icons.Outlined.Terminal,
                                    contentDescription = if (attention > 0) "Terminals, $attention need attention" else "Terminals",
                                )
                            }
                        }
                    }
                    var menu by remember { mutableStateOf(false) }
                    Box {
                        IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "Folder actions") }
                        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                            if (writable && !inTrash(path)) {
                                DropdownMenuItem(
                                    text = { Text("New file…") },
                                    leadingIcon = { Icon(Icons.AutoMirrored.Outlined.NoteAdd, contentDescription = null) },
                                    onClick = { menu = false; dialog = Dialog.NewFileKind },
                                )
                            }
                            DropdownMenuItem(
                                text = { Text("Folder size") },
                                leadingIcon = { Icon(Icons.Outlined.DataUsage, contentDescription = null) },
                                onClick = { menu = false; showUsage(path) },
                            )
                            DropdownMenuItem(
                                text = { Text("Download as ZIP") },
                                leadingIcon = { Icon(Icons.Outlined.FolderZip, contentDescription = null) },
                                onClick = { menu = false; shareZip() },
                            )
                            if (writable && bookmarked != null) {
                                val marked = bookmarked == true
                                DropdownMenuItem(
                                    text = { Text(if (marked) "Remove bookmark" else "Bookmark this folder") },
                                    leadingIcon = { Icon(if (marked) Icons.Outlined.Bookmark else Icons.Outlined.BookmarkBorder, contentDescription = null) },
                                    onClick = {
                                        menu = false
                                        act {
                                            vm.setBookmark(serverId, path, !marked)
                                            bookmarked = !marked
                                            if (marked) "Bookmark removed" else "Bookmarked"
                                        }
                                    },
                                )
                            }
                            DropdownMenuItem(
                                text = { Text("Bookmarks…") },
                                leadingIcon = { Icon(Icons.Outlined.Bookmarks, contentDescription = null) },
                                onClick = {
                                    menu = false
                                    dialog = Dialog.Bookmarks(null)
                                    scope.launch {
                                        val paths = runCatching { vm.bookmarks(serverId) }.getOrElse { emptyList() }
                                        if (dialog is Dialog.Bookmarks) dialog = Dialog.Bookmarks(paths)
                                    }
                                },
                            )
                            if (terminalsOn && !inTrash(path)) {
                                DropdownMenuItem(
                                    text = { Text("Open terminal here") },
                                    leadingIcon = { Icon(Icons.Outlined.Terminal, contentDescription = null) },
                                    onClick = { menu = false; openTerminalIn(path) },
                                )
                                agents.forEach { agent ->
                                    DropdownMenuItem(
                                        text = { Text("Open ${agent.label} here") },
                                        leadingIcon = { Icon(Icons.Outlined.SmartToy, contentDescription = null) },
                                        onClick = { menu = false; openTerminalIn(path, agent) },
                                    )
                                }
                            }
                            DropdownMenuItem(
                                text = { Text("Find duplicates…") },
                                leadingIcon = { Icon(Icons.Outlined.FileCopy, contentDescription = null) },
                                onClick = { menu = false; vm.push(Screen.Duplicates(serverId, path)) },
                            )
                            if (writable && !inTrash(path)) {
                                DropdownMenuItem(
                                    text = { Text("Sort into folders…") },
                                    leadingIcon = { Icon(Icons.AutoMirrored.Outlined.DriveFileMove, contentDescription = null) },
                                    onClick = { menu = false; vm.push(Screen.SortFolder(serverId, path)) },
                                )
                            }
                            DropdownMenuItem(
                                text = { Text("Activity") },
                                leadingIcon = { Icon(Icons.Outlined.History, contentDescription = null) },
                                onClick = { menu = false; vm.push(Screen.Activity(serverId)) },
                            )
                            DropdownMenuItem(
                                text = { Text(if (showHidden) "Hide hidden files" else "Show hidden files") },
                                leadingIcon = { Icon(if (showHidden) Icons.Outlined.VisibilityOff else Icons.Outlined.Visibility, contentDescription = null) },
                                onClick = { menu = false; vm.setShowHidden(!showHidden) },
                            )
                            if (writable && (path == ".ownnas-trash" || path == "Trash")) {
                                DropdownMenuItem(
                                    text = { Text("Empty Trash") },
                                    leadingIcon = { Icon(Icons.Outlined.DeleteForever, contentDescription = null) },
                                    onClick = { menu = false; dialog = Dialog.EmptyTrash },
                                )
                            }
                            DropdownMenuItem(
                                text = { Text("Sync with this phone…") },
                                leadingIcon = { Icon(Icons.Outlined.Sync, contentDescription = null) },
                                onClick = { menu = false; setUpSync() },
                            )
                        }
                    }
                },
            )
        },
        floatingActionButton = {
            if (writable && folder != null && !selecting) {
                FloatingActionButton(onClick = { adding = true }) { Icon(Icons.Filled.Add, contentDescription = "Add") }
            }
        },
        bottomBar = {
            Column {
                uploads?.takeIf { it.active }?.let { UploadBar(it, onCancel = vm::cancelUploads) }
                if (selecting) SelectionActions(bulkActions(selectedEntries, writable), selectedEntries) { onBulk(it, selectedEntries) }
                val waiting = clip
                if (!selecting && writable && waiting != null && waiting.serverId == serverId) {
                    PasteBar(waiting, onCancel = { vm.clip(null) }, onPaste = { paste(waiting) })
                }
            }
        },
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
                    terminalsOn = terminalsOn,
                    terminalsByPath = terminalsByPath,
                    selected = selected,
                    onOpen = { entry ->
                        when {
                            !selecting -> open(entry)
                            entry.path in selected -> selected = selected - entry.path
                            else -> selected = selected + entry.path
                        }
                    },
                    onSelect = { entry -> selected = if (entry.path in selected) selected - entry.path else selected + entry.path },
                    onAction = { action, entry -> onBulk(action, listOf(entry)) },
                    onTerminal = { entry -> openTerminalIn(entry.path) },
                    agents = agents,
                    onAgent = { entry, agent -> openTerminalIn(entry.path, agent) },
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
            onScan = {
                adding = false
                openScanner()
            },
            onFolder = {
                adding = false
                dialog = Dialog.NewFolder
            },
        )
    }

    scan?.let { shown ->
        ScanDialog(
            scan = shown,
            onDismiss = {
                scan = null
                shown.discard()
            },
            onUpload = { name, pdf ->
                scan = null
                uploadScan(shown, name, pdf)
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
            entries = shown.entries,
            onDismiss = { dialog = null },
            onConfirm = {
                dialog = null
                val forever = shown.entries.all { inTrash(it.path) }
                runBulk(if (forever) "Deleting" else "Moving to the Trash", if (forever) "Deleted" else "Moved to the Trash:", shown.entries) {
                    vm.trash(serverId, it)
                }
            },
        )
        is Dialog.Tag -> TagDialog(
            count = shown.entries.size,
            onDismiss = { dialog = null },
            onConfirm = { tag ->
                dialog = null
                runBulk("Tagging #$tag", "Tagged #$tag:", shown.entries) { vm.addTag(serverId, it, tag) }
            },
        )
        is Dialog.NewFolderWith -> NameDialog(
            title = "New folder with ${describe(shown.entries)}",
            initial = "",
            confirm = "Create",
            onDismiss = { dialog = null },
            onConfirm = { name ->
                dialog = null
                scope.launch {
                    val made = try {
                        vm.createFolder(serverId, path, name)
                    } catch (err: Exception) {
                        report(err.userMessage())
                        return@launch
                    }
                    runBulk("Moving into $name", "Moved into $name:", shown.entries) { vm.moveTo(serverId, it, made) }
                }
            },
        )
        is Dialog.Compress -> ChoiceDialog(
            title = "Compress ${describe(shown.entries)} to",
            choices = listOf("zip" to "ZIP", "tar.gz" to "TAR.GZ", "tar.xz" to "TAR.XZ", "tar" to "TAR"),
            onDismiss = { dialog = null },
            onPick = { format ->
                dialog = null
                runOnce("Compressing ${describe(shown.entries)}") {
                    "Made ${vm.compress(serverId, path, shown.entries, format).substringAfterLast('/')}"
                }
            },
        )
        is Dialog.Convert -> ChoiceDialog(
            title = "Convert ${describe(shown.entries)} to",
            choices = listOf("jpeg" to "JPEG", "png" to "PNG", "webp" to "WebP", "gif" to "GIF", "bmp" to "BMP", "tiff" to "TIFF"),
            onDismiss = { dialog = null },
            onPick = { format ->
                dialog = null
                runBulk("Converting to ${format.uppercase()}", "Converted", shown.entries) { vm.convertImage(serverId, it, format) }
            },
        )
        is Dialog.Resize -> ResizeDialog(
            count = shown.entries.size,
            onDismiss = { dialog = null },
            onResize = { choice ->
                dialog = null
                runBulk("Resizing", if (choice.replace) "Resized" else "Made smaller copies of", shown.entries) { vm.resizeImage(serverId, it, choice) }
            },
        )
        is Dialog.Protect -> ChoiceDialog(
            title = "Protect from deletion",
            choices = listOf("self" to "Only the folder itself", "contents" to "The folder and everything in it"),
            onDismiss = { dialog = null },
            onPick = { how ->
                dialog = null
                runOnce("Protecting ${describe(shown.entries)}") {
                    vm.setProtection(serverId, path, shown.entries, protect = true, contents = how == "contents")
                    "${describe(shown.entries)} protected from deletion"
                }
            },
        )
        is Dialog.PdfPages -> PdfPagesDialog(
            name = shown.entry.name,
            onDismiss = { dialog = null },
            onExtract = { pages, separate ->
                dialog = null
                runOnce("Extracting pages") {
                    val made = vm.pdfExtract(serverId, shown.entry, pages, separate)
                    if (made.size == 1) "Made ${made[0].substringAfterLast('/')}" else "Made ${made.size} PDFs"
                }
            },
        )
        is Dialog.PdfRotate -> PdfRotateDialog(
            name = shown.entry.name,
            onDismiss = { dialog = null },
            onRotate = { pages, degrees ->
                dialog = null
                runOnce("Rotating pages") { "Made ${vm.pdfRotate(serverId, shown.entry, pages, degrees).substringAfterLast('/')}" }
            },
        )
        is Dialog.Versions -> VersionsDialog(
            vm = vm,
            serverId = serverId,
            entry = shown.entry,
            writable = writable,
            onDismiss = { dialog = null },
            onDone = { message ->
                dialog = null
                report(message)
                reload++
            },
        )
        is Dialog.Album -> app.ownnas.android.ui.gallery.AlbumPicker(vm, serverId, "Add ${describe(shown.entries)} to…", onDismiss = { dialog = null }) { album ->
            dialog = null
            runOnce("Adding to the album") {
                vm.call(serverId) { server -> server.changeAlbum(album, shown.entries.map { it.path }, true) }
                "Added ${describe(shown.entries)} to the album"
            }
        }
        Dialog.EmptyTrash -> AlertDialog(
            onDismissRequest = { dialog = null },
            icon = { Icon(Icons.Outlined.DeleteForever, contentDescription = null) },
            title = { Text("Empty the Trash?") },
            text = { Text("Everything in it is deleted for good. This cannot be undone.") },
            confirmButton = {
                TextButton(onClick = {
                    dialog = null
                    runOnce("Emptying the Trash") {
                        vm.emptyTrash(serverId, path)
                        "The Trash is empty"
                    }
                }) { Text("Empty Trash") }
            },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text("Cancel") } },
        )
        is Dialog.Video -> VideoDialog(
            count = shown.entries.size,
            onDismiss = { dialog = null },
            onConvert = { choice ->
                dialog = null
                convertVideos(shown.entries, choice)
            },
        )
        Dialog.NewFileKind -> ChoiceDialog(
            title = "New file",
            choices = newFileKinds,
            onDismiss = { dialog = null },
            onPick = { kind -> dialog = Dialog.NewFileName(kind, newFileKinds.first { it.first == kind }.second) },
        )
        is Dialog.NewFileName -> NameDialog(
            title = "New ${shown.label.lowercase()}",
            initial = "",
            confirm = "Create",
            onDismiss = { dialog = null },
            onConfirm = { name ->
                dialog = null
                act { "Made ${vm.createFile(serverId, path, name, shown.kind).substringAfterLast('/')}" }
            },
        )
        is Dialog.Usage -> AlertDialog(
            onDismissRequest = { dialog = null },
            icon = { Icon(Icons.Outlined.DataUsage, contentDescription = null) },
            title = { Text(specialFolderLabel(shown.path.substringAfterLast('/')).ifEmpty { server.name }) },
            text = {
                val usage = shown.usage
                when {
                    shown.problem != null -> Text(shown.problem)
                    usage == null -> Row(verticalAlignment = Alignment.CenterVertically) {
                        CircularProgressIndicator(Modifier.size(20.dp))
                        Text("Counting…", Modifier.padding(start = 12.dp))
                    }
                    else -> Text(
                        (if (usage.truncated) "At least " else "") +
                            "${android.text.format.Formatter.formatFileSize(context, usage.bytes.toLong())} in " +
                            "%,d".format(usage.files.toLong()) + if (usage.files == 1UL) " file" else " files",
                    )
                }
            },
            confirmButton = { TextButton(onClick = { dialog = null }) { Text("OK") } },
        )
        is Dialog.Bookmarks -> BookmarksDialog(
            serverName = server.name,
            paths = shown.paths,
            onDismiss = { dialog = null },
            onOpen = { target ->
                dialog = null
                if (target != path) vm.push(Screen.Browse(serverId, target))
            },
        )
        is Dialog.Pick -> FolderPicker(
            vm = vm,
            serverId = serverId,
            serverName = server.name,
            start = path,
            title = (if (shown.copy) "Copy " else "Move ") + describe(shown.entries),
            confirm = if (shown.copy) "Copy here" else "Move here",
            moving = shown.entries,
            unavailable = if (shown.copy) null else path,
            onDismiss = { dialog = null },
            onPick = { dest ->
                dialog = null
                val where = specialFolderLabel(dest.substringAfterLast('/')).ifEmpty { server.name }
                if (shown.copy) {
                    runBulk("Copying to $where", "Copied to $where:", shown.entries) { vm.copyTo(serverId, it, dest) }
                } else {
                    runBulk("Moving to $where", "Moved to $where:", shown.entries) { vm.moveTo(serverId, it, dest) }
                }
            },
        )
        null -> {}
    }

    bulk?.let { BulkProgressDialog(it) }

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
    terminalsOn: Boolean,
    terminalsByPath: Map<String, List<TerminalInfo>>,
    selected: Set<String>,
    onOpen: (Entry) -> Unit,
    onSelect: (Entry) -> Unit,
    onAction: (Bulk, Entry) -> Unit,
    onTerminal: (Entry) -> Unit,
    agents: List<app.ownnas.core.AgentLauncher>,
    onAgent: (Entry, app.ownnas.core.AgentLauncher) -> Unit,
) {
    val context = LocalContext.current
    val selecting = selected.isNotEmpty()
    val entries = remember(folder) {
        folder.entries.sortedWith(compareBy<Entry> { !it.dir }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name })
    }
    // Room under the last row for the add button.
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 88.dp)) {
        items(entries, key = { it.path }) { entry ->
            var menu by remember { mutableStateOf(false) }
            val here = if (entry.dir) terminalsByPath[entry.path].orEmpty() else emptyList()
            val chosen = entry.path in selected
            ListItem(
                modifier = Modifier.combinedClickable(onClick = { onOpen(entry) }, onLongClick = { onSelect(entry) }),
                colors = if (chosen) ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.secondaryContainer) else ListItemDefaults.colors(),
                leadingContent = {
                    Box {
                        Thumbnail(vm, serverId, entry)
                        if (chosen) {
                            Icon(
                                Icons.Filled.CheckCircle,
                                contentDescription = "Selected",
                                tint = MaterialTheme.colorScheme.primary,
                                modifier = Modifier.align(Alignment.TopStart).offset(x = (-6).dp, y = (-6).dp).size(22.dp)
                                    .background(MaterialTheme.colorScheme.surface, CircleShape),
                            )
                        }
                        if (here.isNotEmpty()) TerminalMark(here, Modifier.align(Alignment.BottomEnd).offset(x = 4.dp, y = 4.dp))
                    }
                },
                headlineContent = { Text(specialFolderLabel(entry.name), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                supportingContent = {
                    val running = here.filter { it.running }
                    val status = running.firstNotNullOfOrNull { info -> agentStatus(info)?.first }
                    val terminalNote = when {
                        status != null -> " · $status"
                        running.size == 1 -> " · terminal open"
                        running.size > 1 -> " · ${running.size} terminals open"
                        here.isNotEmpty() -> " · terminal ${endedText(here.first()).lowercase()}"
                        else -> ""
                    }
                    Text(details(context, entry) + terminalNote, maxLines = 1, overflow = TextOverflow.Ellipsis)
                },
                trailingContent = if (selecting) {
                    null
                } else {
                    {
                        Box {
                            IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "Actions for ${entry.name}") }
                            EntryMenu(
                                entry = entry,
                                actions = bulkActions(listOf(entry), writable),
                                terminals = terminalsOn,
                                expanded = menu,
                                onDismiss = { menu = false },
                                onSelect = { onSelect(entry) },
                                onTerminal = { onTerminal(entry) },
                                onAction = { onAction(it, entry) },
                                agents = agents,
                                onAgent = { onAgent(entry, it) },
                            )
                        }
                    }
                },
            )
        }
        if (folder.truncated) {
            item { Message("Showing the first ${folder.entries.size} items. Open this folder in the browser to see the rest.") }
        }
    }
}

/**
 * A small terminal badge on a folder with open terminals. Amber when an agent wants attention,
 * otherwise the agent's status color, or neutral for a plain shell.
 */
@Composable
private fun TerminalMark(terminals: List<TerminalInfo>, modifier: Modifier) {
    val running = terminals.filter { it.running }
    // Only ended terminals here: show that they stopped, and how.
    if (running.isEmpty()) {
        EndedBadge(terminals.firstOrNull { it.exitCode != null && it.exitCode != 0u } ?: terminals.first(), modifier.size(20.dp))
        return
    }
    val color = when {
        running.any { it.agent.attention != null } -> Color(0xFFF59E0B)
        else -> running.firstNotNullOfOrNull { agentStatus(it)?.second } ?: MaterialTheme.colorScheme.secondary
    }
    Surface(
        shape = CircleShape,
        color = color,
        border = BorderStroke(2.dp, MaterialTheme.colorScheme.surface),
        modifier = modifier.size(20.dp),
    ) {
        Icon(Icons.Outlined.Terminal, contentDescription = "Terminal open", tint = Color.White, modifier = Modifier.padding(3.dp))
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

/**
 * Search in `path` and below: names, tags, and the words inside files (as the web app's search
 * box does). Folders open in the browser; files open in another app.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SearchScreen(vm: AppViewModel, serverId: String, path: String) {
    val server = vm.server(serverId) ?: return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var words by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf("") }
    var searched by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf("") }
    var hits by remember { mutableStateOf<List<app.ownnas.core.SearchHit>?>(null) }
    var searching by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<String?>(null) }
    var fetching by remember { mutableStateOf<Fetch?>(null) }
    val focus = remember { androidx.compose.ui.focus.FocusRequester() }
    LaunchedEffect(Unit) { if (searched.isEmpty()) focus.requestFocus() }
    LaunchedEffect(searched) {
        if (searched.isBlank()) return@LaunchedEffect
        searching = true
        problem = null
        try {
            hits = vm.call(serverId) { it.search(path, searched) }
        } catch (err: Exception) {
            hits = null
            problem = err.userMessage()
        } finally {
            searching = false
        }
    }
    fun open(hit: app.ownnas.core.SearchHit) {
        if (hit.dir) {
            vm.push(Screen.Browse(serverId, hit.path))
            return
        }
        val entry = hit.asEntry()
        val job = Fetch(entry, share = false)
        fetching = job
        scope.launch {
            try {
                val file = vm.download(serverId, entry, job.tracker)
                if (fetching === job) openWithAnotherApp(context, file)
            } catch (err: ActivityNotFoundException) {
                problem = "No app on this phone can open ${entry.name}"
            } catch (err: Exception) {
                if (!err.isCancel()) problem = err.userMessage()
            } finally {
                if (fetching === job) fetching = null
            }
        }
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    androidx.compose.material3.TextField(
                        value = words,
                        onValueChange = { words = it },
                        placeholder = { Text("Search ${specialFolderLabel(path.substringAfterLast('/')).ifEmpty { server.name }}") },
                        singleLine = true,
                        colors = androidx.compose.material3.TextFieldDefaults.colors(focusedContainerColor = Color.Transparent, unfocusedContainerColor = Color.Transparent),
                        keyboardActions = androidx.compose.foundation.text.KeyboardActions(onSearch = { searched = words.trim() }),
                        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(imeAction = androidx.compose.ui.text.input.ImeAction.Search),
                        modifier = Modifier.fillMaxWidth().focusRequester(focus),
                    )
                },
                navigationIcon = { IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") } },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            val found = hits
            when {
                searching -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
                problem != null && found == null -> Message(problem!!)
                found == null -> Message("Names, tags, and the words inside documents, PDFs, and pictures. Type tag:name for one tag.")
                found.isEmpty() -> Message("Nothing matches “$searched”.")
                else -> LazyColumn(Modifier.fillMaxSize()) {
                    items(found, key = { it.path }) { hit ->
                        val entry = hit.asEntry()
                        ListItem(
                            modifier = Modifier.clickable { open(hit) },
                            leadingContent = { Thumbnail(vm, serverId, entry) },
                            headlineContent = { Text(specialFolderLabel(hit.name), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            supportingContent = {
                                Column {
                                    Text(
                                        hit.path.substringBeforeLast('/', "").split('/').joinToString("/", transform = ::specialFolderLabel).ifEmpty { server.name },
                                        maxLines = 1,
                                        overflow = TextOverflow.Ellipsis,
                                    )
                                    val why = hit.matchedContent?.let { "…${it.trim()}…" } ?: hit.matchedTag?.let { "Tagged $it" }
                                    if (why != null) Text(why, maxLines = 2, overflow = TextOverflow.Ellipsis, color = MaterialTheme.colorScheme.primary)
                                }
                            },
                        )
                    }
                }
            }
            if (problem != null && found != null) {
                Text(problem!!, color = MaterialTheme.colorScheme.error, modifier = Modifier.align(Alignment.BottomCenter).padding(16.dp))
            }
        }
    }
    fetching?.let { job ->
        FetchDialog(name = job.entry.name, done = job.done, size = job.entry.size.toLong(), sharing = false, onCancel = {
            job.tracker.cancelled = true
            fetching = null
        })
    }
}

/** A search hit as a folder entry, to show and open it like one. */
private fun app.ownnas.core.SearchHit.asEntry() = Entry(
    name = name,
    path = path,
    dir = dir,
    size = size,
    modified = modified * 1000,
    kind = kind,
    thumb = kind == "image" || kind == "video" || kind == "pdf",
)

/** Shown in folders while cut or copied items wait to be pasted. */
@Composable
private fun PasteBar(clip: Clip, onCancel: () -> Unit, onPaste: () -> Unit) {
    Surface(tonalElevation = 3.dp) {
        Row(
            Modifier.fillMaxWidth().navigationBarsPadding().padding(start = 16.dp, end = 8.dp, top = 8.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Outlined.ContentPaste, contentDescription = null)
            Text(
                (if (clip.cut) "Move " else "Copy ") + describe(clip.entries),
                modifier = Modifier.weight(1f).padding(horizontal = 12.dp),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            TextButton(onClick = onCancel) { Text("Cancel") }
            Button(onClick = onPaste) { Text("Paste here") }
        }
    }
}
