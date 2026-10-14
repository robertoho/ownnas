package app.ownnas.android.ui.gallery

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Label
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Map
import androidx.compose.material.icons.filled.PhotoAlbum
import androidx.compose.material3.AssistChip
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import app.ownnas.android.ui.AppViewModel
import app.ownnas.android.ui.Screen
import app.ownnas.android.ui.userMessage
import app.ownnas.core.DetailRow
import app.ownnas.core.MediaItem
import app.ownnas.core.PhotoDetails
import java.text.DateFormat
import java.util.Date
import java.util.Locale

/** The long lists, folded until asked for. */
private val FOLDED = setOf("All EXIF metadata", "All video metadata")

/** Everything known about a photo, in a sheet pulled up from the viewer. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun PhotoDetailsSheet(vm: AppViewModel, serverId: String, item: MediaItem, onDismiss: () -> Unit) {
    val sheet = rememberModalBottomSheetState()
    val context = LocalContext.current
    val loaded by produceState<Result<PhotoDetails>?>(null, item.path) {
        value = null
        value = runCatching { vm.call(serverId) { it.photoDetails(item.path) } }
    }
    val open = remember { mutableStateMapOf<String, Boolean>() }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheet) {
        val details = loaded?.getOrNull()
        when {
            loaded == null -> Box(Modifier.fillMaxWidth().padding(32.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            details == null -> Text(
                loaded?.exceptionOrNull()?.userMessage() ?: "Could not load the details",
                modifier = Modifier.padding(24.dp),
                color = MaterialTheme.colorScheme.error,
            )
            else -> LazyColumn(contentPadding = PaddingValues(bottom = 32.dp)) {
                item {
                    Column(Modifier.padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(details.name, style = MaterialTheme.typography.titleLarge)
                        item.takenLabel()?.let { Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        if (details.albums.isNotEmpty() || details.tags.isNotEmpty()) {
                            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                for (album in details.albums) {
                                    AssistChip(
                                        onClick = { onDismiss(); vm.push(Screen.GalleryAlbum(serverId, album.id)) },
                                        label = { Text(album.name) },
                                        leadingIcon = { Icon(Icons.Filled.PhotoAlbum, contentDescription = null, Modifier.size(18.dp)) },
                                    )
                                }
                                for (tag in details.tags) {
                                    AssistChip(
                                        onClick = { onDismiss(); vm.push(Screen.GalleryTag(serverId, tag)) },
                                        label = { Text(tag) },
                                        leadingIcon = { Icon(Icons.AutoMirrored.Filled.Label, contentDescription = null, Modifier.size(18.dp)) },
                                    )
                                }
                            }
                        }
                        details.location?.let { place ->
                            FilledTonalButton(onClick = {
                                val label = Uri.encode(details.name)
                                val uri = Uri.parse("geo:${place.lat},${place.lon}?q=${place.lat},${place.lon}($label)")
                                runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, uri)) }
                            }) {
                                Icon(Icons.Filled.Map, contentDescription = null)
                                Text(String.format(Locale.US, "  Open in Maps (%.5f, %.5f)", place.lat, place.lon))
                            }
                        }
                    }
                }
                for (section in details.sections) {
                    val folded = section.title in FOLDED && open[section.title] != true
                    item(key = "title:${section.title}") {
                        HorizontalDivider(Modifier.padding(top = 12.dp))
                        Row(
                            Modifier.fillMaxWidth()
                                .then(if (section.title in FOLDED) Modifier.clickable { open[section.title] = folded } else Modifier)
                                .padding(horizontal = 20.dp, vertical = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(section.title, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary, modifier = Modifier.weight(1f))
                            if (section.title in FOLDED) {
                                Text("${section.rows.size}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                Icon(if (folded) Icons.Filled.ExpandMore else Icons.Filled.ExpandLess, contentDescription = if (folded) "Show" else "Hide")
                            }
                        }
                    }
                    if (!folded) {
                        items(section.rows, key = { "row:${section.title}:${it.label}:${it.value.hashCode()}" }) { row ->
                            DetailLine(row, monospace = section.title in FOLDED || row.label == "SHA-256")
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun DetailLine(row: DetailRow, monospace: Boolean) {
    val value = row.time?.let { DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.MEDIUM).format(Date(it * 1000)) } ?: row.value
    Row(Modifier.fillMaxWidth().heightIn(min = 32.dp).padding(horizontal = 20.dp, vertical = 3.dp)) {
        Text(
            row.label,
            modifier = Modifier.weight(0.42f).padding(end = 12.dp),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        // SelectionContainer drops the row weight, so a box holds the value's share.
        Box(Modifier.weight(0.58f)) {
            SelectionContainer {
                Text(
                    value,
                    style = if (monospace) MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace) else MaterialTheme.typography.bodyMedium,
                    textAlign = TextAlign.Start,
                )
            }
        }
    }
}
