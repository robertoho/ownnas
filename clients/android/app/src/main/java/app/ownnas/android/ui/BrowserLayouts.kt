package app.ownnas.android.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.staggeredgrid.LazyVerticalStaggeredGrid
import androidx.compose.foundation.lazy.staggeredgrid.StaggeredGridCells
import androidx.compose.foundation.lazy.staggeredgrid.StaggeredGridItemSpan
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.automirrored.outlined.ViewList
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.outlined.Dashboard
import androidx.compose.material.icons.outlined.GridView
import androidx.compose.material.icons.outlined.ViewWeek
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.ownnas.core.AgentLauncher
import app.ownnas.core.Entry
import app.ownnas.core.Folder
import app.ownnas.core.TerminalInfo

/** How a folder is shown, as the web app's view buttons. */
enum class BrowserLayout(val label: String, val icon: ImageVector) {
    List("List", androidx.compose.material.icons.Icons.AutoMirrored.Outlined.ViewList),
    Grid("Icons", androidx.compose.material.icons.Icons.Outlined.GridView),
    Masonry("Masonry", androidx.compose.material.icons.Icons.Outlined.Dashboard),
    Columns("Columns", androidx.compose.material.icons.Icons.Outlined.ViewWeek),
    ;

    companion object {
        fun from(name: String?, fallback: BrowserLayout) = entries.firstOrNull { it.name == name } ?: fallback
    }
}

/** The layouts chosen for narrow and wide screens, and the tile size (0 small, 1 medium, 2 large). */
data class BrowserLayouts(val narrow: BrowserLayout, val wide: BrowserLayout, val tile: Int)

/** Columns need room: a phone on its side or an unfolded foldable. */
const val WIDE_SCREEN_DP = 600

/** Whether the window is wide right now; changes when the phone turns or unfolds. */
@Composable
fun isWideWindow(): Boolean = LocalConfiguration.current.screenWidthDp >= WIDE_SCREEN_DP

private val tileSizes = listOf(96.dp, 136.dp, 196.dp)
private val tileLabels = listOf("Small", "Medium", "Large")

/** The top bar's view button: layouts, and the tile size for Icons and Masonry. */
@Composable
fun LayoutButton(layouts: BrowserLayouts, wide: Boolean, onLayout: (BrowserLayout) -> Unit, onTile: (Int) -> Unit) {
    val current = if (wide) layouts.wide else layouts.narrow
    val shown = if (current == BrowserLayout.Columns && !wide) BrowserLayout.List else current
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) { Icon(shown.icon, contentDescription = "View: ${shown.label}") }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            BrowserLayout.entries.forEach { layout ->
                val unavailable = layout == BrowserLayout.Columns && !wide
                DropdownMenuItem(
                    text = {
                        Column {
                            Text(layout.label)
                            if (unavailable) {
                                Text("Turn the phone sideways or unfold it", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    },
                    leadingIcon = { Icon(layout.icon, contentDescription = null) },
                    trailingIcon = if (layout == current) {
                        { Icon(Icons.Filled.CheckCircle, contentDescription = "Chosen", tint = MaterialTheme.colorScheme.primary) }
                    } else {
                        null
                    },
                    // Chosen even when narrow: it is remembered for wide screens too.
                    onClick = { open = false; onLayout(layout) },
                )
            }
            if (shown == BrowserLayout.Grid || shown == BrowserLayout.Masonry) {
                HorizontalDivider()
                tileLabels.forEachIndexed { index, label ->
                    DropdownMenuItem(
                        text = { Text("$label tiles") },
                        trailingIcon = if (layouts.tile == index) {
                            { Icon(Icons.Filled.CheckCircle, contentDescription = "Chosen", tint = MaterialTheme.colorScheme.primary) }
                        } else {
                            null
                        },
                        onClick = { open = false; onTile(index) },
                    )
                }
            }
        }
    }
}

