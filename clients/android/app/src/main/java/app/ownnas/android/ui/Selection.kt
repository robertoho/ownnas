package app.ownnas.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.DriveFileMove
import androidx.compose.material.icons.automirrored.outlined.Label
import androidx.compose.material.icons.automirrored.outlined.CallSplit
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.PhotoAlbum
import androidx.compose.material.icons.outlined.RotateRight
import androidx.compose.material.icons.outlined.PictureAsPdf
import androidx.compose.material.icons.outlined.MergeType
import androidx.compose.material.icons.outlined.PhotoSizeSelectLarge
import androidx.compose.material.icons.outlined.Transform
import androidx.compose.material.icons.outlined.History
import androidx.compose.material.icons.outlined.LockOpen
import androidx.compose.material.icons.outlined.Lock
import androidx.compose.material.icons.outlined.ControlPointDuplicate
import androidx.compose.material.icons.outlined.ContentCut
import androidx.compose.material.icons.outlined.CreateNewFolder
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.DriveFileRenameOutline
import androidx.compose.material.icons.outlined.FileCopy
import androidx.compose.material.icons.outlined.FolderZip
import androidx.compose.material.icons.outlined.RestoreFromTrash
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.Unarchive
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.BottomAppBar
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import app.ownnas.core.Entry
import app.ownnas.core.Folder
import kotlinx.coroutines.launch

/** What can be done to one or more selected files and folders, as in the web app's menu. */
enum class Bulk(val label: String, val icon: ImageVector) {
    Share("Share", Icons.Outlined.Share),
    MoveTo("Move to…", Icons.AutoMirrored.Outlined.DriveFileMove),
    CopyTo("Copy to…", Icons.Outlined.FileCopy),
    Cut("Cut", Icons.Outlined.ContentCut),
    Copy("Copy", Icons.Outlined.ContentCopy),
    Rename("Rename", Icons.Outlined.DriveFileRenameOutline),
    Duplicate("Duplicate", Icons.Outlined.ControlPointDuplicate),
    Tag("Tag…", Icons.AutoMirrored.Outlined.Label),
    Protect("Protect from deletion", Icons.Outlined.Lock),
    Unprotect("Allow deletion", Icons.Outlined.LockOpen),
    Versions("Versions…", Icons.Outlined.History),
    ConvertImage("Convert to…", Icons.Outlined.Transform),
    Resize("Resize…", Icons.Outlined.PhotoSizeSelectLarge),
    JoinPdfs("Join PDFs", Icons.Outlined.MergeType),
    PdfExtract("Extract pages…", Icons.Outlined.PictureAsPdf),
    PdfSplit("Split into pages", Icons.AutoMirrored.Outlined.CallSplit),
    PdfRotate("Rotate pages…", Icons.Outlined.RotateRight),
    AddToAlbum("Add to album…", Icons.Outlined.PhotoAlbum),
    Compress("Compress to…", Icons.Outlined.FolderZip),
    Extract("Extract", Icons.Outlined.Unarchive),
    ExtractHere("Extract here", Icons.Outlined.Unarchive),
    NewFolderWith("New folder with selection", Icons.Outlined.CreateNewFolder),
    Restore("Restore", Icons.Outlined.RestoreFromTrash),
    Trash("Move to Trash", Icons.Outlined.Delete),
    ;

    fun label(entries: List<Entry>) = when {
        this == Trash && entries.any { inTrash(it.path) } -> "Delete permanently"
        this == NewFolderWith && entries.size == 1 -> "New folder with this"
        else -> label
    }
}

/** OwnNAS's own Trash folder, which cannot be moved, copied, or renamed. */
fun Entry.isTrashFolder() = kind == "trash-folder" || path == ".ownnas-trash" || (path == "Trash" && dir)

/** Photos the server can resize (as the web app's list). */
private val resizable = Regex("""\.(jpe?g|png|webp|bmp|tiff?)$""", RegexOption.IGNORE_CASE)

