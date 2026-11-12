package app.ownnas.android.ui

import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.ownnas.core.DuplicateChoice
import app.ownnas.core.Duplicates
import app.ownnas.core.SortResult
import java.text.DateFormatSymbols
import kotlinx.coroutines.launch

/** A folder's name for titles, or the server's for its top. */
private fun folderTitle(path: String, serverName: String) =
    specialFolderLabel(path.substringAfterLast('/')).ifEmpty { serverName }

/**
 * Files with the same contents in a folder and below. In each group the oldest copy is kept
 * and the others are ticked for the Trash; untick any to keep them too. At least one copy of
 * each group always stays.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DuplicatesScreen(vm: AppViewModel, serverId: String, path: String) {
    val server = vm.server(serverId) ?: return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var found by remember { mutableStateOf<Duplicates?>(null) }
    var problem by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    var confirming by remember { mutableStateOf(false) }
    var working by remember { mutableStateOf(false) }
    val remove = remember { mutableStateMapOf<String, Boolean>() }

    LaunchedEffect(reload) {
        found = null
        problem = null
        try {
            val result = vm.call(serverId) { it.duplicates(path) }
            remove.clear()
            result.groups.forEach { group ->
                val keep = group.copies.minByOrNull { it.modified }?.path
                group.copies.forEach { remove[it.path] = it.path != keep }
            }
            found = result
        } catch (err: Exception) {
            problem = err.userMessage()
        }
    }

    val groups = found?.groups.orEmpty()
    val chosen = groups.flatMap { group -> group.copies.filter { remove[it.path] == true }.map { it to group.size } }
    val freeing = chosen.sumOf { it.second.toLong() }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text("Duplicates")
                        Text(folderTitle(path, server.name), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                },
                navigationIcon = { IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") } },
            )
        },
        bottomBar = {
            if (chosen.isNotEmpty()) {
                Column {
                    HorizontalDivider()
                    Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            "${chosen.size} ${if (chosen.size == 1) "copy" else "copies"} · ${Formatter.formatShortFileSize(context, freeing)}",
                            Modifier.weight(1f),
                        )
                        Button(onClick = { confirming = true }, enabled = !working) { Text("Move to Trash") }
                    }
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            val result = found
            when {
                problem != null -> Text(problem!!, Modifier.align(Alignment.Center).padding(32.dp), textAlign = TextAlign.Center)
                result == null -> Column(Modifier.align(Alignment.Center), horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator()
                    Text("Comparing files…", Modifier.padding(top = 12.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                groups.isEmpty() -> Text("No duplicates here.", Modifier.align(Alignment.Center), color = MaterialTheme.colorScheme.onSurfaceVariant)
                else -> LazyColumn(contentPadding = PaddingValues(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    item {
                        Text(
                            "${groups.size} ${if (groups.size == 1) "file has" else "files have"} copies, taking " +
                                Formatter.formatShortFileSize(context, result.wasted.toLong()) + " extra." +
                                if (result.truncated) " Showing the largest." else "",
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    items(groups, key = { it.sha256 }) { group ->
                        ElevatedCard(Modifier.fillMaxWidth()) {
                            Text(
                                "${Formatter.formatShortFileSize(context, group.size.toLong())} · ${group.copies.size} copies",
                                Modifier.padding(start = 16.dp, top = 12.dp, end = 16.dp),
                                style = MaterialTheme.typography.titleSmall,
                            )
                            group.copies.forEach { copy ->
                                val ticked = remove[copy.path] == true
                                val lastKept = !ticked && group.copies.count { remove[it.path] != true } == 1
                                ListItem(
                                    modifier = Modifier.clickable(enabled = !lastKept) { remove[copy.path] = !ticked },
                                    leadingContent = { Checkbox(checked = ticked, onCheckedChange = { remove[copy.path] = it }, enabled = !lastKept) },
                                    headlineContent = { Text(copy.path.substringAfterLast('/'), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                                    supportingContent = {
                                        Text(
                                            (copy.path.substringBeforeLast('/', "").ifEmpty { server.name }) + " · " +
                                                DateUtils.getRelativeTimeSpanString(copy.modified) + if (ticked) " · to the Trash" else " · kept",
                                            maxLines = 2,
                                            overflow = TextOverflow.Ellipsis,
                                        )
                                    },
                                )
                            }
                        }
                    }
                }
            }
        }
    }

    if (confirming) {
        AlertDialog(
            onDismissRequest = { confirming = false },
            title = { Text("Move ${chosen.size} ${if (chosen.size == 1) "copy" else "copies"} to the Trash?") },
            text = { Text("Each is checked again first: it must still match, and a copy of it stays. You can restore them from the Trash.") },
            confirmButton = {
                TextButton(onClick = {
                    confirming = false
                    working = true
                    val choices = groups.mapNotNull { group ->
                        val gone = group.copies.filter { remove[it.path] == true }.map { it.path }
                        if (gone.isEmpty()) null else DuplicateChoice(group.sha256, group.copies.map { it.path } - gone.toSet(), gone)
                    }
                    scope.launch {
                        try {
                            val done = vm.call(serverId) { it.trashDuplicates(choices) }
                            vm.changedFolder(serverId, path)
                            snackbar.showSnackbar(
                                "Moved ${done.trashed} to the Trash, freeing ${Formatter.formatShortFileSize(context, done.freed.toLong())}" +
                                    if (done.skipped.isNotEmpty()) ". Left ${done.skipped.size}: ${done.skipped.first()}" else "",
                            )
                            reload++
                        } catch (err: Exception) {
                            snackbar.showSnackbar(err.userMessage())
                        } finally {
                            working = false
                        }
                    }
                }) { Text("Move to Trash") }
            },
            dismissButton = { TextButton(onClick = { confirming = false }) { Text("Cancel") } },
        )
    }
}

/** The patterns the web app offers for sorting a folder. */
private val sortPatterns = listOf(
    "{YYYY}/{MM}" to "Year / month (2024/03)",
    "{YYYY}-{MM}" to "Year-month (2024-03)",
    "{YYYY}/{MM}/{DD}" to "Year / month / day",
    "{YYYY}" to "Year (2024)",
    "{YYYY}/{MM} {MMMM}" to "Year / month with its name",
    "{type}" to "Kind (Pictures, Videos, Documents…)",
    "{ext}" to "Extension (jpg, pdf…)",
    "{first}" to "First letter (A, B…)",
    "" to "My own pattern…",
)

