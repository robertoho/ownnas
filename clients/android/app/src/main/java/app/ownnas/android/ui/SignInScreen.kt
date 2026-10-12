package app.ownnas.android.ui

import android.util.Base64
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.TextButton
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.Alignment
import androidx.compose.ui.unit.dp
import app.ownnas.android.data.SavedServer
import app.ownnas.android.data.Trust
import app.ownnas.core.ClientException
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SignInScreen(vm: AppViewModel, existing: SavedServer?) {
    var address by rememberSaveable { mutableStateOf(existing?.address.orEmpty()) }
    var username by rememberSaveable { mutableStateOf(existing?.username.orEmpty()) }
    var password by rememberSaveable { mutableStateOf("") }
    var showPassword by rememberSaveable { mutableStateOf(false) }
    var busy by rememberSaveable { mutableStateOf(false) }
    var error by rememberSaveable { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val ready = address.isNotBlank() && username.isNotBlank() && password.isNotEmpty() && !busy
    var trust by remember { mutableStateOf(existing?.trust ?: Trust()) }
    // The fingerprint of a certificate this phone does not trust yet, while asking about it.
    var untrusted by remember { mutableStateOf<String?>(null) }

    fun submit() {
        if (!ready) return
        busy = true
        error = null
        scope.launch {
            try {
                val server = vm.signIn(address, username, password, trust)
                vm.pop()
                if (existing == null) vm.push(Screen.Browse(server.id, ""))
            } catch (err: ClientException.Unauthorized) {
                error = "Wrong username or password"
            } catch (err: ClientException.Untrusted) {
                untrusted = runCatching { vm.fingerprint(address) }.getOrElse { "" }
            } catch (err: Exception) {
                error = err.userMessage()
            } finally {
                busy = false
            }
        }
    }

    // The ca.crt (PEM) or ca.cer (DER) from `ownnas tls generate`.
    val pickCa = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        val bytes = runCatching { context.contentResolver.openInputStream(uri)?.use { it.readBytes() } }.getOrNull()
        val pem = bytes?.let(::asPem)
        if (pem == null) {
            error = "That file is not a certificate"
        } else {
            trust = trust.copy(caPem = pem)
            submit()
        }
    }

    untrusted?.let { print ->
        TrustDialog(
            host = address.substringAfter("://").substringBefore('/'),
            fingerprint = print,
            onTrust = {
                untrusted = null
                // A renewed certificate replaces the one pinned before.
                trust = trust.copy(pinned = listOf(print))
                submit()
            },
            onUseCa = {
                untrusted = null
                pickCa.launch(arrayOf("*/*"))
            },
            onDismiss = { untrusted = null },
        )
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(if (existing == null) "Add server" else "Sign in to ${existing.name}") },
                navigationIcon = {
                    IconButton(onClick = { vm.pop() }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .padding(padding)
                .imePadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 24.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            OutlinedTextField(
                value = address,
                onValueChange = { address = it },
                label = { Text("Server address") },
                placeholder = { Text("nas.local:8787") },
                supportingText = { Text("Use https:// when the server is outside your home network or VPN.") },
                singleLine = true,
                enabled = existing == null && !busy,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Next, autoCorrectEnabled = false),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = username,
                onValueChange = { username = it },
                label = { Text("Username") },
                singleLine = true,
                enabled = existing == null && !busy,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next, autoCorrectEnabled = false),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = password,
                onValueChange = { password = it },
                label = { Text("Password") },
                singleLine = true,
                enabled = !busy,
                visualTransformation = if (showPassword) VisualTransformation.None else PasswordVisualTransformation(),
                trailingIcon = {
                    IconButton(onClick = { showPassword = !showPassword }) {
                        Icon(
                            if (showPassword) Icons.Filled.VisibilityOff else Icons.Filled.Visibility,
                            contentDescription = if (showPassword) "Hide password" else "Show password",
                        )
                    }
                },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { submit() }),
                modifier = Modifier.fillMaxWidth(),
            )
            error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium) }
            Button(onClick = ::submit, enabled = ready, modifier = Modifier.fillMaxWidth()) {
                if (busy) CircularProgressIndicator(modifier = Modifier.size(20.dp), strokeWidth = 2.dp) else Text("Sign in")
            }
        }
    }
}

/** Shows a certificate's fingerprint so it can be checked before the phone trusts it. */
@Composable
private fun TrustDialog(host: String, fingerprint: String, onTrust: () -> Unit, onUseCa: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Trust this server?") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.verticalScroll(rememberScrollState())) {
                Text("This phone does not trust the HTTPS certificate of $host.")
                if (fingerprint.isNotEmpty()) {
                    Text(
                        "Compare this with the line “HTTPS certificate SHA-256” that ownnas serve prints when it starts. " +
                            "Trust it only if every character matches.",
                    )
                    Text(fingerprint, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                }
                Text(
                    "Or use the ca.crt file from ownnas tls generate. That keeps working when the server's certificate is renewed.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        },
        confirmButton = {
            Column(horizontalAlignment = Alignment.End) {
                if (fingerprint.isNotEmpty()) TextButton(onClick = onTrust) { Text("Trust this certificate") }
                TextButton(onClick = onUseCa) { Text("Use ca.crt…") }
                TextButton(onClick = onDismiss) { Text("Cancel") }
            }
        },
    )
}

/** A certificate file as PEM, whether it was saved as PEM (ca.crt) or DER (ca.cer). */
private fun asPem(bytes: ByteArray): String? {
    val text = runCatching { String(bytes, Charsets.US_ASCII) }.getOrNull()
    if (text != null && text.contains("-----BEGIN CERTIFICATE-----")) return text
    // DER starts with an ASN.1 sequence.
    if (bytes.size < 64 || bytes[0] != 0x30.toByte()) return null
    val base64 = Base64.encodeToString(bytes, Base64.NO_WRAP).chunked(64).joinToString("\n")
    return "-----BEGIN CERTIFICATE-----\n$base64\n-----END CERTIFICATE-----\n"
}
