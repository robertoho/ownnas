package app.ownnas.android.ui

import android.content.Context
import android.content.Intent
import android.text.format.DateUtils
import android.text.format.Formatter
import android.webkit.MimeTypeMap
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.ContentPaste
import androidx.compose.material.icons.outlined.CreateNewFolder
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.DocumentScanner
import androidx.compose.material.icons.outlined.DriveFileRenameOutline
import androidx.compose.material.icons.outlined.PhotoLibrary
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material.icons.outlined.SmartToy
import androidx.compose.material.icons.outlined.UploadFile
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import app.ownnas.core.Conflict
import app.ownnas.core.Entry
import java.io.File

/** A row's ⋮ menu: select it, open a terminal in a folder, and the selection actions for it alone. */
@Composable
fun EntryMenu(
    entry: Entry,
    actions: List<Bulk>,
    terminals: Boolean,
    expanded: Boolean,
    onDismiss: () -> Unit,
    onSelect: () -> Unit,
    onTerminal: () -> Unit,
    onAction: (Bulk) -> Unit,
    agents: List<app.ownnas.core.AgentLauncher> = emptyList(),
    onAgent: (app.ownnas.core.AgentLauncher) -> Unit = {},
    /** "Paste 3 items into it", for a folder while something is cut or copied. */
    pasteInto: String? = null,
    onPasteInto: () -> Unit = {},
) {
    DropdownMenu(expanded = expanded, onDismissRequest = onDismiss) {
        MenuItem("Select", Icons.Outlined.CheckCircle) { onDismiss(); onSelect() }
        if (entry.dir && pasteInto != null) MenuItem(pasteInto, Icons.Outlined.ContentPaste) { onDismiss(); onPasteInto() }
        // As in the web app's folder menu.
        if (entry.dir && terminals && !inTrash(entry.path)) {
            MenuItem("Open terminal here", Icons.Outlined.Terminal) { onDismiss(); onTerminal() }
            agents.forEach { agent -> MenuItem("Open ${agent.label} here", Icons.Outlined.SmartToy) { onDismiss(); onAgent(agent) } }
        }
        actions.forEach { action -> MenuItem(action.label(listOf(entry)), action.icon) { onDismiss(); onAction(action) } }
    }
}

@Composable
private fun MenuItem(label: String, icon: ImageVector, onClick: () -> Unit) {
    DropdownMenuItem(text = { Text(label) }, leadingIcon = { Icon(icon, contentDescription = null) }, onClick = onClick)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AddSheet(onDismiss: () -> Unit, onMedia: () -> Unit, onFiles: () -> Unit, onScan: () -> Unit, onFolder: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.navigationBarsPadding().padding(bottom = 16.dp)) {
            SheetRow("Upload photos and videos", Icons.Outlined.PhotoLibrary, onMedia)
            SheetRow("Upload files", Icons.Outlined.UploadFile, onFiles)
            SheetRow("Scan documents", Icons.Outlined.DocumentScanner, onScan)
            SheetRow("New folder", Icons.Outlined.CreateNewFolder, onFolder)
        }
    }
}

@Composable
private fun SheetRow(label: String, icon: ImageVector, onClick: () -> Unit) {
    ListItem(
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick),
        colors = ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
        leadingContent = { Icon(icon, contentDescription = null) },
        headlineContent = { Text(label) },
    )
}

