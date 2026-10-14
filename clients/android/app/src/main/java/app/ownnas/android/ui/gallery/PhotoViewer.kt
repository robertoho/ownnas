package app.ownnas.android.ui.gallery

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PlayCircle
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import app.ownnas.android.ui.AppViewModel
import app.ownnas.android.ui.Screen
import app.ownnas.android.ui.openWithAnotherApp
import app.ownnas.android.ui.parentPath
import app.ownnas.android.ui.shareWithAnotherApp
import app.ownnas.android.ui.userMessage
import app.ownnas.core.MediaItem
import app.ownnas.core.SmartFeatures
import kotlinx.coroutines.launch

/** Photos full screen: swipe between them, pinch or double-tap to zoom; videos open in a player. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PhotoViewer(vm: AppViewModel, serverId: String, start: Int, list: Long) {
    val items = remember(list) { vm.viewerLists[list].orEmpty() }
    if (items.isEmpty()) {
        LaunchedEffect(Unit) { vm.pop() }
        return
    }
    val pager = rememberPagerState(initialPage = start.coerceIn(0, items.size - 1)) { items.size }
    var chrome by remember { mutableStateOf(true) }
    var menu by remember { mutableStateOf(false) }
    var picking by remember { mutableStateOf(false) }
    var smart by remember { mutableStateOf(SmartFeatures(search = false, faces = false)) }
    LaunchedEffect(serverId) { runCatching { vm.call(serverId) { it.smartFeatures() } }.onSuccess { smart = it } }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val current = items[pager.currentPage]
    fun withFile(share: Boolean) = scope.launchCatching(onError = { scope.launch { snackbar.showSnackbar(it.userMessage()) } }) {
        val entry = app.ownnas.core.Entry(current.name, current.path, false, (current.size ?: 0).toULong(), 0L, current.kind ?: "image", true)
        val file = vm.download(serverId, entry, app.ownnas.android.ui.Tracker {})
        if (share) shareWithAnotherApp(context, file) else openWithAnotherApp(context, file)
    }
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        HorizontalPager(pager, beyondViewportPageCount = 1, modifier = Modifier.fillMaxSize()) { page ->
            ViewerPage(vm, serverId, items[page], onTap = { chrome = !chrome }, onPlay = { withFile(false) })
        }
        if (chrome) {
            TopAppBar(
                title = {
                    Column {
                        Text(current.name, maxLines = 1)
                        current.takenLabel()?.let { Text(it, style = androidx.compose.material3.MaterialTheme.typography.bodySmall) }
                    }
                },
                navigationIcon = { IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") } },
                actions = {
                    IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "More") }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        if (smart.search) DropdownMenuItem(text = { Text("Find similar photos") }, onClick = { menu = false; vm.push(Screen.GalleryResults(serverId, similarTo = current.path)) })
                        DropdownMenuItem(text = { Text("Add to album…") }, onClick = { menu = false; picking = true })
                        DropdownMenuItem(text = { Text(if (current.hidden) "Show in Photos" else "Hide from Photos") }, onClick = {
                            menu = false
                            scope.launchCatching(onError = { scope.launch { snackbar.showSnackbar(it.userMessage()) } }) {
                                vm.call(serverId) { it.hideFromTimeline(listOf(current.path), !current.hidden) }
                                snackbar.showSnackbar(if (current.hidden) "Back in Photos" else "Hidden from Photos (still in its folder and albums)")
                            }
                        })
                        DropdownMenuItem(text = { Text("Show in folder") }, onClick = { menu = false; vm.push(Screen.Browse(serverId, current.path.parentPath())) })
                        DropdownMenuItem(text = { Text("Open with another app") }, onClick = { menu = false; withFile(false) })
                        DropdownMenuItem(text = { Text("Share") }, onClick = { menu = false; withFile(true) })
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = Color(0x88000000),
                    titleContentColor = Color.White,
                    navigationIconContentColor = Color.White,
                    actionIconContentColor = Color.White,
                ),
            )
        }
        SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter))
    }
    if (picking) {
        AlbumPicker(vm, serverId, "Add to…", onDismiss = { picking = false }) { album ->
            picking = false
            scope.launchCatching(onError = { scope.launch { snackbar.showSnackbar(it.userMessage()) } }) {
                vm.call(serverId) { it.changeAlbum(album, listOf(current.path), true) }
                snackbar.showSnackbar("Added to the album")
            }
        }
    }
}

@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
private fun ViewerPage(vm: AppViewModel, serverId: String, item: MediaItem, onTap: () -> Unit, onPlay: () -> Unit) {
    val key = "g:$serverId:${item.path}:${item.takenAt ?: 0}:${item.size ?: 0}"
    val screen = with(LocalDensity.current) { LocalConfiguration.current.screenHeightDp.dp.toPx().toInt() }
    val thumb by produceState(vm.cachedThumbnail(key), key) { if (value == null) value = vm.thumbnailOf(serverId, item.path, key) }
    val video = item.kind == "video"
    // The whole photo, sized for the screen (videos only show their thumbnail).
    val full by produceState<androidx.compose.ui.graphics.ImageBitmap?>(null, item.path) {
        if (!video) value = vm.fullPhoto(serverId, item.path, item.size ?: 0, (screen * 1.5).toInt().coerceIn(1200, 4096))
    }
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    val transform = rememberTransformableState { zoom, pan, _ ->
        scale = (scale * zoom).coerceIn(1f, 6f)
        offset = if (scale == 1f) Offset.Zero else offset + pan
    }
    Box(
        Modifier.fillMaxSize()
            .pointerInput(item.path) {
                detectTapGestures(
                    onTap = { if (video) onPlay() else onTap() },
                    onDoubleTap = {
                        if (scale > 1f) { scale = 1f; offset = Offset.Zero } else scale = 2.5f
                    },
                )
            }
            .then(if (video) Modifier else Modifier.transformable(transform, lockRotationOnZoomPan = true, canPan = { scale > 1f })),
        contentAlignment = Alignment.Center,
    ) {
        val picture = full ?: thumb
        if (picture != null) {
            Image(
                bitmap = picture,
                contentDescription = item.name,
                contentScale = ContentScale.Fit,
                modifier = Modifier.fillMaxWidth().graphicsLayer {
                    scaleX = scale
                    scaleY = scale
                    translationX = offset.x
                    translationY = offset.y
                },
            )
        }
        if (video) {
            Icon(Icons.Filled.PlayCircle, contentDescription = "Play", tint = Color.White, modifier = Modifier.size(72.dp))
        } else if (full == null) {
            CircularProgressIndicator(color = Color.White, modifier = Modifier.align(Alignment.BottomCenter).size(24.dp))
        }
    }
}