/** The actions that fit `entries`, in menu order. */
fun bulkActions(entries: List<Entry>, writable: Boolean): List<Bulk> {
    if (entries.isEmpty() || entries.any { it.isTrashFolder() }) return emptyList()
    val one = entries.singleOrNull()
    val actions = mutableListOf<Bulk>()
    if (entries.any { !it.dir }) actions += Bulk.Share
    if (!writable) {
        if (one != null && !one.dir) actions += Bulk.Versions
        return actions
    }
    if (entries.all { inTrash(it.path) }) return actions + listOf(Bulk.Restore, Bulk.Trash)
    actions += listOf(Bulk.MoveTo, Bulk.CopyTo, Bulk.Cut, Bulk.Copy)
    if (one != null) actions += Bulk.Rename
    actions += listOf(Bulk.Duplicate, Bulk.Tag)
    actions += if (entries.all { it.protected == "self" || it.protected == "contents" }) Bulk.Unprotect else Bulk.Protect
    if (one != null && !one.dir) actions += Bulk.Versions
    if (entries.all { !it.dir && it.kind == "image" }) {
        actions += Bulk.ConvertImage
        if (entries.all { resizable.containsMatchIn(it.name) }) actions += Bulk.Resize
    }
    if (entries.all { !it.dir && it.kind == "pdf" }) {
        if (entries.size >= 2) actions += Bulk.JoinPdfs
        if (one != null) actions += listOf(Bulk.PdfExtract, Bulk.PdfSplit, Bulk.PdfRotate)
    }
    if (entries.all { !it.dir && (it.kind == "image" || it.kind == "video") }) actions += Bulk.AddToAlbum
    actions += Bulk.Compress
    if (one != null && !one.dir && one.kind == "archive") actions += listOf(Bulk.Extract, Bulk.ExtractHere)
    actions += listOf(Bulk.NewFolderWith, Bulk.Trash)
    return actions
}

