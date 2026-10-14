package app.ownnas.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.outlined.Dns
import androidx.compose.material.icons.outlined.Bolt
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material.icons.outlined.CloudUpload
import androidx.compose.material.icons.outlined.Sync
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LargeTopAppBar
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.ScaffoldDefaults
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.ownnas.android.data.SavedServer

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ServersScreen(vm: AppViewModel, insets: WindowInsets = ScaffoldDefaults.contentWindowInsets) {
    val servers by vm.servers.collectAsStateWithLifecycle()
    var removing by remember { mutableStateOf<SavedServer?>(null) }
    val scroll = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        contentWindowInsets = insets,
        topBar = {
            LargeTopAppBar(
                title = { Text("OwnNAS") },
                actions = {
                    if (servers.isNotEmpty()) {
                        IconButton(onClick = { vm.push(Screen.Backup) }) { Icon(Icons.Outlined.CloudUpload, contentDescription = "Back up this phone") }
                        IconButton(onClick = { vm.push(Screen.Sync) }) { Icon(Icons.Outlined.Sync, contentDescription = "Sync") }
                    }
                },
                scrollBehavior = scroll,
            )
        },
        floatingActionButton = {
            if (servers.isNotEmpty()) {
                ExtendedFloatingActionButton(
                    onClick = { vm.push(Screen.SignIn()) },
                    icon = { Icon(Icons.Filled.Add, contentDescription = null) },
                    text = { Text("Add server") },
                )
            }
        },
    ) { padding ->
        if (servers.isEmpty()) {
            Welcome(Modifier.padding(padding)) { vm.push(Screen.SignIn()) }
        } else {
            LazyColumn(contentPadding = padding) {
                items(servers, key = { it.id }) { server ->
                    ServerRow(
                        server = server,
                        onOpen = { vm.push(Screen.Browse(server.id, "")) },
                        onGallery = { vm.push(Screen.Gallery(server.id)) },
                        onSignIn = { vm.push(Screen.SignIn(server)) },
                        onRemove = { removing = server },
                    )
                }
            }
        }
    }
    removing?.let { server ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Remove ${server.name}?") },
            text = { Text("This signs ${server.username} out on this phone. Nothing on the server is deleted.") },
            confirmButton = {
                TextButton(onClick = { vm.remove(server.id); removing = null }) { Text("Remove") }
            },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun ServerRow(server: SavedServer, onOpen: () -> Unit, onGallery: () -> Unit, onSignIn: () -> Unit, onRemove: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    ListItem(
        modifier = Modifier.clickable(onClick = onOpen),
        leadingContent = { Icon(Icons.Outlined.Dns, contentDescription = null) },
        headlineContent = { Text(server.name) },
        supportingContent = { Text("${server.username} · ${server.address.substringAfter("://")}") },
        trailingContent = {
            Box {
                IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "More") }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(text = { Text("Gallery") }, onClick = { menu = false; onGallery() })
                    DropdownMenuItem(text = { Text("Sign in again") }, onClick = { menu = false; onSignIn() })
                    DropdownMenuItem(text = { Text("Remove") }, onClick = { menu = false; onRemove() })
                }
            }
        },
    )
}

@Composable
private fun Welcome(modifier: Modifier, onAdd: () -> Unit) {
    Column(
        modifier = modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(Icons.Outlined.Dns, contentDescription = null, modifier = Modifier.size(56.dp), tint = MaterialTheme.colorScheme.primary)
        Text("Connect to your OwnNAS", style = MaterialTheme.typography.headlineSmall)
        Text(
            "Sign in with the address, username, and password you use in the browser.",
            style = MaterialTheme.typography.bodyMedium,
            textAlign = TextAlign.Center,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Button(onClick = onAdd) { Text("Add server") }
    }
}

/** The main screen: servers and shortcuts as tabs at the bottom. */
@Composable
fun HomeScreen(vm: AppViewModel) {
    var tab by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf(0) }
    // The tabs bar takes care of the navigation bar; each tab keeps only the top and sides.
    val insets = WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal)
    Column(Modifier.fillMaxSize()) {
        Box(Modifier.weight(1f)) {
            if (tab == 0) ServersScreen(vm, insets) else ShortcutsTab(vm, insets)
        }
        NavigationBar {
            NavigationBarItem(
                selected = tab == 0,
                onClick = { tab = 0 },
                icon = { Icon(Icons.Outlined.Dns, contentDescription = null) },
                label = { Text("Servers") },
            )
            NavigationBarItem(
                selected = tab == 1,
                onClick = { tab = 1 },
                icon = { Icon(Icons.Outlined.Bolt, contentDescription = null) },
                label = { Text("Shortcuts") },
            )
        }
    }
}