/** What every layout needs to show and act on a folder's entries. */
class EntryActions(
    val vm: AppViewModel,
    val serverId: String,
    val writable: Boolean,
    val terminalsOn: Boolean,
    val terminalsByPath: Map<String, List<TerminalInfo>>,
    val selected: Set<String>,
    val onOpen: (Entry) -> Unit,
    val onSelect: (Entry) -> Unit,
    val onAction: (Bulk, Entry) -> Unit,
    val onTerminal: (Entry) -> Unit,
    val agents: List<AgentLauncher>,
    val onAgent: (Entry, AgentLauncher) -> Unit,
) {
    val selecting get() = selected.isNotEmpty()
}

fun sortedEntries(folder: Folder): List<Entry> =
    folder.entries.sortedWith(compareBy<Entry> { !it.dir }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name })

/** A folder in the chosen layout. Columns fall back to List on a narrow screen. */
@Composable
fun FolderLayout(
    layout: BrowserLayout,
    tile: Int,
    wide: Boolean,
    folder: Folder?,
    path: String,
    serverName: String,
    actions: EntryActions,
    onBrowse: (String) -> Unit,
) {
    val shown = if (layout == BrowserLayout.Columns && !wide) BrowserLayout.List else layout
    if (shown == BrowserLayout.Columns) {
        ColumnsView(path, folder, serverName, actions, onBrowse)
        return
    }
    if (folder == null) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        return
    }
    val entries = remember(folder) { sortedEntries(folder) }
    // Room under the last entries for the add button.
    val bottom = PaddingValues(bottom = 88.dp)
    when (shown) {
        BrowserLayout.List -> LazyColumn(Modifier.fillMaxSize(), contentPadding = bottom) {
            items(entries, key = { it.path }) { EntryRow(it, actions) }
            if (folder.truncated) item { Truncated(folder) }
        }
        BrowserLayout.Grid -> LazyVerticalGrid(
            columns = GridCells.Adaptive(tileSizes[tile]),
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = 8.dp, top = 8.dp, end = 8.dp, bottom = 88.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(entries.size, key = { entries[it].path }) { EntryTile(entries[it], actions, square = true) }
            if (folder.truncated) item(span = { GridItemSpan(maxLineSpan) }) { Truncated(folder) }
        }
        BrowserLayout.Masonry -> LazyVerticalStaggeredGrid(
            columns = StaggeredGridCells.Adaptive(tileSizes[tile]),
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = 8.dp, top = 8.dp, end = 8.dp, bottom = 88.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalItemSpacing = 8.dp,
        ) {
            items(entries.size, key = { entries[it].path }) { EntryTile(entries[it], actions, square = false) }
            if (folder.truncated) item(span = StaggeredGridItemSpan.FullLine) { Truncated(folder) }
        }
        BrowserLayout.Columns -> {}
    }
}

@Composable
private fun Truncated(folder: Folder) {
    Text(
        "Showing the first ${folder.entries.size} items. Open this folder in the browser to see the rest.",
        Modifier.fillMaxWidth().padding(24.dp),
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

/** The menu on an entry's ⋮: select it, open a terminal or an agent, and its actions. */
@Composable
private fun EntryMoreButton(entry: Entry, actions: EntryActions, modifier: Modifier = Modifier) {
    var menu by remember { mutableStateOf(false) }
    Box(modifier) {
        IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "Actions for ${entry.name}") }
        EntryMenu(
            entry = entry,
            actions = bulkActions(listOf(entry), actions.writable),
            terminals = actions.terminalsOn,
            expanded = menu,
            onDismiss = { menu = false },
            onSelect = { actions.onSelect(entry) },
            onTerminal = { actions.onTerminal(entry) },
            onAction = { actions.onAction(it, entry) },
            agents = actions.agents,
            onAgent = { actions.onAgent(entry, it) },
        )
    }
}

@Composable
private fun SelectedMark(modifier: Modifier) {
    Icon(
        Icons.Filled.CheckCircle,
        contentDescription = "Selected",
        tint = MaterialTheme.colorScheme.primary,
        modifier = modifier.size(22.dp).background(MaterialTheme.colorScheme.surface, CircleShape),
    )
}