/** The bar under a selection: the most used actions, and the rest under More. */
@Composable
fun SelectionActions(actions: List<Bulk>, entries: List<Entry>, onAction: (Bulk) -> Unit) {
    val primary = listOf(Bulk.Share, Bulk.MoveTo, Bulk.CopyTo, Bulk.Restore, Bulk.Trash).filter { it in actions }.take(4)
    val more = actions - primary.toSet()
    BottomAppBar {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly, verticalAlignment = Alignment.CenterVertically) {
            if (actions.isEmpty()) {
                Text("Nothing to do with this selection", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            primary.forEach { action ->
                BarButton(action.label(entries).removeSuffix("…").replace("Move to Trash", "Trash").replace("Delete permanently", "Delete"), action.icon) { onAction(action) }
            }
            if (more.isNotEmpty()) {
                var open by remember { mutableStateOf(false) }
                Box {
                    BarButton("More", Icons.Filled.MoreVert) { open = true }
                    DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                        more.forEach { action ->
                            DropdownMenuItem(
                                text = { Text(action.label(entries)) },
                                leadingIcon = { Icon(action.icon, contentDescription = null) },
                                onClick = { open = false; onAction(action) },
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun BarButton(label: String, icon: ImageVector, onClick: () -> Unit) {
    Column(
        Modifier.clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 6.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(icon, contentDescription = null)
        Text(label, style = MaterialTheme.typography.labelSmall, maxLines = 1)
    }
}

/** An action running over several items, one at a time. */
class BulkProgress(val title: String, val total: Int) {
    var done by mutableIntStateOf(0)
    var current by mutableStateOf("")
    /** Asked to stop after the item in hand. */
    @Volatile var stopped = false
    /** A download in progress, cancelled by Stop. */
    @Volatile var tracker: Tracker? = null

    fun stop() {
        stopped = true
        tracker?.cancelled = true
    }
}

@Composable
fun BulkProgressDialog(progress: BulkProgress) {
    AlertDialog(
        onDismissRequest = {},
        title = { Text(progress.title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(progress.current, maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (progress.total > 1) {
                    LinearProgressIndicator(progress = { progress.done.toFloat() / progress.total }, modifier = Modifier.fillMaxWidth())
                    Text("${progress.done} of ${progress.total}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                } else {
                    LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                }
            }
        },
        confirmButton = {
            if (progress.total > 1 || progress.tracker != null) TextButton(onClick = { progress.stop() }, enabled = !progress.stopped) { Text("Stop") }
        },
    )
}

/** "3 items", or the name when there is one. */
fun describe(entries: List<Entry>) = if (entries.size == 1) entries[0].name else "${entries.size} items"

/**
 * Chooses a folder on the server to move or copy into: browses folders from `start`, can make
 * a new one, and does not offer the moving folders themselves or anything inside them.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FolderPicker(
    vm: AppViewModel,
    serverId: String,
    serverName: String,
    start: String,
    title: String,
    confirm: String,
    moving: List<Entry>,
    /** A folder that cannot be chosen, such as where the items already are for a move. */
    unavailable: String?,
    onDismiss: () -> Unit,
    onPick: (String) -> Unit,
) {
    val scope = rememberCoroutineScope()
    var at by remember { mutableStateOf(start) }
    var folder by remember { mutableStateOf<Folder?>(null) }
    var problem by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    var naming by remember { mutableStateOf(false) }
    val movingDirs = moving.filter { it.dir }.map { it.path }
    fun blocked(path: String) = movingDirs.any { path == it || path.startsWith("$it/") }

    LaunchedEffect(at, reload) {
        folder = null
        problem = null
        try {
            folder = vm.list(serverId, at)
        } catch (err: Exception) {
            problem = err.userMessage()
        }
    }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize()) {
            Scaffold(
                topBar = {
                    TopAppBar(
                        title = {
                            Column {
                                Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Text(
                                    if (at.isEmpty()) serverName else "$serverName/" + at.split('/').joinToString("/", transform = ::specialFolderLabel),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                        },
                        navigationIcon = {
                            if (at.isEmpty()) {
                                IconButton(onClick = onDismiss) { Icon(Icons.Filled.Close, contentDescription = "Cancel") }
                            } else {
                                IconButton(onClick = { at = at.parentPath() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Up") }
                            }
                        },
                        actions = {
                            IconButton(onClick = { naming = true }) { Icon(Icons.Outlined.CreateNewFolder, contentDescription = "New folder") }
                        },
                    )
                },
                bottomBar = {
                    Column(Modifier.navigationBarsPadding()) {
                        HorizontalDivider()
                        Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.End)) {
                            TextButton(onClick = onDismiss) { Text("Cancel") }
                            Button(onClick = { onPick(at) }, enabled = folder != null && at != unavailable && !blocked(at)) { Text(confirm) }
                        }
                    }
                },
            ) { padding ->
                val shown = folder
                Box(Modifier.padding(padding).fillMaxSize()) {
                    when {
                        problem != null -> Text(problem!!, textAlign = TextAlign.Center, modifier = Modifier.align(Alignment.Center).padding(32.dp))
                        shown == null -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                        else -> {
                            val dirs = shown.entries.filter { it.dir && !it.isTrashFolder() }
                                .sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.name })
                            if (dirs.isEmpty()) {
                                Text(
                                    "No folders here",
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    modifier = Modifier.align(Alignment.Center),
                                )
                            }
                            LazyColumn(Modifier.fillMaxSize()) {
                                items(dirs, key = { it.path }) { dir ->
                                    val off = blocked(dir.path)
                                    ListItem(
                                        modifier = if (off) Modifier else Modifier.clickable { at = dir.path },
                                        leadingContent = {
                                            Icon(
                                                Icons.Filled.Folder,
                                                contentDescription = null,
                                                tint = if (off) MaterialTheme.colorScheme.outline else MaterialTheme.colorScheme.primary,
                                                modifier = Modifier.size(32.dp),
                                            )
                                        },
                                        headlineContent = {
                                            Text(
                                                specialFolderLabel(dir.name),
                                                maxLines = 1,
                                                overflow = TextOverflow.Ellipsis,
                                                color = if (off) MaterialTheme.colorScheme.outline else MaterialTheme.colorScheme.onSurface,
                                            )
                                        },
                                        supportingContent = if (off) {
                                            { Text("Being moved") }
                                        } else {
                                            null
                                        },
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if (naming) {
        NameDialog(
            title = "New folder",
            initial = "",
            confirm = "Create",
            onDismiss = { naming = false },
            onConfirm = { name ->
                naming = false
                scope.launch {
                    try {
                        at = vm.createFolder(serverId, at, name)
                    } catch (err: Exception) {
                        problem = err.userMessage()
                    }
                }
            },
        )
    }
}

/** Asks for a tag to add. */
@Composable
fun TagDialog(count: Int, onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var text by remember { mutableStateOf("") }
    val tag = text.trim().removePrefix("#")
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (count == 1) "Tag" else "Tag $count items") },
        text = {
            androidx.compose.material3.OutlinedTextField(
                value = text,
                onValueChange = { text = it },
                singleLine = true,
                label = { Text("Tag") },
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = { TextButton(onClick = { onConfirm(tag) }, enabled = tag.isNotEmpty()) { Text("Add tag") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Picks one of a few choices, such as a format. */
@Composable
fun ChoiceDialog(title: String, choices: List<Pair<String, String>>, onDismiss: () -> Unit, onPick: (String) -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column {
                choices.forEach { (value, label) ->
                    ListItem(
                        headlineContent = { Text(label) },
                        modifier = Modifier.clickable { onPick(value) },
                        colors = androidx.compose.material3.ListItemDefaults.colors(containerColor = androidx.compose.ui.graphics.Color.Transparent),
                    )
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** How much smaller to make photos: a size for the long side, or a share. */
data class ResizeChoice(val percent: Int?, val longest: Int?, val replace: Boolean)

private val resizePresets = listOf(
    "Small · 1280" to ResizeChoice(null, 1280, false),
    "Medium · 1920" to ResizeChoice(null, 1920, false),
    "Large · 2560" to ResizeChoice(null, 2560, false),
    "4K · 3840" to ResizeChoice(null, 3840, false),
    "Half · 50%" to ResizeChoice(50, null, false),
    "Quarter · 25%" to ResizeChoice(25, null, false),
)

@Composable
fun ResizeDialog(count: Int, onDismiss: () -> Unit, onResize: (ResizeChoice) -> Unit) {
    var picked by remember { mutableIntStateOf(1) }
    var replace by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (count == 1) "Resize photo" else "Resize $count photos") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                resizePresets.forEachIndexed { index, (label, _) ->
                    Row(
                        Modifier.fillMaxWidth().clickable { picked = index }.padding(vertical = 2.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        androidx.compose.material3.RadioButton(selected = picked == index, onClick = { picked = index })
                        Text(label)
                    }
                }
                Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("Replace the originals", Modifier.weight(1f))
                    androidx.compose.material3.Switch(checked = replace, onCheckedChange = { replace = it })
                }
                Text(
                    if (replace) "The originals go to the Trash." else "Smaller copies are saved beside them. Photos already that small are left as they are.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
        confirmButton = { TextButton(onClick = { onResize(resizePresets[picked].second.copy(replace = replace)) }) { Text("Resize") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Pages to take out of a PDF ("1,3,5-8"), as one PDF or one per page. */
@Composable
fun PdfPagesDialog(name: String, onDismiss: () -> Unit, onExtract: (pages: String, separate: Boolean) -> Unit) {
    var pages by remember { mutableStateOf("") }
    var separate by remember { mutableStateOf(false) }
    val valid = pages.isNotBlank() && pages.all { it.isDigit() || it == ',' || it == '-' || it == ' ' }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Extract pages") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(name, maxLines = 1, overflow = TextOverflow.Ellipsis, color = MaterialTheme.colorScheme.onSurfaceVariant)
                androidx.compose.material3.OutlinedTextField(
                    value = pages,
                    onValueChange = { pages = it },
                    singleLine = true,
                    label = { Text("Pages, such as 1,3,5-8") },
                    keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Phone),
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text("One PDF per page", Modifier.weight(1f))
                    androidx.compose.material3.Switch(checked = separate, onCheckedChange = { separate = it })
                }
            }
        },
        confirmButton = { TextButton(onClick = { onExtract(pages.replace(" ", ""), separate) }, enabled = valid) { Text("Extract") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Which way to turn a PDF's pages, and which pages (all when left empty). */
@Composable
fun PdfRotateDialog(name: String, onDismiss: () -> Unit, onRotate: (pages: String, degrees: Long) -> Unit) {
    var pages by remember { mutableStateOf("") }
    var degrees by remember { mutableIntStateOf(90) }
    val valid = pages.all { it.isDigit() || it == ',' || it == '-' || it == ' ' }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Rotate pages") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(name, maxLines = 1, overflow = TextOverflow.Ellipsis, color = MaterialTheme.colorScheme.onSurfaceVariant)
                SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                    listOf(90 to "Right", 180 to "Upside down", 270 to "Left").forEachIndexed { index, (value, label) ->
                        SegmentedButton(selected = degrees == value, onClick = { degrees = value }, shape = SegmentedButtonDefaults.itemShape(index, 3)) {
                            Text(label, maxLines = 1)
                        }
                    }
                }
                androidx.compose.material3.OutlinedTextField(
                    value = pages,
                    onValueChange = { pages = it },
                    singleLine = true,
                    label = { Text("Pages (empty for all)") },
                    keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Phone),
                    modifier = Modifier.fillMaxWidth(),
                )
                Text("Saved as a new PDF beside it.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        },
        confirmButton = { TextButton(onClick = { onRotate(pages.replace(" ", ""), degrees.toLong()) }, enabled = valid) { Text("Rotate") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Earlier versions of a file: put one back, or delete it. */
@Composable
fun VersionsDialog(vm: AppViewModel, serverId: String, entry: Entry, writable: Boolean, onDismiss: () -> Unit, onDone: (String) -> Unit) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val scope = rememberCoroutineScope()
    var versions by remember { mutableStateOf<List<app.ownnas.core.FileVersion>?>(null) }
    var problem by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    var deleting by remember { mutableStateOf<app.ownnas.core.FileVersion?>(null) }
    LaunchedEffect(reload) {
        try {
            versions = vm.call(serverId) { it.versions(entry.path) }
        } catch (err: Exception) {
            problem = err.userMessage()
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Versions of ${entry.name}", maxLines = 2, overflow = TextOverflow.Ellipsis) },
        text = {
            val shown = versions
            when {
                problem != null -> Text(problem!!)
                shown == null -> Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
                shown.isEmpty() -> Text("No earlier versions. A file gets one each time it is replaced or edited.")
                else -> LazyColumn(Modifier.fillMaxWidth()) {
                    items(shown, key = { it.id }) { version ->
                        val reason = when (version.reason) {
                            "edit" -> "edited"
                            "upload" -> "replaced by an upload"
                            "sync" -> "replaced by sync"
                            "restore" -> "replaced by a restore"
                            else -> version.reason
                        }
                        ListItem(
                            colors = androidx.compose.material3.ListItemDefaults.colors(containerColor = androidx.compose.ui.graphics.Color.Transparent),
                            headlineContent = {
                                Text(
                                    android.text.format.DateUtils.formatDateTime(
                                        context,
                                        version.modified,
                                        android.text.format.DateUtils.FORMAT_SHOW_DATE or android.text.format.DateUtils.FORMAT_SHOW_TIME or android.text.format.DateUtils.FORMAT_SHOW_YEAR,
                                    ),
                                )
                            },
                            supportingContent = {
                                Text("${android.text.format.Formatter.formatShortFileSize(context, version.size.toLong())} · $reason" + if (version.by.isNotEmpty()) " by ${version.by}" else "")
                            },
                            trailingContent = if (writable) {
                                {
                                    Row {
                                        TextButton(onClick = {
                                            scope.launch {
                                                try {
                                                    vm.call(serverId) { it.restoreVersion(version.id) }
                                                    onDone("Put back the version from ${android.text.format.DateUtils.getRelativeTimeSpanString(version.modified)}")
                                                } catch (err: Exception) {
                                                    problem = err.userMessage()
                                                }
                                            }
                                        }) { Text("Restore") }
                                        IconButton(onClick = { deleting = version }) { Icon(Icons.Outlined.Delete, contentDescription = "Delete this version") }
                                    }
                                }
                            } else {
                                null
                            },
                        )
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Close") } },
    )
    deleting?.let { version ->
        AlertDialog(
            onDismissRequest = { deleting = null },
            title = { Text("Delete this version?") },
            text = { Text("It cannot be brought back.") },
            confirmButton = {
                TextButton(onClick = {
                    deleting = null
                    scope.launch {
                        try {
                            vm.call(serverId) { it.deleteVersion(version.id) }
                            reload++
                        } catch (err: Exception) {
                            problem = err.userMessage()
                        }
                    }
                }) { Text("Delete") }
            },
            dismissButton = { TextButton(onClick = { deleting = null }) { Text("Cancel") } },
        )
    }
}
