package app.ownnas.android.ui.gallery

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.outlined.Person
import androidx.compose.material.icons.outlined.PhotoLibrary
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.ownnas.android.ui.AppViewModel
import app.ownnas.android.ui.userMessage
import app.ownnas.core.Album
import app.ownnas.core.MediaItem
import app.ownnas.core.Person
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

/** The file name at the end of a library path. */
val MediaItem.name: String get() = path.substringAfterLast('/')

private val dayFormat = DateTimeFormatter.ofPattern("EEE, d MMM yyyy")
private val monthFormat = DateTimeFormatter.ofPattern("LLLL yyyy")

/** "2024-07-15" as "Mon, 15 Jul 2024". */
fun dayLabel(day: String): String = runCatching { LocalDate.parse(day).format(dayFormat) }.getOrDefault(day)

/** "2024-07" as "July 2024". */
fun monthLabel(month: String): String = runCatching { LocalDate.parse("$month-01").format(monthFormat) }.getOrDefault(month)

/** The UTC day a photo belongs to in the timeline ("2024-07-15"). */
fun MediaItem.day(): String? = takenAt?.let { Instant.ofEpochSecond(it).atOffset(ZoneOffset.UTC).toLocalDate().toString() }

/** When it was taken, for showing: camera dates are local times stored as UTC. */
fun MediaItem.takenLabel(): String? {
    val at = takenAt ?: return null
    val zone = if (dateSource == "exif") ZoneOffset.UTC else ZoneId.systemDefault()
    return Instant.ofEpochSecond(at).atZone(zone).format(DateTimeFormatter.ofLocalizedDateTime(FormatStyle.MEDIUM, FormatStyle.SHORT))
}

private fun MediaItem.thumbKey(serverId: String) = "g:$serverId:$path:${takenAt ?: 0}:${size ?: 0}"