/** One entry as a row: thumbnail, name, size and date, terminal status, and ⋮. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun EntryRow(entry: Entry, actions: EntryActions, highlighted: Boolean = false) {
    val context = LocalContext.current
    val here = if (entry.dir) actions.terminalsByPath[entry.path].orEmpty() else emptyList()
    val chosen = entry.path in actions.selected
    ListItem(
        modifier = Modifier.combinedClickable(onClick = { actions.onOpen(entry) }, onLongClick = { actions.onSelect(entry) }),
        colors = when {
            chosen -> ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.secondaryContainer)
            highlighted -> ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.surfaceContainerHighest)
            else -> ListItemDefaults.colors()
        },
        leadingContent = {
            Box {
                Thumbnail(actions.vm, actions.serverId, entry)
                if (chosen) SelectedMark(Modifier.align(Alignment.TopStart).offset(x = (-6).dp, y = (-6).dp))
                if (here.isNotEmpty()) TerminalMark(here, Modifier.align(Alignment.BottomEnd).offset(x = 4.dp, y = 4.dp))
            }
        },
        headlineContent = { Text(specialFolderLabel(entry.name), maxLines = 1, overflow = TextOverflow.Ellipsis) },
        supportingContent = { Text(details(context, entry) + terminalNote(here), maxLines = 1, overflow = TextOverflow.Ellipsis) },
        trailingContent = if (actions.selecting) null else { { EntryMoreButton(entry, actions) } },
    )
}

/** How a folder's terminals are doing, after its details. */
private fun terminalNote(here: List<TerminalInfo>): String {
    val running = here.filter { it.running }
    val status = running.firstNotNullOfOrNull { info -> app.ownnas.android.ui.terminal.agentStatus(info)?.first }
    return when {
        status != null -> " · $status"
        running.size == 1 -> " · terminal open"
        running.size > 1 -> " · ${running.size} terminals open"
        here.isNotEmpty() -> " · terminal ${app.ownnas.android.ui.terminal.endedText(here.first()).lowercase()}"
        else -> ""
    }
}

/** A thumbnail, loaded once and kept in the app's memory cache. */
@Composable
fun rememberThumbnail(vm: AppViewModel, serverId: String, entry: Entry): ImageBitmap? {
    val key = "$serverId:${entry.path}:${entry.modified}"
    val image by produceState(vm.cachedThumbnail(key), key) {
        if (value == null && entry.thumb) value = vm.thumbnail(serverId, entry, key)
    }
    return image
}

/**
 * One entry as a tile: a large thumbnail (square for Icons; its own shape for Masonry) or the
 * kind's icon, the name, and ⋮.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun EntryTile(entry: Entry, actions: EntryActions, square: Boolean) {
    val context = LocalContext.current
    val here = if (entry.dir) actions.terminalsByPath[entry.path].orEmpty() else emptyList()
    val chosen = entry.path in actions.selected
    val image = rememberThumbnail(actions.vm, actions.serverId, entry)
    Surface(
        shape = RoundedCornerShape(12.dp),
        color = if (chosen) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.surfaceContainerLow,
        modifier = Modifier.fillMaxWidth().combinedClickable(onClick = { actions.onOpen(entry) }, onLongClick = { actions.onSelect(entry) }),
    ) {
        Column {
            val ratio = when {
                square || image == null -> 1f
                else -> (image.width.toFloat() / image.height.coerceAtLeast(1)).coerceIn(0.5f, 2f)
            }
            Box(Modifier.fillMaxWidth().aspectRatio(ratio).padding(6.dp).clip(RoundedCornerShape(8.dp))) {
                if (image != null) {
                    Image(image, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
                } else {
                    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainerHighest), contentAlignment = Alignment.Center) {
                        Icon(
                            iconFor(entry),
                            contentDescription = null,
                            tint = if (entry.dir) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.fillMaxSize(0.45f),
                        )
                    }
                }
                if (chosen) SelectedMark(Modifier.align(Alignment.TopStart).padding(4.dp))
                if (here.isNotEmpty()) TerminalMark(here, Modifier.align(Alignment.BottomEnd).padding(4.dp))
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f).padding(start = 10.dp, bottom = 8.dp)) {
                    Text(specialFolderLabel(entry.name), maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium)
                    Text(
                        details(context, entry),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (!actions.selecting) EntryMoreButton(entry, actions)
            }
        }
    }
}

/** The folders above `path`, from the top: "Docs/sub" → "", "Docs". */
private fun ancestors(path: String): List<String> {
    if (path.isEmpty()) return emptyList()
    val parts = path.split('/')
    return listOf("") + parts.dropLast(1).indices.map { parts.take(it + 1).joinToString("/") }
}

