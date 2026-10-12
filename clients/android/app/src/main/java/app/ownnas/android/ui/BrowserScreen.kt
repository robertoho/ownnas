package app.ownnas.android.ui

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.text.format.DateUtils
import android.text.format.Formatter
import android.webkit.MimeTypeMap
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.outlined.Audiotrack
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.FolderZip
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.Movie
import androidx.compose.material.icons.outlined.PictureAsPdf
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
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
import androidx.core.content.FileProvider
import app.ownnas.core.ClientException
import app.ownnas.core.Entry
import app.ownnas.core.Folder
import kotlinx.coroutines.launch
import java.io.File

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
    var reload by remember { mutableStateOf(0) }
    var opening by remember { mutableStateOf<Entry?>(null) }

    LaunchedEffect(reload) {
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

    fun open(entry: Entry) {
        if (entry.dir) {
            vm.push(Screen.Browse(serverId, entry.path))
            return
        }
        opening = entry
        scope.launch {
            try {
                val file = vm.download(serverId, entry)
                if (opening == entry) openWithAnotherApp(context, file)
            } catch (err: ActivityNotFoundException) {
                snackbar.showSnackbar("No app on this phone can open ${entry.name}")
            } catch (err: Exception) {
                snackbar.showSnackbar(err.userMessage())
            } finally {
                opening = null
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(
                            path.substringAfterLast('/').ifEmpty { server.name },
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        if (path.isNotEmpty()) {
                            Text(
                                "${server.name}/$path",
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
            )
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
                failure != null && shown == null -> Problem(
                    failure,
                    onRetry = { reload++ },
                    onSignIn = { vm.push(Screen.SignIn(server)) },
                )
                shown == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
                shown.entries.isEmpty() -> Message("This folder is empty")
                else -> Listing(vm, serverId, shown, onOpen = ::open)
            }
        }
    }

    opening?.let { entry ->
        AlertDialog(
            onDismissRequest = { opening = null },
            icon = { CircularProgressIndicator(modifier = Modifier.size(32.dp)) },
            title = { Text("Opening ${entry.name}") },
            text = { Text(Formatter.formatShortFileSize(context, entry.size.toLong())) },
            confirmButton = {},
        )
    }
}

@Composable
private fun Listing(vm: AppViewModel, serverId: String, folder: Folder, onOpen: (Entry) -> Unit) {
    val context = LocalContext.current
    val entries = remember(folder) {
        folder.entries.sortedWith(compareBy<Entry> { !it.dir }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name })
    }
    LazyColumn(Modifier.fillMaxSize()) {
        items(entries, key = { it.path }) { entry ->
            ListItem(
                modifier = Modifier.clickable { onOpen(entry) },
                leadingContent = { Thumbnail(vm, serverId, entry) },
                headlineContent = { Text(entry.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                supportingContent = { Text(details(context, entry), maxLines = 1) },
            )
        }
        if (folder.truncated) {
            item { Message("Showing the first ${folder.entries.size} items. Open this folder in the browser to see the rest.") }
        }
    }
}

@Composable
private fun Thumbnail(vm: AppViewModel, serverId: String, entry: Entry) {
    val shape = RoundedCornerShape(8.dp)
    val modifier = Modifier.size(44.dp).clip(shape)
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

private fun iconFor(entry: Entry): ImageVector = when {
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
    val modified = DateUtils.getRelativeTimeSpanString(entry.modified, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS)
    return if (entry.dir) "Folder · $modified" else "${Formatter.formatShortFileSize(context, entry.size.toLong())} · $modified"
}

/** Shows the file in whichever app handles its type, such as Photos, a PDF reader, or a player. */
private fun openWithAnotherApp(context: Context, file: File) {
    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    val type = MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase()) ?: "application/octet-stream"
    val intent = Intent(Intent.ACTION_VIEW)
        .setDataAndType(uri, type)
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    context.startActivity(intent)
}
