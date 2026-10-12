package app.ownnas.android.data

import android.content.Context
import androidx.core.content.edit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

/** A saved command to run in any terminal, like Termius's snippets. */
data class Snippet(
    val id: String = UUID.randomUUID().toString(),
    val name: String,
    /** One or more lines; each is typed and run in turn. */
    val command: String,
)

/**
 * Opens a terminal in a server folder and runs a command there, such as `tmux new -A -s main`.
 * The terminal is named after the shortcut, so opening it again returns to it while it runs.
 */
data class TerminalShortcut(
    val id: String = UUID.randomUUID().toString(),
    val name: String,
    val serverId: String,
    /** Folder relative to the server's shared folder ("" for the top). */
    val path: String,
    /** Run once when a new terminal opens; empty for a plain shell. */
    val command: String,
)

/** Shortcuts and snippets, kept on this phone. */
class Shortcuts(context: Context) {
    private val prefs = context.getSharedPreferences("shortcuts", Context.MODE_PRIVATE)
    private val terminalList = MutableStateFlow(loadTerminals())
    private val snippetList = MutableStateFlow(loadSnippets())
    val terminals: StateFlow<List<TerminalShortcut>> = terminalList.asStateFlow()
    val snippets: StateFlow<List<Snippet>> = snippetList.asStateFlow()

    /** Adds the shortcut, or replaces the one with the same id. */
    fun save(shortcut: TerminalShortcut) = saveTerminals(upsert(terminalList.value, shortcut) { it.id })

    fun removeTerminal(id: String) = saveTerminals(terminalList.value.filterNot { it.id == id })

    fun forgetServer(serverId: String) = saveTerminals(terminalList.value.filterNot { it.serverId == serverId })

    fun save(snippet: Snippet) = saveSnippets(upsert(snippetList.value, snippet) { it.id })

    fun removeSnippet(id: String) = saveSnippets(snippetList.value.filterNot { it.id == id })

    private fun <T> upsert(list: List<T>, item: T, id: (T) -> String): List<T> =
        if (list.any { id(it) == id(item) }) list.map { if (id(it) == id(item)) item else it } else list + item

    private fun saveTerminals(list: List<TerminalShortcut>) {
        val array = JSONArray(
            list.map {
                JSONObject().put("id", it.id).put("name", it.name).put("serverId", it.serverId)
                    .put("path", it.path).put("command", it.command)
            },
        )
        prefs.edit { putString(TERMINALS, array.toString()) }
        terminalList.value = list
    }

    private fun saveSnippets(list: List<Snippet>) {
        val array = JSONArray(list.map { JSONObject().put("id", it.id).put("name", it.name).put("command", it.command) })
        prefs.edit { putString(SNIPPETS, array.toString()) }
        snippetList.value = list
    }

    private fun objects(key: String): List<JSONObject> {
        val array = runCatching { JSONArray(prefs.getString(key, "[]")) }.getOrDefault(JSONArray())
        return (0 until array.length()).map(array::getJSONObject)
    }

    private fun loadTerminals() = objects(TERMINALS).map {
        TerminalShortcut(it.getString("id"), it.getString("name"), it.getString("serverId"), it.optString("path"), it.optString("command"))
    }

    private fun loadSnippets() = objects(SNIPPETS).map { Snippet(it.getString("id"), it.getString("name"), it.getString("command")) }

    private companion object {
        const val TERMINALS = "terminals"
        const val SNIPPETS = "snippets"
    }
}