/**
 * Miller columns, as the web app's column view: the folders above this one, narrow, then this
 * folder with selection and every action. Tapping a folder in any column opens it.
 */
@Composable
private fun ColumnsView(path: String, folder: Folder?, serverName: String, actions: EntryActions, onBrowse: (String) -> Unit) {
    val chain = remember(path) { ancestors(path) }
    val state = rememberLazyListState()
    // Slide to the new column; the ones before it stay where they are.
    LaunchedEffect(path) { state.animateScrollToItem(chain.size) }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val narrow: Dp = 240.dp
        val current: Dp = maxOf(320.dp, maxWidth - narrow * minOf(chain.size, 2))
        LazyRow(state = state, modifier = Modifier.fillMaxSize()) {
            items(chain.size, key = { "column:${chain[it]}" }) { index ->
                val above = chain[index]
                val next = chain.getOrNull(index + 1) ?: path
                Row(Modifier.width(narrow).fillMaxHeight()) {
                    AboveColumn(actions, above, next, serverName, onBrowse, Modifier.weight(1f))
                    VerticalDivider()
                }
            }
            item(key = "column:$path") {
                Box(Modifier.width(current).fillMaxHeight()) {
                    when {
                        folder == null -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                        folder.entries.isEmpty() -> Text(
                            "This folder is empty",
                            Modifier.align(Alignment.Center).padding(24.dp),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        else -> {
                            val entries = remember(folder) { sortedEntries(folder) }
                            LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 88.dp)) {
                                items(entries, key = { it.path }) { EntryRow(it, actions) }
                                if (folder.truncated) item { Truncated(folder) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/** A folder above the one shown, with the way down highlighted. */
@Composable
private fun AboveColumn(actions: EntryActions, path: String, next: String, serverName: String, onBrowse: (String) -> Unit, modifier: Modifier) {
    val listed by produceState(actions.vm.cachedFolder(actions.serverId, path), path) {
        runCatching { actions.vm.list(actions.serverId, path) }.onSuccess { value = it }
    }
    val shown = listed
    Column(modifier.fillMaxHeight()) {
        Text(
            specialFolderLabel(path.substringAfterLast('/')).ifEmpty { serverName },
            Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (shown == null) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            return@Column
        }
        val entries = remember(shown) { sortedEntries(shown) }
        LazyColumn(Modifier.fillMaxSize(), state = rememberLazyListState(initialFirstVisibleItemIndex = entries.indexOfFirst { it.path == next }.coerceAtLeast(0))) {
            items(entries, key = { it.path }) { entry ->
                val onPath = entry.path == next
                ListItem(
                    modifier = Modifier.clickable { if (entry.dir) onBrowse(entry.path) else actions.onOpen(entry) },
                    colors = if (onPath) ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.secondaryContainer) else ListItemDefaults.colors(),
                    leadingContent = { Thumbnail(actions.vm, actions.serverId, entry) },
                    headlineContent = { Text(specialFolderLabel(entry.name), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    trailingContent = if (entry.dir) {
                        { Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, contentDescription = null) }
                    } else {
                        null
                    },
                )
            }
        }
    }
}
