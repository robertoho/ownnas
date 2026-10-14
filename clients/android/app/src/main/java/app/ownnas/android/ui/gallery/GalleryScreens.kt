package app.ownnas.android.ui.gallery

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyGridScope
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.PrimaryTabRow
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.ownnas.android.ui.AppViewModel
import app.ownnas.android.ui.NameDialog
import app.ownnas.android.ui.Screen
import app.ownnas.android.ui.userMessage
import app.ownnas.core.Album
import app.ownnas.core.AlbumPage
import app.ownnas.core.MediaItem
import app.ownnas.core.Person
import app.ownnas.core.SmartFeatures
import app.ownnas.core.TagCount
import app.ownnas.core.TimelineDays
import kotlinx.coroutines.launch
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneOffset
import kotlin.math.roundToInt

// ---- The Gallery: tabs for photos, albums, people, and tags ----

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun GalleryScreen(vm: AppViewModel, serverId: String, startTab: String) {
    var smart by remember { mutableStateOf(SmartFeatures(search = false, faces = false)) }
    LaunchedEffect(serverId) { runCatching { vm.call(serverId) { it.smartFeatures() } }.onSuccess { smart = it } }
    val tabs = listOfNotNull("photos" to "Photos", "albums" to "Albums", if (smart.faces) "people" to "People" else null, "tags" to "Tags")
    var tab by rememberSaveable { mutableStateOf(startTab) }
    var searching by rememberSaveable { mutableStateOf(false) }
    var words by rememberSaveable { mutableStateOf("") }
    var menu by remember { mutableStateOf(false) }
    val selection = remember { Selection() }
    val snackbar = remember { SnackbarHostState() }
    Scaffold(
        topBar = {
            if (selection.active) {
                SelectionBar(vm, serverId, selection, smart, snackbar, inAlbum = null, inPerson = null)
            } else {
                TopAppBar(
                    title = {
                        if (searching) {
                            val focus = remember { androidx.compose.ui.focus.FocusRequester() }
                            LaunchedEffect(Unit) { focus.requestFocus() }
                            TextField(
                                value = words,
                                onValueChange = { words = it },
                                placeholder = { Text("Dog on the beach…") },
                                singleLine = true,
                                colors = TextFieldDefaults.colors(focusedContainerColor = Color.Transparent, unfocusedContainerColor = Color.Transparent),
                                keyboardActions = androidx.compose.foundation.text.KeyboardActions(onSearch = {
                                    if (words.isNotBlank()) vm.push(Screen.GalleryResults(serverId, words = words.trim()))
                                }),
                                keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(imeAction = androidx.compose.ui.text.input.ImeAction.Search),
                                modifier = Modifier.fillMaxWidth().focusRequester(focus),
                            )
                        } else {
                            Text("Gallery")
                        }
                    },
                    navigationIcon = {
                        IconButton(onClick = { if (searching) searching = false else vm.pop() }) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                        }
                    },
                    actions = {
                        if (smart.search && !searching) {
                            IconButton(onClick = { searching = true }) { Icon(Icons.Filled.Search, contentDescription = "Search photos") }
                        }
                        IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "More") }
                        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                            DropdownMenuItem(text = { Text("Hidden photos") }, onClick = { menu = false; vm.push(Screen.GalleryHidden(serverId)) })
                            DropdownMenuItem(text = { Text("Back up this phone") }, onClick = { menu = false; vm.push(Screen.Backup) })
                        }
                    },
                )
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            PrimaryTabRow(selectedTabIndex = tabs.indexOfFirst { it.first == tab }.coerceAtLeast(0)) {
                for ((key, label) in tabs) {
                    Tab(selected = tab == key, onClick = { tab = key; selection.clear() }, text = { Text(label) })
                }
            }
            Box(Modifier.weight(1f)) {
                when (tab) {
                    "albums" -> AlbumsTab(vm, serverId)
                    "people" -> PeopleTab(vm, serverId)
                    "tags" -> TagsTab(vm, serverId)
                    else -> Timeline(vm, serverId, selection)
                }
            }
        }
    }
}