/** One square photo: its thumbnail, a play mark for videos, and a tick when selected. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun PhotoTile(
    vm: AppViewModel,
    serverId: String,
    item: MediaItem,
    selected: Boolean,
    selecting: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    showHidden: Boolean = true,
) {
    val key = item.thumbKey(serverId)
    val image by produceState(vm.cachedThumbnail(key), key) {
        if (value == null && !item.missing) value = vm.thumbnailOf(serverId, item.path, key)
    }
    Box(
        Modifier
            .aspectRatio(1f)
            .clip(RoundedCornerShape(4.dp))
            .background(MaterialTheme.colorScheme.surfaceContainerHighest)
            .combinedClickable(onClick = onClick, onLongClick = onLongClick),
    ) {
        val picture = image
        if (picture != null) {
            Image(
                bitmap = picture,
                contentDescription = item.name,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize().padding(if (selected) 10.dp else 0.dp).clip(RoundedCornerShape(if (selected) 6.dp else 0.dp)),
            )
        } else {
            Text(
                if (item.missing) "Missing" else item.name,
                style = MaterialTheme.typography.labelSmall,
                color = if (item.missing) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 3,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.align(Alignment.Center).padding(6.dp),
            )
        }
        if (item.kind == "video") {
            Icon(
                Icons.Filled.PlayArrow,
                contentDescription = "Video",
                tint = Color.White,
                modifier = Modifier.align(Alignment.BottomEnd).padding(4.dp).size(20.dp).background(Color(0x99000000), CircleShape),
            )
        }
        if (showHidden && item.hidden) {
            Icon(
                Icons.Outlined.VisibilityOff,
                contentDescription = "Hidden from Photos",
                tint = Color.White,
                modifier = Modifier.align(Alignment.BottomStart).padding(4.dp).size(20.dp).background(Color(0x99000000), CircleShape).padding(2.dp),
            )
        }
        if (selecting) {
            Icon(
                Icons.Filled.CheckCircle,
                contentDescription = if (selected) "Selected" else "Not selected",
                tint = if (selected) MaterialTheme.colorScheme.primary else Color(0xCCFFFFFF),
                modifier = Modifier.align(Alignment.TopStart).padding(4.dp).size(22.dp)
                    .background(if (selected) Color.White else Color(0x55000000), CircleShape),
            )
        }
    }
}

/** A person's face in a circle (or a blank one). */
@Composable
fun FaceCircle(vm: AppViewModel, serverId: String, faceId: Long?, size: androidx.compose.ui.unit.Dp) {
    val face by produceState<androidx.compose.ui.graphics.ImageBitmap?>(null, faceId) {
        if (faceId != null) value = vm.facePicture(serverId, faceId)
    }
    Box(
        Modifier.size(size).clip(CircleShape).background(MaterialTheme.colorScheme.surfaceContainerHighest)
            .border(2.dp, MaterialTheme.colorScheme.outlineVariant, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        val picture = face
        if (picture != null) {
            Image(picture, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        } else {
            Icon(Icons.Outlined.Person, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** An album's cover and name, as a card. */
@Composable
fun AlbumCard(vm: AppViewModel, serverId: String, album: Album, onClick: () -> Unit) {
    Column(Modifier.clickable(onClick = onClick).padding(4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        val cover = album.cover
        Box(
            Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.surfaceContainerHighest),
            contentAlignment = Alignment.Center,
        ) {
            if (cover != null) {
                val key = "cover:$serverId:$cover"
                val image by produceState(vm.cachedThumbnail(key), key) { if (value == null) value = vm.thumbnailOf(serverId, cover, key) }
                image?.let { Image(it, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize()) }
            } else {
                Icon(Icons.Outlined.PhotoLibrary, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(40.dp))
            }
        }
        Text(album.name, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(
            listOfNotNull(
                album.count.takeIf { it > 0 }?.let { "$it photo${if (it == 1L) "" else "s"}" },
                album.albums.takeIf { it > 0 }?.let { "$it album${if (it == 1L) "" else "s"}" },
            ).joinToString(" · ").ifEmpty { "Empty" },
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** Choosing an album to add photos to, with a way to make a new one. */
@Composable
fun AlbumPicker(vm: AppViewModel, serverId: String, title: String, onDismiss: () -> Unit, onPick: (Long) -> Unit) {
    var albums by remember { mutableStateOf<List<Album>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var filter by remember { mutableStateOf("") }
    var creating by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(Unit) {
        runCatching { vm.call(serverId) { it.allAlbums() } }.onSuccess { albums = it }.onFailure { error = it.userMessage() }
    }
    // Indented by depth, in tree order.
    val rows = remember(albums, filter) {
        val all = albums.orEmpty()
        val byParent = all.groupBy { it.parentId }
        val out = mutableListOf<Pair<Album, Int>>()
        fun walk(parent: Long?, depth: Int) {
            for (album in byParent[parent].orEmpty().sortedBy { it.name.lowercase() }) {
                out += album to depth
                walk(album.id, depth + 1)
            }
        }
        walk(null, 0)
        if (filter.isBlank()) out else out.filter { it.first.name.contains(filter.trim(), ignoreCase = true) }.map { it.first to 0 }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(filter, { filter = it }, singleLine = true, label = { Text("Find an album") }, modifier = Modifier.fillMaxWidth())
                when {
                    error != null -> Text(error!!, color = MaterialTheme.colorScheme.error)
                    albums == null -> Text("Loading…")
                    rows.isEmpty() -> Text("No albums yet. Make one with New album.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    else -> LazyColumn(Modifier.heightIn(max = 340.dp)) {
                        items(rows, key = { it.first.id }) { (album, depth) ->
                            Row(
                                Modifier.fillMaxWidth().clickable { onPick(album.id) }.padding(start = (8 + depth * 18).dp, top = 10.dp, bottom = 10.dp, end = 8.dp),
                                horizontalArrangement = Arrangement.SpaceBetween,
                            ) {
                                Text(album.name, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                                if (album.count > 0) Text("${album.count}", color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = { creating = true }) { Text("New album") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
    if (creating) {
        app.ownnas.android.ui.NameDialog("New album", "", "Create", onDismiss = { creating = false }, onConfirm = { name ->
            creating = false
            scope.launchCatching(onError = { error = it.userMessage() }) {
                val album = vm.call(serverId) { it.createAlbum(name, null) }
                onPick(album.id)
            }
        })
    }
}

/** Choosing a person (for "this is someone else" and "same person as"). */
@Composable
fun PersonPicker(vm: AppViewModel, serverId: String, title: String, exclude: Long?, onDismiss: () -> Unit, onPick: (Long) -> Unit) {
    var people by remember { mutableStateOf<List<Person>?>(null) }
    LaunchedEffect(Unit) { people = runCatching { vm.call(serverId) { it.people(false) } }.getOrDefault(emptyList()) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            val list = people.orEmpty().filter { it.id != exclude }
            if (people == null) {
                Text("Loading…")
            } else if (list.isEmpty()) {
                Text("Nobody else yet.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            } else {
                LazyColumn(Modifier.heightIn(max = 360.dp)) {
                    items(list, key = { it.id }) { person ->
                        Row(
                            Modifier.fillMaxWidth().clickable { onPick(person.id) }.padding(8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            FaceCircle(vm, serverId, person.coverFace, 44.dp)
                            Text(person.name ?: "Unnamed · ${person.photos} photos")
                        }
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Runs `block`, sending any failure to `onError`. */
fun CoroutineScope.launchCatching(onError: (Throwable) -> Unit, block: suspend () -> Unit) = launch {
    try {
        block()
    } catch (err: CancellationException) {
        throw err
    } catch (err: Throwable) {
        onError(err)
    }
}
