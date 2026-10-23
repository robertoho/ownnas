package app.ownnas.android.ui.gallery

import android.content.Context
import android.location.Geocoder
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
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
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.ownnas.android.ui.AppViewModel
import app.ownnas.android.ui.Screen
import app.ownnas.android.ui.userMessage
import app.ownnas.core.MemoryYear
import app.ownnas.core.Place
import app.ownnas.core.Workspace
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.time.LocalDate
import java.util.Locale

// ---- Workspaces: the Gallery's title chooses which one shows ----

/** The Gallery's title: the workspace showing, and a menu to show another (or every photo). */
@Composable
fun WorkspaceTitle(workspaces: List<Workspace>, current: Long?, onChoose: (Long?) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val showing = workspaces.firstOrNull { it.id == current }
    Box {
        Row(
            Modifier.clickable(enabled = workspaces.isNotEmpty()) { open = true }.padding(vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(showing?.name ?: "Gallery", maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (workspaces.isNotEmpty()) Icon(Icons.Filled.ArrowDropDown, contentDescription = "Choose a workspace")
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            WorkspaceItem("All photos", null, current == null) { open = false; onChoose(null) }
            HorizontalDivider()
            for (workspace in workspaces) {
                WorkspaceItem(workspace.name, workspace.photos, current == workspace.id) { open = false; onChoose(workspace.id) }
            }
        }
    }
}

@Composable
private fun WorkspaceItem(name: String, photos: Long?, chosen: Boolean, onClick: () -> Unit) {
    DropdownMenuItem(
        text = { Text(name) },
        leadingIcon = { if (chosen) Icon(Icons.Filled.Check, contentDescription = "Showing") else Box(Modifier.width(24.dp)) },
        trailingIcon = photos?.let { { Text("$it", color = MaterialTheme.colorScheme.onSurfaceVariant) } },
        onClick = onClick,
    )
}

// ---- On this day: a row above the timeline ----

/** Photos taken on this day in earlier years, one card a year; empty when there are none. */
@Composable
fun OnThisDay(vm: AppViewModel, serverId: String, years: List<MemoryYear>) {
    val thisYear = LocalDate.now().year
    Column(Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 4.dp)) {
        Text("On this day", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(start = 8.dp, bottom = 6.dp))
        LazyRow(contentPadding = PaddingValues(horizontal = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            items(years, key = { it.year }) { year ->
                val ago = thisYear - year.year
                PictureCard(
                    vm, serverId,
                    path = year.items.firstOrNull()?.path,
                    title = if (ago == 1) "1 year ago" else "$ago years ago",
                    subtitle = "${year.year} · ${year.count} photo${if (year.count == 1L) "" else "s"}",
                    modifier = Modifier.width(124.dp),
                ) { vm.viewer(serverId, year.items, 0) }
            }
        }
    }
}

/** A photo with a title over its bottom edge, for On this day and Places. */
@Composable
private fun PictureCard(
    vm: AppViewModel,
    serverId: String,
    path: String?,
    title: String,
    subtitle: String,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Box(modifier.aspectRatio(0.8f).clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.surfaceContainerHighest).clickable(onClick = onClick)) {
        if (path != null) {
            val key = "card:$serverId:$path"
            val image by produceState(vm.cachedThumbnail(key), key) { if (value == null) value = vm.thumbnailOf(serverId, path, key) }
            image?.let { Image(it, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize()) }
        }
        Column(
            Modifier.align(Alignment.BottomStart).fillMaxWidth()
                .background(Brush.verticalGradient(listOf(Color.Transparent, Color(0xCC000000))))
                .padding(start = 10.dp, end = 10.dp, top = 24.dp, bottom = 8.dp),
        ) {
            Text(title, color = Color.White, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Text(subtitle, color = Color(0xDDFFFFFF), style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

// ---- Places: where photos were taken ----

/** How finely photos are gathered into places: about a city and its surroundings. */
private const val PLACE_ZOOM: UByte = 7u

@Composable
fun PlacesTab(vm: AppViewModel, serverId: String) {
    val context = LocalContext.current
    var places by remember { mutableStateOf<List<Place>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(serverId) {
        runCatching { vm.call(serverId) { it.places(PLACE_ZOOM, null) } }
            .onSuccess { places = it.places }
            .onFailure { error = it.userMessage() }
    }
    val list = places.orEmpty()
    when {
        error != null -> GalleryMessage(error!!)
        places == null -> GalleryMessage("Loading places…")
        list.isEmpty() -> GalleryMessage("No places yet. Photos taken with the camera's location on show up here, by where they were taken.")
        else -> LazyVerticalGrid(GridCells.Adaptive(150.dp), contentPadding = PaddingValues(8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    "${list.sumOf { it.count.toLong() }} photos in ${list.size} place${if (list.size == 1) "" else "s"}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(4.dp),
                )
            }
            items(list, key = { "${it.area.west},${it.area.north}" }) { place ->
                val name by produceState(coordinates(place.lat, place.lon), place.lat, place.lon) { placeName(context, place.lat, place.lon)?.let { value = it } }
                PictureCard(vm, serverId, place.newest.path, name, "${place.count} photo${if (place.count == 1uL) "" else "s"}") {
                    vm.push(Screen.GalleryPlace(serverId, name, place.area.west, place.area.south, place.area.east, place.area.north))
                }
            }
        }
    }
}

private fun coordinates(lat: Double, lon: Double) =
    String.format(Locale.ROOT, "%.2f°%s, %.2f°%s", kotlin.math.abs(lat), if (lat >= 0) "N" else "S", kotlin.math.abs(lon), if (lon >= 0) "E" else "W")

private val placeNames = HashMap<Pair<Double, Double>, String>()

/** The town and country at a point, from the phone's geocoder (`null` without one, or offline). */
private suspend fun placeName(context: Context, lat: Double, lon: Double): String? {
    synchronized(placeNames) { placeNames[lat to lon] }?.let { return it }
    if (!Geocoder.isPresent()) return null
    return withContext(Dispatchers.IO) {
        runCatching {
            @Suppress("DEPRECATION")
            val found = Geocoder(context).getFromLocation(lat, lon, 1)?.firstOrNull() ?: return@runCatching null
            listOfNotNull(found.locality ?: found.subAdminArea ?: found.adminArea, found.countryName).distinct().joinToString(", ").ifEmpty { null }
        }.getOrNull()
    }?.also { synchronized(placeNames) { placeNames[lat to lon] = it } }
}

@Composable
fun GalleryMessage(text: String) {
    Box(Modifier.fillMaxWidth().padding(32.dp), contentAlignment = Alignment.Center) {
        Text(text, textAlign = androidx.compose.ui.text.style.TextAlign.Center, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