/** Asks for a file or folder name. For renames of files, the part before the extension starts selected. */
@Composable
fun NameDialog(
    title: String,
    initial: String,
    confirm: String,
    onDismiss: () -> Unit,
    onConfirm: (String) -> Unit,
    selectStem: Boolean = false,
) {
    val stem = if (selectStem) initial.lastIndexOf('.').takeIf { it > 0 } ?: initial.length else initial.length
    var value by remember { mutableStateOf(TextFieldValue(initial, TextRange(0, stem))) }
    val focus = remember { FocusRequester() }
    val name = value.text.trim()
    val valid = name.isNotEmpty() && name != "." && name != ".." && '/' !in name && '\\' !in name
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            OutlinedTextField(
                value = value,
                onValueChange = { value = it },
                singleLine = true,
                label = { Text("Name") },
                isError = name.isNotEmpty() && !valid,
                supportingText = if (name.isNotEmpty() && !valid) {
                    { Text("Names cannot contain / or \\") }
                } else {
                    null
                },
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { if (valid) onConfirm(name) }),
                modifier = Modifier.fillMaxWidth().focusRequester(focus),
            )
        },
        confirmButton = { TextButton(onClick = { onConfirm(name) }, enabled = valid) { Text(confirm) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
    LaunchedEffect(Unit) { focus.requestFocus() }
}

@Composable
fun TrashDialog(entries: List<Entry>, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    val one = entries.singleOrNull()
    if (entries.all { inTrash(it.path) }) {
        AlertDialog(
            onDismissRequest = onDismiss,
            icon = { Icon(Icons.Outlined.Delete, contentDescription = null) },
            title = { Text("Delete permanently?") },
            text = {
                Text(
                    if (one != null) "“${one.name}” is already in the Trash. Deleting it here cannot be undone."
                    else "These ${entries.size} items are already in the Trash. Deleting them here cannot be undone.",
                )
            },
            confirmButton = { TextButton(onClick = onConfirm) { Text("Delete") } },
            dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
        )
        return
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(Icons.Outlined.Delete, contentDescription = null) },
        title = { Text("Move to Trash?") },
        text = {
            Text(
                when {
                    one == null -> "These ${entries.size} items go to the server's Trash. You can restore them from the Trash."
                    one.dir -> "“${one.name}” and everything in it go to the server's Trash. You can restore them from the Trash."
                    else -> "“${one.name}” goes to the server's Trash. You can restore it from the Trash."
                },
            )
        },
        confirmButton = { TextButton(onClick = onConfirm) { Text("Move to Trash") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Progress while a file downloads to be opened or shared. */
@Composable
fun FetchDialog(name: String, done: Long, size: Long, sharing: Boolean, onCancel: () -> Unit) {
    val context = LocalContext.current
    AlertDialog(
        onDismissRequest = {},
        title = { Text(if (sharing) "Preparing to share" else "Opening", maxLines = 1) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(name, maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (size > 0) {
                    LinearProgressIndicator(progress = { (done.toFloat() / size).coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
                } else {
                    LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                }
                Text(
                    "${Formatter.formatShortFileSize(context, done)} of ${Formatter.formatShortFileSize(context, size)}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onCancel) { Text("Cancel") } },
    )
}

/** Sits under the file list while uploads run. */
@Composable
fun UploadBar(status: UploadStatus, onCancel: () -> Unit) {
    val context = LocalContext.current
    Surface(color = MaterialTheme.colorScheme.surfaceContainer, tonalElevation = 3.dp) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 16.dp, vertical = 10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(
                        "Uploading ${minOf(status.finished + status.failed + 1, status.total)} of ${status.total}",
                        style = MaterialTheme.typography.titleSmall,
                    )
                    val size = status.currentSize
                    Text(
                        buildString {
                            append(status.current ?: "")
                            if (size != null) append(" · ${Formatter.formatShortFileSize(context, status.currentDone)} of ${Formatter.formatShortFileSize(context, size)}")
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                TextButton(onClick = onCancel) { Text("Cancel") }
            }
            val size = status.currentSize
            if (size != null && size > 0) {
                LinearProgressIndicator(progress = { (status.currentDone.toFloat() / size).coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
            } else {
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
            }
        }
    }
}

internal fun File.contentUri(context: Context) = FileProvider.getUriForFile(context, "${context.packageName}.files", this)

internal fun File.mimeType() =
    MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension.lowercase()) ?: "application/octet-stream"

/** Shows the file in whichever app handles its type, such as Photos, a PDF reader, or a player. */
fun openWithAnotherApp(context: Context, file: File) {
    val intent = Intent(Intent.ACTION_VIEW)
        .setDataAndType(file.contentUri(context), file.mimeType())
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    context.startActivity(intent)
}

/** Offers the file to the share sheet: messages, mail, nearby devices, and so on. */
fun shareWithAnotherApp(context: Context, file: File) {
    val send = Intent(Intent.ACTION_SEND)
        .setType(file.mimeType())
        .putExtra(Intent.EXTRA_STREAM, file.contentUri(context))
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    context.startActivity(Intent.createChooser(send, null))
}

/** Offers several files to the share sheet at once. */
fun shareManyWithAnotherApp(context: Context, files: List<File>) {
    if (files.size == 1) return shareWithAnotherApp(context, files[0])
    val types = files.map { it.mimeType() }.distinct()
    val type = types.singleOrNull() ?: types.map { it.substringBefore('/') }.distinct().singleOrNull()?.let { "$it/*" } ?: "*/*"
    val send = Intent(Intent.ACTION_SEND_MULTIPLE)
        .setType(type)
        .putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(files.map { it.contentUri(context) }))
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    context.startActivity(Intent.createChooser(send, null))
}

/** The web app's "File already exists" question, with the same choices. */
@Composable
fun ConflictDialog(prompt: ConflictPrompt) {
    val context = LocalContext.current
    var rest by remember(prompt) { mutableStateOf(false) }
    val existing = prompt.existing
    fun describe(size: Long, modified: Long) =
        "${Formatter.formatShortFileSize(context, size)}, ${DateUtils.formatDateTime(context, modified, DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_SHOW_TIME or DateUtils.FORMAT_SHOW_YEAR)}"
    val note = if (existing.dir) {
        "A folder with this name is already here. Keep both saves the upload under a new name."
    } else {
        buildString {
            append("Already here: ${describe(existing.size.toLong(), existing.modified)}.")
            val size = prompt.incomingSize
            val modified = prompt.incomingModified
            if (size != null && modified != null) {
                append(" This upload: ${describe(size, modified)}.")
                // Compare whole seconds, as the server does.
                val here = existing.modified / 1000
                val incoming = modified / 1000
                append(
                    when {
                        here < incoming -> " The file already here is older."
                        here > incoming -> " This upload is older."
                        else -> " Both have the same date."
                    },
                )
            }
        }
    }
    val choices = listOf(
        Conflict.OVERWRITE to "Overwrite",
        Conflict.KEEP_BOTH to "Keep both",
        Conflict.ARCHIVE_OLDER to "Archive older",
        Conflict.ARCHIVE_EXISTING to "Archive existing",
        Conflict.IGNORE to "Ignore",
    ).filter { (choice, _) -> prompt.allows(choice) }
    AlertDialog(
        onDismissRequest = {},
        title = { Text(if (existing.dir) "Folder already exists" else "File already exists") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(if (existing.dir) "A folder named “${prompt.name}” already exists." else "“${prompt.name}” already exists.")
                Text(note, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                // Shown even for a single file, exactly as the web app's dialog does.
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth().clickable { rest = !rest },
                ) {
                    Checkbox(checked = rest, onCheckedChange = { rest = it })
                    Text("Use this choice for the remaining files")
                }
                Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) {
                    choices.forEach { (choice, label) ->
                        TextButton(onClick = { prompt.answer(ConflictAnswer(choice, rest)) }) { Text(label) }
                    }
                    TextButton(onClick = { prompt.answer(null) }) { Text("Cancel") }
                }
            }
        },
        confirmButton = {},
    )
}
