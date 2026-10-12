package app.ownnas.android.data

import android.content.Context
import androidx.core.content.edit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

/** A server the app has signed in to. `name` is the server's shared folder name. */
data class SavedServer(
    val id: String,
    val address: String,
    val username: String,
    val name: String,
    val trust: Trust = Trust(),
)

/** Certificates to trust for an HTTPS server beyond the public ones. */
data class Trust(
    /** A private CA in PEM, such as the `ca.crt` from `ownnas tls generate`. */
    val caPem: String? = null,
    /** SHA-256 fingerprints of server certificates the user accepted. */
    val pinned: List<String> = emptyList(),
)

/** Saved servers in shared preferences, with each sign-in token sealed by [SecretBox]. */
class ServerStore(context: Context) {
    private val prefs = context.getSharedPreferences("servers", Context.MODE_PRIVATE)
    private val list = MutableStateFlow(load())
    val servers: StateFlow<List<SavedServer>> = list.asStateFlow()

    fun find(id: String): SavedServer? = list.value.firstOrNull { it.id == id }

    fun token(id: String): String? = prefs.getString(tokenKey(id), null)?.let(SecretBox::open)

    /** Adds the server, or replaces the saved one with the same address and username. */
    fun save(address: String, username: String, name: String, token: String, trust: Trust): SavedServer {
        val existing = list.value.firstOrNull { it.address == address && it.username == username }
        val server = SavedServer(existing?.id ?: UUID.randomUUID().toString(), address, username, name, trust)
        val updated = if (existing == null) list.value + server else list.value.map { if (it.id == server.id) server else it }
        prefs.edit {
            putString(LIST_KEY, encode(updated))
            putString(tokenKey(server.id), SecretBox.seal(token))
        }
        list.value = updated
        return server
    }

    fun forgetToken(id: String) {
        prefs.edit { remove(tokenKey(id)) }
    }

    fun remove(id: String) {
        val updated = list.value.filterNot { it.id == id }
        prefs.edit {
            putString(LIST_KEY, encode(updated))
            remove(tokenKey(id))
        }
        list.value = updated
    }

    private fun load(): List<SavedServer> {
        val array = runCatching { JSONArray(prefs.getString(LIST_KEY, "[]")) }.getOrDefault(JSONArray())
        return (0 until array.length()).map { index ->
            val item = array.getJSONObject(index)
            val pins = item.optJSONArray("pinned")
            val trust = Trust(
                caPem = item.optString("caPem").takeIf { it.isNotEmpty() },
                pinned = if (pins == null) emptyList() else (0 until pins.length()).map(pins::getString),
            )
            SavedServer(item.getString("id"), item.getString("address"), item.getString("username"), item.getString("name"), trust)
        }
    }

    private fun encode(servers: List<SavedServer>): String = JSONArray(
        servers.map { server ->
            JSONObject()
                .put("id", server.id)
                .put("address", server.address)
                .put("username", server.username)
                .put("name", server.name)
                .put("caPem", server.trust.caPem ?: "")
                .put("pinned", JSONArray(server.trust.pinned))
        },
    ).toString()

    private fun tokenKey(id: String) = "token.$id"

    private companion object {
        const val LIST_KEY = "list"
    }
}
