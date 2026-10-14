package app.ownnas.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import app.ownnas.android.ui.AppViewModel
import app.ownnas.android.ui.BrowserScreen
import app.ownnas.android.ui.ConflictDialog
import app.ownnas.android.ui.Screen
import app.ownnas.android.ui.HomeScreen
import app.ownnas.android.ui.SignInScreen
import app.ownnas.android.ui.SyncScreen
import app.ownnas.android.ui.BackupScreen
import app.ownnas.android.ui.gallery.GalleryAlbumScreen
import app.ownnas.android.ui.gallery.GalleryHiddenScreen
import app.ownnas.android.ui.gallery.GalleryPersonScreen
import app.ownnas.android.ui.gallery.GalleryResultsScreen
import app.ownnas.android.ui.gallery.GalleryScreen
import app.ownnas.android.ui.gallery.GalleryTagScreen
import app.ownnas.android.ui.gallery.PhotoViewer
import app.ownnas.android.ui.terminal.TerminalScreen
import app.ownnas.android.ui.terminal.TerminalsScreen
import app.ownnas.android.ui.theme.OwnNasTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            OwnNasTheme {
                App()
            }
        }
    }
}

@Composable
private fun App(vm: AppViewModel = viewModel()) {
    val screen = vm.stack.last()
    val conflict by vm.conflict.collectAsStateWithLifecycle()
    val saveable = rememberSaveableStateHolder()
    BackHandler(enabled = vm.stack.size > 1) { vm.pop() }
    AnimatedContent(targetState = screen, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "screen") { target ->
        // Each screen on the stack keeps its scroll position and typed text while covered.
        saveable.SaveableStateProvider(target.toString()) {
            when (target) {
                Screen.Servers -> HomeScreen(vm)
                is Screen.SignIn -> SignInScreen(vm, target.existing)
                is Screen.Browse -> BrowserScreen(vm, target.serverId, target.path)
                is Screen.Terminals -> TerminalsScreen(vm, target.serverId, target.path)
                is Screen.Terminal -> TerminalScreen(vm, target.serverId, target.terminalId, target.startup)
                Screen.Sync -> SyncScreen(vm)
                is Screen.Gallery -> GalleryScreen(vm, target.serverId, target.tab)
                is Screen.GalleryAlbum -> GalleryAlbumScreen(vm, target.serverId, target.albumId)
                is Screen.GalleryPerson -> GalleryPersonScreen(vm, target.serverId, target.personId)
                is Screen.GalleryTag -> GalleryTagScreen(vm, target.serverId, target.tag)
                is Screen.GalleryResults -> GalleryResultsScreen(vm, target.serverId, target.words, target.similarTo)
                is Screen.GalleryHidden -> GalleryHiddenScreen(vm, target.serverId)
                is Screen.PhotoViewer -> PhotoViewer(vm, target.serverId, target.start, target.list)
                Screen.Backup -> BackupScreen(vm)
            }
        }
    }
    conflict?.let { ConflictDialog(it) }
}