/** Month names in the phone's language, for {MMMM}. */
private fun monthNames(): List<String> =
    DateFormatSymbols.getInstance().months.take(12).map { name -> name.replaceFirstChar { it.titlecase() } }

/** Sorts a folder's files into folders by date, kind, extension, or first letter. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SortFolderScreen(vm: AppViewModel, serverId: String, path: String) {
    val server = vm.server(serverId) ?: return
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var preset by remember { mutableStateOf("{YYYY}/{MM}") }
    var custom by remember { mutableStateOf("Photos {YYYY}") }
    var date by remember { mutableStateOf("taken") }
    var recursive by remember { mutableStateOf(false) }
    var preview by remember { mutableStateOf<SortResult?>(null) }
    var problem by remember { mutableStateOf<String?>(null) }
    var working by remember { mutableStateOf(false) }
    val pattern = if (preset.isEmpty()) custom.trim() else preset
    val usesDate = "{Y" in pattern || "{M" in pattern || "{D" in pattern

    // A changed choice needs a new preview.
    LaunchedEffect(pattern, date, recursive) { preview = null; problem = null }

    fun run(apply: Boolean) {
        working = true
        problem = null
        scope.launch {
            try {
                val result = vm.call(serverId) { it.sortFolder(path, pattern, date, recursive, monthNames(), apply) }
                if (!apply) {
                    preview = result
                } else {
                    preview = null
                    vm.changedFolder(serverId, path)
                    val shown = snackbar.showSnackbar(
                        "Moved ${result.total} ${if (result.total == 1UL) "file" else "files"} into ${result.folders} ${if (result.folders == 1UL) "folder" else "folders"}" +
                            if (result.failed > 0UL) ", ${result.failed} could not be moved" else "",
                        actionLabel = if (result.moves.isNotEmpty()) "Undo" else null,
                        duration = SnackbarDuration.Long,
                    )
                    if (shown == SnackbarResult.ActionPerformed) {
                        val back = vm.call(serverId) { it.undoSort(result.moves) }
                        vm.changedFolder(serverId, path)
                        snackbar.showSnackbar("Put back $back ${if (back == 1UL) "file" else "files"}")
                    }
                }
            } catch (err: Exception) {
                problem = err.userMessage()
            } finally {
                working = false
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text("Sort into folders")
                        Text(folderTitle(path, server.name), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                },
                navigationIcon = { IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") } },
            )
        },
        bottomBar = {
            Column {
                HorizontalDivider()
                Row(Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.End)) {
                    OutlinedButton(onClick = { run(apply = false) }, enabled = pattern.isNotEmpty() && !working) { Text("Preview") }
                    Button(
                        onClick = { run(apply = true) },
                        enabled = pattern.isNotEmpty() && !working && preview?.let { it.total > 0UL } == true,
                    ) { Text("Sort") }
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(bottom = 16.dp)) {
            item { Text("Folders by", Modifier.padding(16.dp, 12.dp, 16.dp, 4.dp), style = MaterialTheme.typography.titleSmall) }
            items(sortPatterns, key = { it.second }) { (value, label) ->
                Row(Modifier.fillMaxWidth().clickable { preset = value }.padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    RadioButton(selected = preset == value, onClick = { preset = value })
                    Text(label)
                }
            }
            if (preset.isEmpty()) {
                item {
                    OutlinedTextField(
                        value = custom,
                        onValueChange = { custom = it },
                        singleLine = true,
                        label = { Text("Pattern") },
                        supportingText = { Text("{YYYY} year, {MM} month, {DD} day, {MMMM} month name, {type} kind, {ext} extension, {first} first letter, / for a folder inside a folder") },
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                    )
                }
            }
            if (usesDate) {
                item {
                    Text("Dates from", Modifier.padding(16.dp, 12.dp, 16.dp, 8.dp), style = MaterialTheme.typography.titleSmall)
                    val dates = listOf("taken" to "Date taken", "modified" to "Modified", "name" to "File name")
                    SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth().padding(horizontal = 16.dp)) {
                        dates.forEachIndexed { index, (value, label) ->
                            SegmentedButton(selected = date == value, onClick = { date = value }, shape = SegmentedButtonDefaults.itemShape(index, dates.size)) { Text(label, maxLines = 1) }
                        }
                    }
                }
            }
            item {
                Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("Include subfolders")
                        Text("Files in folders inside it are sorted too", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Switch(checked = recursive, onCheckedChange = { recursive = it })
                }
            }
            problem?.let { item { Text(it, Modifier.padding(16.dp), color = MaterialTheme.colorScheme.error) } }
            if (working) item { Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator() } }
            preview?.let { shown ->
                item {
                    HorizontalDivider()
                    Text(
                        if (shown.total == 0UL) {
                            "Nothing to move" + if (shown.inPlace > 0UL) ": ${shown.inPlace} already in place." else "."
                        } else {
                            "${shown.total} ${if (shown.total == 1UL) "file goes" else "files go"} into ${shown.folders} ${if (shown.folders == 1UL) "folder" else "folders"}" +
                                if (shown.inPlace > 0UL) " (${shown.inPlace} already in place)." else "."
                        },
                        Modifier.padding(16.dp),
                        style = MaterialTheme.typography.titleSmall,
                    )
                }
                items(shown.moves.take(200), key = { it.from }) { move ->
                    ListItem(
                        headlineContent = { Text(move.from.substringAfterLast('/'), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        supportingContent = { Text("→ ${move.to.removePrefix(path).trimStart('/').ifEmpty { move.to }}", maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    )
                }
                if (shown.total > shown.moves.size.toULong().coerceAtMost(200UL)) {
                    item { Text("…and ${shown.total - shown.moves.size.toULong().coerceAtMost(200UL)} more", Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant) }
                }
            }
        }
    }
}

/** What people did on the server lately, newest first. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ActivityScreen(vm: AppViewModel, serverId: String) {
    val server = vm.server(serverId) ?: return
    var events by remember { mutableStateOf<List<app.ownnas.core.ActivityEvent>?>(null) }
    var problem by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(serverId) {
        try {
            events = vm.call(serverId) { it.activity() }
        } catch (err: Exception) {
            problem = err.userMessage()
        }
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text("Activity")
                        Text(server.name, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                },
                navigationIcon = { IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") } },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            val shown = events
            when {
                problem != null -> Text(problem!!, Modifier.align(Alignment.Center).padding(32.dp), textAlign = TextAlign.Center)
                shown == null -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                shown.isEmpty() -> Text("Nothing yet.", Modifier.align(Alignment.Center), color = MaterialTheme.colorScheme.onSurfaceVariant)
                else -> LazyColumn {
                    items(shown) { event ->
                        ListItem(
                            headlineContent = { Text(event.detail.ifEmpty { event.action }, maxLines = 2, overflow = TextOverflow.Ellipsis) },
                            overlineContent = { Text(event.action.replace('-', ' ').replaceFirstChar { it.titlecase() }) },
                            supportingContent = { Text("${event.username} · ${DateUtils.getRelativeTimeSpanString(event.createdAt)}") },
                        )
                    }
                }
            }
        }
    }
}
