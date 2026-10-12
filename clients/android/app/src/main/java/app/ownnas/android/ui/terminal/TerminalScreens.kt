package app.ownnas.android.ui.terminal

import android.content.ClipboardManager
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import android.os.Handler
import android.os.Looper
import android.view.inputmethod.InputMethodManager
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.outlined.AttachFile
import androidx.compose.material.icons.outlined.ContentPaste
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import app.ownnas.android.ui.AppViewModel
import app.ownnas.android.ui.ShortcutDialog
import app.ownnas.android.ui.SnippetDialog
import app.ownnas.android.ui.SnippetsSheet
import app.ownnas.android.data.TerminalShortcut
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.compose.material.icons.outlined.Bolt
import app.ownnas.android.ui.Screen
import app.ownnas.android.ui.userMessage
import app.ownnas.core.ClientException
import app.ownnas.core.TerminalConnection
import app.ownnas.core.TerminalInfo
import app.ownnas.core.TerminalListener
import app.ownnas.core.TerminalSession
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** How an agent's status reads and looks, as in the web app's terminal tabs. */
internal fun agentStatus(info: TerminalInfo): Pair<String, Color>? {
    val agent = info.agent
    if (agent.name == null) return null
    val label = agent.label ?: agent.name!!
    // Attention outlasts the status that caused it (an agent can go idle after asking).
    if (agent.attention != null) return "$label · needs you" to Color(0xFFF59E0B)
    return when (agent.status) {
        "working" -> "$label · working" to Color(0xFF3B82F6)
        "waiting" -> "$label · needs you" to Color(0xFFF59E0B)
        "done" -> "$label · done" to Color(0xFF22C55E)
        "error" -> "$label · error" to Color(0xFFEF4444)
        else -> "$label · idle" to Color(0xFF9CA3AF)
    }
}

/** Grey after a clean exit, red after a failure. */
internal fun endedColor(info: TerminalInfo): Color =
    if (info.exitCode == null || info.exitCode == 0u) Color(0xFF9CA3AF) else Color(0xFFEF4444)

internal fun endedText(info: TerminalInfo): String =
    if (info.exitCode == null || info.exitCode == 0u) "Ended" else "Ended · exit code ${info.exitCode}"

/** A small round badge for the corner of an icon: a stop sign for an ended terminal. */
@Composable
internal fun EndedBadge(info: TerminalInfo, modifier: Modifier = Modifier) {
    Surface(
        shape = CircleShape,
        color = endedColor(info),
        border = androidx.compose.foundation.BorderStroke(2.dp, MaterialTheme.colorScheme.surface),
        modifier = modifier.size(18.dp),
    ) {
        Icon(Icons.Filled.Stop, contentDescription = endedText(info), tint = Color.White, modifier = Modifier.padding(3.dp))
    }
}