/** Photos picked with a long press, and the paths' items (for the actions). */
class Selection {
    val items = mutableStateMapOf<String, MediaItem>()
    val active get() = items.isNotEmpty()
    fun toggle(item: MediaItem) {
        if (items.containsKey(item.path)) items.remove(item.path) else items[item.path] = item
    }
    fun clear() = items.clear()
}

/** What can be done with the photos picked: albums, tags, hiding, finding similar ones. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SelectionBar(
    vm: AppViewModel,
    serverId: String,
    selection: Selection,
    smart: SmartFeatures,
    snackbar: SnackbarHostState,
    inAlbum: Album?,
    inPerson: Person?,
    onChanged: () -> Unit = {},
) {
    val scope = rememberCoroutineScope()
    var menu by remember { mutableStateOf(false) }
    var picking by remember { mutableStateOf(false) }
    var tagging by remember { mutableStateOf(false) }
    var someoneElse by remember { mutableStateOf(false) }
    val picked = selection.items.values.toList()
    val paths = picked.filterNot { it.missing }.map { it.path }
    fun act(done: String, block: suspend () -> Unit) {
        scope.launchCatching(onError = { scope.launch { snackbar.showSnackbar(it.userMessage()) } }) {
            block()
            selection.clear()
            onChanged()
            snackbar.showSnackbar(done)
        }
    }
    TopAppBar(
        title = { Text("${picked.size} selected") },
        navigationIcon = { IconButton(onClick = { selection.clear() }) { Icon(Icons.Filled.Close, contentDescription = "Clear selection") } },
        actions = {
            TextButton(onClick = { picking = true }, enabled = paths.isNotEmpty()) { Text("Add to album") }
            IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "More") }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                DropdownMenuItem(text = { Text("Tag…") }, onClick = { menu = false; tagging = true })
                if (picked.any { !it.hidden }) {
                    DropdownMenuItem(text = { Text("Hide from Photos") }, onClick = {
                        menu = false
                        act("Hidden from Photos (still in their folders and albums)") { vm.call(serverId) { it.hideFromTimeline(paths, true) } }
                    })
                }
                if (picked.any { it.hidden }) {
                    DropdownMenuItem(text = { Text("Show in Photos") }, onClick = {
                        menu = false
                        act("Back in Photos") { vm.call(serverId) { it.hideFromTimeline(picked.filter { it.hidden }.map { it.path }, false) } }
                    })
                }
                if (smart.search && picked.size == 1) {
                    DropdownMenuItem(text = { Text("Find similar photos") }, onClick = {
                        menu = false
                        val path = picked.first().path
                        selection.clear()
                        vm.push(Screen.GalleryResults(serverId, similarTo = path))
                    })
                }
                if (inAlbum != null) {
                    DropdownMenuItem(text = { Text("Remove from album") }, onClick = {
                        menu = false
                        act("Removed from “${inAlbum.name}”") { vm.call(serverId) { it.changeAlbum(inAlbum.id, picked.map { it.path }, false) } }
                    })
                }
                if (inPerson != null) {
                    val faces = picked.mapNotNull { it.faceId }
                    DropdownMenuItem(text = { Text("Not ${inPerson.name ?: "this person"}") }, onClick = {
                        menu = false
                        act("Taken out") { vm.call(serverId) { it.moveFaces(faces, null) } }
                    })
                    DropdownMenuItem(text = { Text("This is someone else…") }, onClick = { menu = false; someoneElse = true })
                }
            }
        },
    )
    if (picking) {
        AlbumPicker(vm, serverId, "Add ${paths.size} to…", onDismiss = { picking = false }) { album ->
            picking = false
            act("Added to the album") { vm.call(serverId) { it.changeAlbum(album, paths, true) } }
        }
    }
    if (tagging) {
        NameDialog("Tag ${paths.size} photo${if (paths.size == 1) "" else "s"}", "", "Tag", onDismiss = { tagging = false }, onConfirm = { tag ->
            tagging = false
            act("Tagged “$tag”") { vm.call(serverId) { it.tagPhotos(paths, tag, true) } }
        })
    }
    if (someoneElse && inPerson != null) {
        PersonPicker(vm, serverId, "Who is it?", exclude = inPerson.id, onDismiss = { someoneElse = false }) { to ->
            someoneElse = false
            act("Moved") { vm.call(serverId) { it.moveFaces(picked.mapNotNull { it.faceId }, to) } }
        }
    }
}

// ---- Photos: the timeline ----

private sealed interface Cell {
    data class MonthHead(val month: String) : Cell
    data class DayHead(val day: String) : Cell
    data class Slot(val month: String, val day: String, val index: Int) : Cell
}

@Composable
private fun Timeline(vm: AppViewModel, serverId: String, selection: Selection) {
    var days by remember { mutableStateOf<TimelineDays?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(serverId) {
        runCatching { vm.call(serverId) { it.galleryDays() } }.onSuccess { days = it }.onFailure { error = it.userMessage() }
    }
    val loaded = remember { mutableStateMapOf<String, Map<String, List<MediaItem>>>() }
    val cells = remember(days) {
        val out = mutableListOf<Cell>()
        var month = ""
        for (day in days?.days.orEmpty()) {
            val key = day.day.take(7)
            if (key != month) {
                month = key
                out += Cell.MonthHead(key)
            }
            out += Cell.DayHead(day.day)
            for (index in 0 until day.count.toInt()) out += Cell.Slot(key, day.day, index)
        }
        out
    }
    val monthStarts = remember(cells) { cells.withIndex().filter { it.value is Cell.MonthHead }.map { it.index to (it.value as Cell.MonthHead).month } }
    val grid = rememberLazyGridState()
    // Load the months on (and near) the screen.
    LaunchedEffect(cells) {
        snapshotFlow { grid.layoutInfo.visibleItemsInfo.map { it.index } }.collect { visible ->
            if (visible.isEmpty()) return@collect
            val from = (visible.first() - 80).coerceAtLeast(0)
            val to = (visible.last() + 80).coerceAtMost(cells.size - 1)
            val wanted = (from..to).mapNotNull { (cells[it] as? Cell.Slot)?.month }.distinct().filterNot { loaded.containsKey(it) }
            for (month in wanted) {
                val start = LocalDate.parse("$month-01")
                val fromSecs = start.atStartOfDay().toEpochSecond(ZoneOffset.UTC)
                val toSecs = start.plusMonths(1).atStartOfDay().toEpochSecond(ZoneOffset.UTC)
                val items = runCatching { vm.call(serverId) { it.galleryBetween(fromSecs, toSecs) } }.getOrNull() ?: continue
                loaded[month] = items.groupBy { it.day() ?: "" }
            }
        }
    }
    fun openAt(item: MediaItem) {
        val all = cells.mapNotNull { cell -> (cell as? Cell.Slot)?.let { loaded[it.month]?.get(it.day)?.getOrNull(it.index) } }
        vm.viewer(serverId, all, all.indexOfFirst { it.path == item.path }.coerceAtLeast(0))
    }
    when {
        error != null -> Message(error!!)
        days == null -> Message("Loading photos…")
        cells.isEmpty() -> Message("No photos or videos yet. Photos anywhere in the library show up here, newest first.")
        else -> Box(Modifier.fillMaxSize()) {
            LazyVerticalGrid(
                columns = GridCells.Adaptive(96.dp),
                state = grid,
                contentPadding = PaddingValues(start = 4.dp, end = 40.dp, bottom = 24.dp),
                horizontalArrangement = Arrangement.spacedBy(3.dp),
                verticalArrangement = Arrangement.spacedBy(3.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                items(cells.size, span = { index -> if (cells[index] is Cell.Slot) GridItemSpan(1) else GridItemSpan(maxLineSpan) }) { index ->
                    when (val cell = cells[index]) {
                        is Cell.MonthHead -> Text(monthLabel(cell.month), style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(start = 8.dp, top = 18.dp, bottom = 2.dp))
                        is Cell.DayHead -> Text(dayLabel(cell.day), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(start = 8.dp, top = 8.dp, bottom = 4.dp))
                        is Cell.Slot -> {
                            val item = loaded[cell.month]?.get(cell.day)?.getOrNull(cell.index)
                            if (item == null) {
                                Box(Modifier.aspectRatio(1f).background(MaterialTheme.colorScheme.surfaceContainer, RoundedCornerShape(4.dp)))
                            } else {
                                PhotoTile(
                                    vm, serverId, item,
                                    selected = selection.items.containsKey(item.path),
                                    selecting = selection.active,
                                    onClick = { if (selection.active) selection.toggle(item) else openAt(item) },
                                    onLongClick = { selection.toggle(item) },
                                )
                            }
                        }
                    }
                }
            }
            if (monthStarts.size > 1) Scrubber(grid, cells.size, monthStarts, Modifier.align(Alignment.CenterEnd))
        }
    }
}

/** Years down the right edge: drag or tap to jump through the timeline, as in Apple Photos. */
@Composable
private fun Scrubber(grid: androidx.compose.foundation.lazy.grid.LazyGridState, total: Int, monthStarts: List<Pair<Int, String>>, modifier: Modifier) {
    val scope = rememberCoroutineScope()
    var bubble by remember { mutableStateOf<Pair<Float, String>?>(null) }
    BoxWithConstraints(modifier.fillMaxHeight().width(40.dp).padding(vertical = 8.dp)) {
        val height = constraints.maxHeight.toFloat().coerceAtLeast(1f)
        fun jump(y: Float) {
            val fraction = (y / height).coerceIn(0f, 1f)
            val target = (fraction * total).toInt()
            val (index, month) = monthStarts.lastOrNull { it.first <= target } ?: monthStarts.first()
            bubble = y to monthLabel(month)
            scope.launch { grid.scrollToItem(index) }
        }
        Box(
            Modifier.fillMaxSize()
                .pointerInput(total) {
                    detectVerticalDragGestures(onDragEnd = { bubble = null }, onDragCancel = { bubble = null }) { change, _ -> jump(change.position.y) }
                }
                .pointerInput(total) { detectTapGestures { offset -> jump(offset.y); bubble = null } },
        ) {
            var lastYear = ""
            var lastY = -100f
            val density = LocalDensity.current
            for ((index, month) in monthStarts) {
                val year = month.take(4)
                val y = index.toFloat() / total * height
                if (year != lastYear && y - lastY > with(density) { 16.dp.toPx() }) {
                    Text(
                        year,
                        fontSize = 10.sp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.align(Alignment.TopEnd).offset { IntOffset(0, y.roundToInt()) }.padding(end = 4.dp),
                    )
                    lastY = y
                }
                lastYear = year
            }
        }
        bubble?.let { (y, label) ->
            Surface(
                color = MaterialTheme.colorScheme.primary,
                shape = RoundedCornerShape(50),
                modifier = Modifier.align(Alignment.TopEnd).offset { IntOffset(-130, (y - 20).roundToInt()) },
            ) { Text(label, color = MaterialTheme.colorScheme.onPrimary, modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp)) }
        }
    }
}

