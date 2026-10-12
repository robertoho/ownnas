package app.ownnas.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.outlined.Bolt
import androidx.compose.material.icons.outlined.Code
import androidx.compose.material.icons.outlined.Input
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.ExposedDropdownMenuDefaults
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuAnchorType
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.ownnas.android.data.Snippet
import app.ownnas.android.data.TerminalShortcut
import app.ownnas.core.ClientException
import kotlinx.coroutines.launch

/** What the editor is showing: a new or existing shortcut or snippet. */
private sealed interface Editing {
    data class Shortcut(val shortcut: TerminalShortcut?) : Editing
    data class Snippet(val snippet: app.ownnas.android.data.Snippet?) : Editing
}

/** The main screen's Shortcuts tab: terminals to open in one tap, and snippets to run in any terminal. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShortcutsTab(vm: AppViewModel, insets: WindowInsets) {
    val shortcuts by vm.shortcuts.terminals.collectAsStateWithLifecycle()
    val snippets by vm.shortcuts.snippets.collectAsStateWithLifecycle()
    val servers by vm.servers.collectAsStateWithLifecycle()
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var editing by remember { mutableStateOf<Editing?>(null) }
    var adding by remember { mutableStateOf(false) }
    var opening by remember { mutableStateOf<String?>(null) }

    fun open(shortcut: TerminalShortcut) {
        opening = shortcut.id
        scope.launch {
            try {
                vm.push(vm.launch(shortcut))
            } catch (err: ClientException.Locked) {
                // The terminals screen asks for the terminal password.
                vm.push(Screen.Terminals(shortcut.serverId, shortcut.path))
            } catch (err: Exception) {
                snackbar.showSnackbar(err.userMessage())
            } finally {
                opening = null
            }
        }
    }

    Scaffold(
        contentWindowInsets = insets,
        topBar = { TopAppBar(title = { Text("Shortcuts") }) },
        floatingActionButton = {
            Box {
                FloatingActionButton(onClick = { adding = true }) { Icon(Icons.Filled.Add, contentDescription = "Add") }
                DropdownMenu(expanded = adding, onDismissRequest = { adding = false }) {
                    DropdownMenuItem(
                        text = { Text("Terminal shortcut") },
                        leadingIcon = { Icon(Icons.Outlined.Terminal, null) },
                        enabled = servers.isNotEmpty(),
                        onClick = { adding = false; editing = Editing.Shortcut(null) },
                    )
                    DropdownMenuItem(
                        text = { Text("Snippet") },
                        leadingIcon = { Icon(Icons.Outlined.Code, null) },
                        onClick = { adding = false; editing = Editing.Snippet(null) },
                    )
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        if (shortcuts.isEmpty() && snippets.isEmpty()) {
            Column(
                Modifier.padding(padding).fillMaxSize().padding(32.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Icon(Icons.Outlined.Bolt, null, Modifier.size(48.dp), tint = MaterialTheme.colorScheme.primary)
                Text(
                    "Open a terminal in one tap, already running what you need, and keep commands you use often as snippets.",
                    textAlign = TextAlign.Center,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (servers.isNotEmpty()) {
                    OutlinedButton(onClick = {
                        editing = Editing.Shortcut(TerminalShortcut(name = "tmux", serverId = servers.first().id, path = "", command = "tmux new -A -s main"))
                    }) { Text("Add a tmux shortcut") }
                }
                OutlinedButton(onClick = { editing = Editing.Snippet(null) }) { Text("Add a snippet") }
            }
        } else {
            LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(bottom = 88.dp)) {
                if (shortcuts.isNotEmpty()) item { Section("Terminals") }
                items(shortcuts, key = { it.id }) { shortcut ->
                    val server = vm.server(shortcut.serverId)
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        ListItem(
                            modifier = Modifier.weight(1f).clickable(enabled = opening == null) { open(shortcut) },
                            leadingContent = {
                                if (opening == shortcut.id) CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                                else Icon(Icons.Outlined.Terminal, null, tint = MaterialTheme.colorScheme.primary)
                            },
                            headlineContent = { Text(shortcut.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            supportingContent = {
                                Column {
                                    Text("${server?.name ?: "Removed server"}/${shortcut.path}".trimEnd('/'), maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    if (shortcut.command.isNotBlank()) {
                                        Text(shortcut.command, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall)
                                    }
                                }
                            },
                        )
                        ItemMenu(onEdit = { editing = Editing.Shortcut(shortcut) }, onDelete = { vm.shortcuts.removeTerminal(shortcut.id) })
                    }
                }
                if (snippets.isNotEmpty()) item { Section("Snippets") }
                items(snippets, key = { it.id }) { snippet ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        ListItem(
                            modifier = Modifier.weight(1f).clickable { editing = Editing.Snippet(snippet) },
                            leadingContent = { Icon(Icons.Outlined.Code, null) },
                            headlineContent = { Text(snippet.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            supportingContent = {
                                Text(snippet.command, fontFamily = FontFamily.Monospace, maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall)
                            },
                        )
                        ItemMenu(onEdit = { editing = Editing.Snippet(snippet) }, onDelete = { vm.shortcuts.removeSnippet(snippet.id) })
                    }
                }
                item {
                    Text(
                        "Snippets run from the ⚡ button in any terminal. A shortcut's terminal is named after it, so opening it again goes back to it while it runs.",
                        modifier = Modifier.padding(16.dp),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }

    when (val current = editing) {
        is Editing.Shortcut -> ShortcutDialog(vm, current.shortcut, onDismiss = { editing = null }) { saved ->
            vm.shortcuts.save(saved)
            editing = null
        }
        is Editing.Snippet -> SnippetDialog(current.snippet, onDismiss = { editing = null }) { saved ->
            vm.shortcuts.save(saved)
            editing = null
        }
        null -> {}
    }
}

@Composable
private fun Section(title: String) {
    Text(
        title,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 4.dp),
    )
}

@Composable
private fun ItemMenu(onEdit: () -> Unit, onDelete: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "More") }
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            DropdownMenuItem(text = { Text("Edit") }, onClick = { menu = false; onEdit() })
            DropdownMenuItem(text = { Text("Delete") }, onClick = { menu = false; onDelete() })
        }
    }
}

/** Creates or edits a terminal shortcut. `initial` without a matching saved id is a prefilled new one. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShortcutDialog(vm: AppViewModel, initial: TerminalShortcut?, onDismiss: () -> Unit, onSave: (TerminalShortcut) -> Unit) {
    val servers by vm.servers.collectAsStateWithLifecycle()
    val snippets by vm.shortcuts.snippets.collectAsStateWithLifecycle()
    var name by remember { mutableStateOf(initial?.name ?: "") }
    var serverId by remember { mutableStateOf(initial?.serverId ?: servers.firstOrNull()?.id) }
    var path by remember { mutableStateOf(initial?.path ?: "") }
    var command by remember { mutableStateOf(initial?.command ?: "") }
    var pickServer by remember { mutableStateOf(false) }
    var pickSnippet by remember { mutableStateOf(false) }
    val valid = name.isNotBlank() && serverId != null
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (initial == null || vm.shortcuts.terminals.value.none { it.id == initial.id }) "New terminal shortcut" else "Edit shortcut") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(name, { name = it }, label = { Text("Name") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                ExposedDropdownMenuBox(expanded = pickServer, onExpandedChange = { pickServer = it }) {
                    OutlinedTextField(
                        value = servers.firstOrNull { it.id == serverId }?.let { "${it.name} · ${it.username}" } ?: "",
                        onValueChange = {},
                        readOnly = true,
                        label = { Text("Server") },
                        trailingIcon = { ExposedDropdownMenuDefaults.TrailingIcon(pickServer) },
                        modifier = Modifier.fillMaxWidth().menuAnchor(MenuAnchorType.PrimaryNotEditable),
                    )
                    ExposedDropdownMenu(expanded = pickServer, onDismissRequest = { pickServer = false }) {
                        servers.forEach { server ->
                            DropdownMenuItem(text = { Text("${server.name} · ${server.address.substringAfter("://")}") }, onClick = {
                                serverId = server.id
                                pickServer = false
                            })
                        }
                    }
                }
                OutlinedTextField(
                    value = path,
                    onValueChange = { path = it },
                    label = { Text("Folder") },
                    placeholder = { Text("Projects/website") },
                    supportingText = { Text("Inside the shared folder. Leave empty for the top.") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = command,
                    onValueChange = { command = it },
                    label = { Text("Run on start") },
                    placeholder = { Text("tmux new -A -s main") },
                    supportingText = { Text("Runs when a new terminal opens. One command per line.") },
                    textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                    minLines = 2,
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, imeAction = ImeAction.Default),
                    modifier = Modifier.fillMaxWidth(),
                )
                if (snippets.isNotEmpty()) {
                    Box {
                        TextButton(onClick = { pickSnippet = true }) { Text("Use a snippet…") }
                        DropdownMenu(expanded = pickSnippet, onDismissRequest = { pickSnippet = false }) {
                            snippets.forEach { snippet ->
                                DropdownMenuItem(text = { Text(snippet.name) }, onClick = {
                                    command = snippet.command
                                    if (name.isBlank()) name = snippet.name
                                    pickSnippet = false
                                })
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(enabled = valid, onClick = {
                val base = initial ?: TerminalShortcut(name = "", serverId = "", path = "", command = "")
                onSave(base.copy(name = name.trim(), serverId = serverId!!, path = path.trim().trim('/'), command = command.trimEnd()))
            }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
fun SnippetDialog(initial: Snippet?, onDismiss: () -> Unit, onSave: (Snippet) -> Unit) {
    var name by remember { mutableStateOf(initial?.name ?: "") }
    var command by remember { mutableStateOf(initial?.command ?: "") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (initial == null) "New snippet" else "Edit snippet") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(name, { name = it }, label = { Text("Name") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(
                    value = command,
                    onValueChange = { command = it },
                    label = { Text("Command") },
                    placeholder = { Text("git pull && npm test") },
                    textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                    minLines = 3,
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        },
        confirmButton = {
            TextButton(enabled = name.isNotBlank() && command.isNotBlank(), onClick = {
                val base = initial ?: Snippet(name = "", command = "")
                onSave(base.copy(name = name.trim(), command = command.trimEnd()))
            }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** The terminal's snippet list: tap to run, or the second button to only type it. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SnippetsSheet(snippets: List<Snippet>, onDismiss: () -> Unit, onRun: (Snippet) -> Unit, onInsert: (Snippet) -> Unit, onNew: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.navigationBarsPadding().padding(bottom = 8.dp)) {
            Text("Snippets", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp))
            if (snippets.isEmpty()) {
                Text(
                    "No snippets yet. Save commands you use often and run them here in one tap.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
                )
            }
            snippets.forEach { snippet ->
                ListItem(
                    modifier = Modifier.clickable { onRun(snippet) },
                    leadingContent = { Icon(Icons.Outlined.Bolt, null, tint = MaterialTheme.colorScheme.primary) },
                    headlineContent = { Text(snippet.name) },
                    supportingContent = {
                        Text(snippet.command, fontFamily = FontFamily.Monospace, maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall)
                    },
                    trailingContent = {
                        IconButton(onClick = { onInsert(snippet) }) { Icon(Icons.Outlined.Input, contentDescription = "Type without running") }
                    },
                )
            }
            TextButton(onClick = onNew, modifier = Modifier.padding(horizontal = 12.dp)) { Text("New snippet") }
        }
    }
}