internal fun titleOf(info: TerminalInfo) =
    info.name?.takeIf { it.isNotBlank() } ?: info.title?.takeIf { it.isNotBlank() } ?: info.path.ifEmpty { "/" }

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TerminalsScreen(vm: AppViewModel, serverId: String, path: String) {
    val server = vm.server(serverId) ?: return
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var terminals by remember { mutableStateOf<List<TerminalInfo>?>(null) }
    var error by remember { mutableStateOf<Throwable?>(null) }
    var locked by remember { mutableStateOf(false) }
    var reload by remember { mutableIntStateOf(0) }
    var refreshing by remember { mutableStateOf(false) }
    var ending by remember { mutableStateOf<TerminalInfo?>(null) }
    var opening by remember { mutableStateOf(false) }

    // Agents change state on their own, so keep the list fresh while it is showing.
    LaunchedEffect(reload) {
        while (true) {
            try {
                terminals = vm.terminals(serverId)
                error = null
            } catch (err: ClientException.Locked) {
                locked = true
                return@LaunchedEffect
            } catch (err: Exception) {
                error = err
            } finally {
                refreshing = false
            }
            delay(3000)
        }
    }

    fun open() {
        opening = true
        scope.launch {
            try {
                val info = vm.openTerminal(serverId, path)
                vm.push(Screen.Terminal(serverId, info.id))
            } catch (err: ClientException.Locked) {
                locked = true
            } catch (err: Exception) {
                snackbar.showSnackbar(err.userMessage())
            } finally {
                opening = false
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Terminals") },
                navigationIcon = {
                    IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
                },
            )
        },
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = { if (!opening) open() },
                icon = { if (opening) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp) else Icon(Icons.Filled.Add, null) },
                text = { Text("New in ${path.substringAfterLast('/').ifEmpty { server.name }}") },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = { refreshing = true; reload++ },
            modifier = Modifier.padding(padding).fillMaxSize(),
        ) {
            val list = terminals
            when {
                list == null && error != null -> Centered(error!!.userMessage())
                list == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { if (!locked) CircularProgressIndicator() }
                list.isEmpty() -> Centered("No terminals are open. Start one in this folder with the button below.")
                else -> LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 88.dp)) {
                    items(list, key = { it.id }) { info -> TerminalRow(info, onOpen = { vm.push(Screen.Terminal(serverId, info.id)) }, onEnd = { ending = info }) }
                }
            }
        }
    }

    if (locked) {
        UnlockDialog(
            onDismiss = { locked = false; vm.pop() },
            onUnlock = { password ->
                vm.unlockTerminals(serverId, password)
                locked = false
                reload++
            },
        )
    }
    ending?.let { info ->
        AlertDialog(
            onDismissRequest = { ending = null },
            title = { Text("End this terminal?") },
            text = { Text("“${titleOf(info)}” and everything running in it stop on the server.") },
            confirmButton = {
                TextButton(onClick = {
                    ending = null
                    scope.launch {
                        runCatching { vm.closeTerminal(serverId, info.id) }.onFailure { snackbar.showSnackbar(it.userMessage()) }
                        reload++
                    }
                }) { Text("End terminal") }
            },
            dismissButton = { TextButton(onClick = { ending = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun TerminalRow(info: TerminalInfo, onOpen: () -> Unit, onEnd: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    val status = agentStatus(info)
    ListItem(
        modifier = Modifier.clickable(onClick = onOpen),
        leadingContent = {
            Box {
                Icon(
                    Icons.Outlined.Terminal,
                    contentDescription = null,
                    tint = if (info.running) LocalContentColor.current else MaterialTheme.colorScheme.outline,
                )
                if (!info.running) EndedBadge(info, Modifier.align(Alignment.BottomEnd).offset(x = 6.dp, y = 6.dp))
            }
        },
        headlineContent = {
            Text(
                titleOf(info),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                color = if (info.running) Color.Unspecified else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        },
        supportingContent = {
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                if (info.running) {
                    Text("/${info.path}", maxLines = 1, overflow = TextOverflow.Ellipsis)
                } else {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Box(Modifier.size(8.dp).background(endedColor(info), CircleShape))
                        Text("${endedText(info)} · /${info.path}", maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
                if (status != null) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Box(Modifier.size(8.dp).background(status.second, CircleShape))
                        Text(status.first, style = MaterialTheme.typography.labelMedium)
                    }
                    val detail = info.agent.attention ?: info.agent.detail
                    if (detail != null) Text(detail, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall)
                }
            }
        },
        trailingContent = {
            Box {
                IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "Actions") }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(text = { Text("End terminal") }, onClick = { menu = false; onEnd() })
                }
            }
        },
    )
}

@Composable
private fun Centered(text: String) {
    Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
        Text(text, textAlign = TextAlign.Center, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** Terminals can have their own password on top of the account's. */
@Composable
private fun UnlockDialog(onDismiss: () -> Unit, onUnlock: suspend (String) -> Unit) {
    var password by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Terminal password") },
        text = {
            OutlinedTextField(
                value = password,
                onValueChange = { password = it },
                label = { Text("Password") },
                singleLine = true,
                isError = error != null,
                supportingText = error?.let { { Text(it) } },
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
            )
        },
        confirmButton = {
            TextButton(
                enabled = password.isNotEmpty() && !busy,
                onClick = {
                    busy = true
                    scope.launch {
                        try {
                            onUnlock(password)
                        } catch (err: Exception) {
                            error = err.userMessage()
                        } finally {
                            busy = false
                        }
                    }
                },
            ) { Text("Unlock") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** The web terminal's extra keys, for what a phone keyboard lacks. */
private val EXTRA_KEYS = listOf(
    "Esc" to "\u001b", "Tab" to "\t", "⇤" to "\u001b[Z", "←" to null, "↑" to null, "↓" to null, "→" to null,
    "Home" to "\u001b[H", "End" to "\u001b[F", "PgUp" to "\u001b[5~", "PgDn" to "\u001b[6~", "^C" to "\u0003",
    "|" to "|", "/" to "/", "~" to "~", "-" to "-",
)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TerminalScreen(vm: AppViewModel, serverId: String, terminalId: String, startup: String? = null) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var session by remember { mutableStateOf<TerminalSession?>(null) }
    var state by remember { mutableStateOf<Pair<TerminalConnection, String?>>(TerminalConnection.Connecting to null) }
    var title by remember { mutableStateOf<String?>(null) }
    var ctrl by remember { mutableStateOf(false) }
    var alt by remember { mutableStateOf(false) }
    var ending by remember { mutableStateOf(false) }
    var view by remember { mutableStateOf<TerminalView?>(null) }
    var uploading by remember { mutableStateOf<String?>(null) }
    // The shell has drawn something, so it is ready for a shortcut's command.
    var output by remember { mutableStateOf(false) }
    var startupDone by rememberSaveable { mutableStateOf(startup == null) }
    var snippetsOpen by remember { mutableStateOf(false) }
    var newSnippet by remember { mutableStateOf(false) }
    var savingShortcut by remember { mutableStateOf(false) }
    val snippets by vm.shortcuts.snippets.collectAsStateWithLifecycle()

    /** Types each line and presses Enter, as if typed by hand. */
    fun run(command: String) {
        command.lines().filter { it.isNotBlank() }.forEach { session?.typeText(it + "\r") }
    }
    var info by remember { mutableStateOf<TerminalInfo?>(null) }
    val snackbar = remember { SnackbarHostState() }

    // Paste → upload to the terminal's temporary folder → paste the paths, as the web terminal does.
    fun pasteFiles(uris: List<Uri>) {
        if (uris.isEmpty()) return
        uploading = if (uris.size == 1) "Uploading the file…" else "Uploading ${uris.size} files…"
        scope.launch {
            try {
                val paths = vm.uploadToTerminal(serverId, terminalId, uris)
                session?.pastePaths(paths)
                view?.showKeyboard()
            } catch (err: Exception) {
                snackbar.showSnackbar(err.userMessage())
            } finally {
                uploading = null
            }
        }
    }
    val pickMedia = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia()) { pasteFiles(it) }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { pasteFiles(it) }

    DisposableEffect(serverId, terminalId) {
        val main = Handler(Looper.getMainLooper())
        // Called on the session's thread: redraw directly, move state changes to the main thread.
        val listener = object : TerminalListener {
            override fun screenChanged() {
                view?.postInvalidateOnAnimation()
                if (!output) main.post { output = true }
            }

            override fun connectionChanged(connection: TerminalConnection, message: String?) {
                main.post { state = connection to message }
            }
        }
        val job = scope.launch {
            try {
                session = vm.connectTerminal(serverId, terminalId, listener)
                vm.markTerminalSeen(serverId, terminalId)
            } catch (err: Exception) {
                state = TerminalConnection.Closed to err.userMessage()
            }
        }
        onDispose {
            job.cancel()
            session?.disconnect()
            session?.close()
        }
    }

    LaunchedEffect(output, state.first) {
        if (!startupDone && output && state.first == TerminalConnection.Live && startup != null) {
            // Let the prompt finish drawing first.
            delay(400)
            run(startup)
            startupDone = true
        }
    }

    // When the shell ends: after a clean `exit` the terminal is done, so close it and go back;
    // after a failure, keep the screen so what happened can be read.
    val exited = state.first as? TerminalConnection.Exited
    LaunchedEffect(exited) {
        if (exited == null) return@LaunchedEffect
        view?.let { terminal ->
            terminal.clearFocus()
            context.getSystemService(InputMethodManager::class.java)?.hideSoftInputFromWindow(terminal.windowToken, 0)
        }
        if (exited.code == 0u) {
            delay(700)
            runCatching { vm.closeTerminal(serverId, terminalId) }
            Toast.makeText(context, "Terminal closed", Toast.LENGTH_SHORT).show()
            if (vm.stack.lastOrNull() == Screen.Terminal(serverId, terminalId)) vm.pop()
        }
    }

    // The program's title wins; otherwise the terminal's name or folder, as in the web tabs.
    // The agent's status is refreshed with it.
    LaunchedEffect(serverId, terminalId) {
        while (true) {
            runCatching { vm.terminals(serverId).firstOrNull { it.id == terminalId } }.getOrNull()?.let { info = it }
            title = session?.snapshot()?.title ?: info?.let(::titleOf)
            delay(3000)
        }
    }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(title ?: "Terminal", maxLines = 1, overflow = TextOverflow.Ellipsis)
                        val status = when (val c = state.first) {
                            TerminalConnection.Connecting -> "Connecting…"
                            TerminalConnection.Live -> info?.let(::agentStatus)?.first
                            TerminalConnection.Reconnecting -> "Connection lost. Reconnecting…"
                            is TerminalConnection.Exited -> "Ended${c.code?.let { " with code $it" } ?: ""}"
                            TerminalConnection.Closed -> state.second ?: "Disconnected"
                        }
                        val shown = uploading ?: status
                        if (shown != null) Text(shown, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
                    }
                },
                navigationIcon = {
                    IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
                },
                actions = {
                    IconButton(onClick = { snippetsOpen = true }) { Icon(Icons.Outlined.Bolt, contentDescription = "Snippets") }
                    IconButton(onClick = { view?.showKeyboard() }) { Icon(Icons.Outlined.Keyboard, contentDescription = "Keyboard") }
                    IconButton(onClick = {
                        // Copied files and images (a screenshot's "Copy") are uploaded; text pastes as usual.
                        val clip = context.getSystemService(ClipboardManager::class.java)?.primaryClip
                        val items = clip?.let { c -> (0 until c.itemCount).map(c::getItemAt) }.orEmpty()
                        val files = items.mapNotNull { it.uri }.filter { it.scheme == "content" || it.scheme == "file" }
                        if (files.isNotEmpty()) {
                            pasteFiles(files)
                        } else {
                            val text = items.firstOrNull()?.coerceToText(context)?.toString()
                            if (!text.isNullOrEmpty()) session?.paste(text)
                        }
                    }) { Icon(Icons.Outlined.ContentPaste, contentDescription = "Paste") }
                    var attach by remember { mutableStateOf(false) }
                    Box {
                        IconButton(onClick = { attach = true }) { Icon(Icons.Outlined.AttachFile, contentDescription = "Attach a file") }
                        DropdownMenu(expanded = attach, onDismissRequest = { attach = false }) {
                            DropdownMenuItem(text = { Text("Photos and videos") }, onClick = {
                                attach = false
                                pickMedia.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageAndVideo))
                            })
                            DropdownMenuItem(text = { Text("Files") }, onClick = { attach = false; pickFiles.launch(arrayOf("*/*")) })
                        }
                    }
                    var menu by remember { mutableStateOf(false) }
                    Box {
                        IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "More") }
                        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                            DropdownMenuItem(text = { Text("Save as shortcut…") }, onClick = { menu = false; savingShortcut = true })
                            DropdownMenuItem(text = { Text("End terminal") }, onClick = { menu = false; ending = true })
                        }
                    }
                },
            )
        },
    ) { padding ->
        // The Scaffold already leaves room for the navigation bar; the keyboard replaces it.
        Column(Modifier.padding(padding).consumeWindowInsets(padding).fillMaxSize().imePadding()) {
            AndroidView(
                factory = { ctx ->
                    TerminalView(ctx) { _, _ -> }.also {
                        view = it
                        it.onModifiersUsed = { ctrl = false; alt = false }
                        it.onContent = { uri -> pasteFiles(listOf(uri)) }
                        // Ready to type as soon as the terminal opens.
                        it.post { it.showKeyboard() }
                    }
                },
                update = { it.session = session },
                modifier = Modifier.weight(1f).fillMaxWidth().background(Color(0xFF151810)),
            )
            if (exited != null && exited.code != 0u) {
                Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
                    Row(
                        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            "This terminal has ended${exited.code?.let { " (exit code $it)" } ?: ""}.",
                            style = MaterialTheme.typography.bodyMedium,
                            modifier = Modifier.weight(1f),
                        )
                        TextButton(onClick = {
                            scope.launch {
                                try {
                                    val fresh = vm.openTerminal(serverId, info?.path ?: "")
                                    runCatching { vm.closeTerminal(serverId, terminalId) }
                                    vm.pop()
                                    vm.push(Screen.Terminal(serverId, fresh.id))
                                } catch (err: Exception) {
                                    snackbar.showSnackbar(err.userMessage())
                                }
                            }
                        }) { Text("New terminal") }
                        TextButton(onClick = {
                            scope.launch {
                                runCatching { vm.closeTerminal(serverId, terminalId) }
                                vm.pop()
                            }
                        }) { Text("Close") }
                    }
                }
            } else Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
                Row(
                    Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 4.dp, vertical = 2.dp),
                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    FilterChip(selected = ctrl, onClick = { ctrl = !ctrl; view?.ctrlArmed = ctrl }, label = { Text("Ctrl") })
                    FilterChip(selected = alt, onClick = { alt = !alt; view?.altArmed = alt }, label = { Text("Alt") })
                    for ((label, bytes) in EXTRA_KEYS) {
                        TextButton(onClick = {
                            val application = session?.snapshot()?.applicationCursor == true
                            val arrow = mapOf("↑" to 'A', "↓" to 'B', "→" to 'C', "←" to 'D')[label]
                            val sequence = bytes ?: if (application) "\u001bO$arrow" else "\u001b[$arrow"
                            view?.send(sequence.toByteArray())
                        }) { Text(label, fontFamily = FontFamily.Monospace) }
                    }
                }
            }
        }
    }

    if (snippetsOpen) {
        SnippetsSheet(
            snippets = snippets,
            onDismiss = { snippetsOpen = false },
            onRun = { snippetsOpen = false; run(it.command); view?.showKeyboard() },
            onInsert = { snippetsOpen = false; session?.typeText(it.command); view?.showKeyboard() },
            onNew = { snippetsOpen = false; newSnippet = true },
        )
    }
    if (newSnippet) {
        SnippetDialog(null, onDismiss = { newSnippet = false }) { vm.shortcuts.save(it); newSnippet = false }
    }
    if (savingShortcut) {
        ShortcutDialog(
            vm,
            TerminalShortcut(name = info?.let(::titleOf) ?: "Terminal", serverId = serverId, path = info?.path ?: "", command = ""),
            onDismiss = { savingShortcut = false },
        ) {
            vm.shortcuts.save(it)
            savingShortcut = false
            scope.launch { snackbar.showSnackbar("Saved “${it.name}” in Shortcuts") }
        }
    }

    if (ending) {
        AlertDialog(
            onDismissRequest = { ending = false },
            title = { Text("End this terminal?") },
            text = { Text("The shell and everything running in it stop on the server.") },
            confirmButton = {
                TextButton(onClick = {
                    ending = false
                    scope.launch {
                        runCatching { vm.closeTerminal(serverId, terminalId) }
                        vm.pop()
                    }
                }) { Text("End terminal") }
            },
            dismissButton = { TextButton(onClick = { ending = false }) { Text("Cancel") } },
        )
    }
}