// ---- Albums ----

@Composable
private fun AlbumsTab(vm: AppViewModel, serverId: String) {
    var page by remember { mutableStateOf<AlbumPage?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var creating by remember { mutableStateOf(false) }
    var reload by remember { mutableIntStateOf(0) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(reload) { runCatching { vm.call(serverId) { it.album(null) } }.onSuccess { page = it }.onFailure { error = it.userMessage() } }
    Box(Modifier.fillMaxSize()) {
        val albums = page?.albums.orEmpty()
        when {
            error != null -> Message(error!!)
            page == null -> Message("Loading albums…")
            albums.isEmpty() -> Message("No albums yet. Albums gather photos from anywhere in the library without moving them, and can hold other albums, like folders.")
            else -> LazyVerticalGrid(GridCells.Adaptive(150.dp), contentPadding = PaddingValues(8.dp, 8.dp, 8.dp, 88.dp)) {
                items(albums, key = { it.id }) { album -> AlbumCard(vm, serverId, album) { vm.push(Screen.GalleryAlbum(serverId, album.id)) } }
            }
        }
        ExtendedFloatingActionButton(
            onClick = { creating = true },
            icon = { Icon(Icons.Filled.Add, null) },
            text = { Text("New album") },
            modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp),
        )
    }
    if (creating) {
        NameDialog("New album", "", "Create", onDismiss = { creating = false }, onConfirm = { name ->
            creating = false
            scope.launchCatching(onError = { error = it.userMessage() }) {
                val album = vm.call(serverId) { it.createAlbum(name, null) }
                reload++
                vm.push(Screen.GalleryAlbum(serverId, album.id))
            }
        })
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun GalleryAlbumScreen(vm: AppViewModel, serverId: String, albumId: Long) {
    var page by remember { mutableStateOf<AlbumPage?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    var dialog by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(reload) {
        runCatching { vm.call(serverId) { it.album(albumId) } }.onSuccess { page = it }.onFailure { snackbar.showSnackbar(it.userMessage()) }
    }
    val album = page?.album
    PhotoGridPage(
        vm, serverId,
        title = album?.name ?: "Album",
        subtitle = page?.breadcrumb?.dropLast(1)?.joinToString(" › ") { it.name }?.takeIf { it.isNotEmpty() },
        items = page?.items,
        empty = "No photos in this album yet. Long-press photos in the Gallery and choose Add to album.",
        snackbar = snackbar,
        inAlbum = album,
        onChanged = { reload++ },
        menu = listOf(
            "New album inside" to { dialog = "new" },
            "Rename" to { dialog = "rename" },
            "Delete album" to { dialog = "delete" },
        ),
        header = {
            val inside = page?.albums.orEmpty()
            if (inside.isNotEmpty()) {
                items(inside, key = { "album-${it.id}" }, span = { GridItemSpan(2) }) { child ->
                    AlbumCard(vm, serverId, child) { vm.push(Screen.GalleryAlbum(serverId, child.id)) }
                }
                item(span = { GridItemSpan(maxLineSpan) }) { Text("Photos", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(8.dp)) }
            }
        },
    )
    val current = album ?: return
    when (dialog) {
        "new" -> NameDialog("New album inside “${current.name}”", "", "Create", onDismiss = { dialog = null }, onConfirm = { name ->
            dialog = null
            scope.launchCatching(onError = { scope.launch { snackbar.showSnackbar(it.userMessage()) } }) {
                val created = vm.call(serverId) { it.createAlbum(name, current.id) }
                reload++
                vm.push(Screen.GalleryAlbum(serverId, created.id))
            }
        })
        "rename" -> NameDialog("Rename album", current.name, "Rename", onDismiss = { dialog = null }, onConfirm = { name ->
            dialog = null
            scope.launchCatching(onError = { scope.launch { snackbar.showSnackbar(it.userMessage()) } }) {
                vm.call(serverId) { it.renameAlbum(current.id, name) }
                reload++
            }
        })
        "delete" -> AlertDialog(
            onDismissRequest = { dialog = null },
            title = { Text("Delete “${current.name}”?") },
            text = { Text("The album${if (current.albums > 0) " and the albums inside it" else ""} goes; the photos stay where they are.") },
            confirmButton = {
                TextButton(onClick = {
                    dialog = null
                    scope.launchCatching(onError = { scope.launch { snackbar.showSnackbar(it.userMessage()) } }) {
                        vm.call(serverId) { it.deleteAlbum(current.id) }
                        vm.pop()
                    }
                }) { Text("Delete album") }
            },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text("Cancel") } },
        )
    }
}

// ---- People ----

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun PeopleTab(vm: AppViewModel, serverId: String) {
    var people by remember { mutableStateOf<List<Person>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(serverId) { runCatching { vm.call(serverId) { it.people(false) } }.onSuccess { people = it }.onFailure { error = it.userMessage() } }
    val list = people.orEmpty()
    when {
        error != null -> Message(error!!)
        people == null -> Message("Loading people…")
        list.isEmpty() -> Message("No people yet. Someone shows up here once they are in two photos.")
        else -> LazyVerticalGrid(GridCells.Adaptive(104.dp), contentPadding = PaddingValues(12.dp)) {
            items(list, key = { it.id }) { person ->
                Column(
                    Modifier.clickable { vm.push(Screen.GalleryPerson(serverId, person.id)) }.padding(6.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    FaceCircle(vm, serverId, person.coverFace, 88.dp)
                    Text(
                        person.name ?: "Add a name",
                        style = MaterialTheme.typography.labelLarge,
                        color = if (person.name == null) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.onSurface,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text("${person.photos} photos", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

@Composable
fun GalleryPersonScreen(vm: AppViewModel, serverId: String, personId: Long) {
    var person by remember { mutableStateOf<Person?>(null) }
    var items by remember { mutableStateOf<List<MediaItem>?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    var dialog by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(reload) {
        runCatching { vm.call(serverId) { it.person(personId) } }
            .onSuccess { person = it.person; items = it.items }
            .onFailure { snackbar.showSnackbar(it.userMessage()) }
    }
    val current = person
    fun change(name: String? = null, hidden: Boolean? = null, into: Long? = null) {
        scope.launchCatching(onError = { scope.launch { snackbar.showSnackbar(it.userMessage()) } }) {
            vm.call(serverId) { it.changePerson(personId, name, hidden, into) }
            if (into != null || hidden == true) vm.pop() else reload++
        }
    }
    PhotoGridPage(
        vm, serverId,
        title = current?.name ?: "Unnamed person",
        subtitle = null,
        items = items,
        empty = "No photos left for this person.",
        snackbar = snackbar,
        inPerson = current,
        onChanged = { reload++ },
        menu = listOf(
            (if (current?.name == null) "Add a name" else "Rename") to { dialog = "rename" },
            "Same person as…" to { dialog = "merge" },
            (if (current?.hidden == true) "Show in People" else "Hide from People") to { change(hidden = current?.hidden != true) },
        ),
    )
    when (dialog) {
        "rename" -> NameDialog("Who is this?", current?.name.orEmpty(), "Save", onDismiss = { dialog = null }, onConfirm = { dialog = null; change(name = it) })
        "merge" -> PersonPicker(vm, serverId, "${current?.name ?: "This person"} is the same as…", exclude = personId, onDismiss = { dialog = null }) { into ->
            dialog = null
            change(into = into)
        }
    }
}

// ---- Tags ----

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun TagsTab(vm: AppViewModel, serverId: String) {
    var tags by remember { mutableStateOf<List<TagCount>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(serverId) { runCatching { vm.call(serverId) { it.tags() } }.onSuccess { tags = it }.onFailure { error = it.userMessage() } }
    when {
        error != null -> Message(error!!)
        tags == null -> Message("Loading tags…")
        tags!!.isEmpty() -> Message("No tags yet. Long-press photos and choose Tag… from the menu.")
        else -> FlowRow(Modifier.padding(12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for (tag in tags!!) {
                AssistChip(onClick = { vm.push(Screen.GalleryTag(serverId, tag.tag)) }, label = { Text("${tag.tag}  ${tag.count}") })
            }
        }
    }
}

@Composable
fun GalleryTagScreen(vm: AppViewModel, serverId: String, tag: String) =
    LoadedPhotos(vm, serverId, title = tag, empty = "No photos or videos have this tag.") { it.tagged(tag) }

@Composable
fun GalleryResultsScreen(vm: AppViewModel, serverId: String, words: String?, similarTo: String?) =
    LoadedPhotos(
        vm, serverId,
        title = words?.let { "“$it”" } ?: "Similar photos",
        subtitle = similarTo?.substringAfterLast('/')?.let { "Like $it" },
        empty = if (words != null) "Nothing matches. Try other words, in English: “red car”, “people at a table”." else "No photo looks much like this one.",
    ) { server -> if (words != null) server.searchPhotos(words) else server.similarPhotos(similarTo.orEmpty()) }

@Composable
fun GalleryHiddenScreen(vm: AppViewModel, serverId: String) =
    LoadedPhotos(vm, serverId, title = "Hidden photos", subtitle = "Left out of Photos; still in their folders and albums", empty = "Nothing is hidden.") { it.hiddenPhotos() }

@Composable
private fun LoadedPhotos(
    vm: AppViewModel,
    serverId: String,
    title: String,
    subtitle: String? = null,
    empty: String,
    load: (app.ownnas.core.Server) -> List<MediaItem>,
) {
    var items by remember { mutableStateOf<List<MediaItem>?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(reload) {
        runCatching { vm.call(serverId, load) }.onSuccess { items = it }.onFailure { items = emptyList(); snackbar.showSnackbar(it.userMessage()) }
    }
    PhotoGridPage(vm, serverId, title, subtitle, items, empty, snackbar, onChanged = { reload++ })
}

/** A titled page of photos with selection and its actions. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PhotoGridPage(
    vm: AppViewModel,
    serverId: String,
    title: String,
    subtitle: String?,
    items: List<MediaItem>?,
    empty: String,
    snackbar: SnackbarHostState,
    inAlbum: Album? = null,
    inPerson: Person? = null,
    onChanged: () -> Unit,
    menu: List<Pair<String, () -> Unit>> = emptyList(),
    header: LazyGridScope.() -> Unit = {},
) {
    val selection = remember { Selection() }
    var smart by remember { mutableStateOf(SmartFeatures(search = false, faces = false)) }
    LaunchedEffect(serverId) { runCatching { vm.call(serverId) { it.smartFeatures() } }.onSuccess { smart = it } }
    var open by remember { mutableStateOf(false) }
    Scaffold(
        topBar = {
            if (selection.active) {
                SelectionBar(vm, serverId, selection, smart, snackbar, inAlbum, inPerson, onChanged)
            } else {
                TopAppBar(
                    title = {
                        Column {
                            Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            subtitle?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1) }
                        }
                    },
                    navigationIcon = { IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") } },
                    actions = {
                        if (menu.isNotEmpty()) {
                            IconButton(onClick = { open = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "More") }
                            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                                for ((label, action) in menu) DropdownMenuItem(text = { Text(label) }, onClick = { open = false; action() })
                            }
                        }
                    },
                )
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            val list = items
            if (list == null) {
                Message("Loading…")
            } else {
                LazyVerticalGrid(
                    columns = GridCells.Adaptive(96.dp),
                    contentPadding = PaddingValues(4.dp),
                    horizontalArrangement = Arrangement.spacedBy(3.dp),
                    verticalArrangement = Arrangement.spacedBy(3.dp),
                    modifier = Modifier.fillMaxSize(),
                ) {
                    header()
                    if (list.isEmpty()) {
                        item(span = { GridItemSpan(maxLineSpan) }) { Message(empty) }
                    }
                    items(list, key = { it.path }) { item ->
                        PhotoTile(
                            vm, serverId, item,
                            selected = selection.items.containsKey(item.path),
                            selecting = selection.active,
                            onClick = {
                                if (selection.active) selection.toggle(item)
                                else if (!item.missing) vm.viewer(serverId, list.filterNot { it.missing }, list.filterNot { it.missing }.indexOf(item).coerceAtLeast(0))
                            },
                            onLongClick = { selection.toggle(item) },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun Message(text: String) {
    Box(Modifier.fillMaxWidth().padding(32.dp), contentAlignment = Alignment.Center) {
        Text(text, textAlign = TextAlign.Center, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

