package app.ownnas.android.ui

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.ParcelFileDescriptor
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import com.google.mlkit.vision.documentscanner.GmsDocumentScannerOptions
import com.google.mlkit.vision.documentscanner.GmsDocumentScanning
import com.google.mlkit.vision.documentscanner.GmsDocumentScanningResult
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** One file from the document scanner, opened as soon as the scan comes back: the scanner
 *  leaves its files in the app's cache, which Android empties when the phone is low on space. */
class ScanFile(val uri: Uri, val file: ParcelFileDescriptor)

/** Pages from the document scanner, kept open until they are uploaded or dropped. */
class Scan(val pdf: ScanFile?, val pages: List<ScanFile>) {
    /** When the scan was made, for its default name and as the uploads' modification time. */
    val taken = System.currentTimeMillis()

    val defaultName: String = "Scan " + SimpleDateFormat("yyyy-MM-dd HH.mm", Locale.ROOT).format(Date(taken))

    /** The files to upload: one PDF, or one JPEG per page ("Scan 1.jpg", "Scan 2.jpg", …). */
    fun files(name: String, pdf: Boolean): List<Pair<String, ScanFile>> {
        val document = this.pdf
        if (pdf && document != null) return listOf("$name.pdf" to document)
        if (pages.size == 1) return listOf("$name.jpg" to pages[0])
        val digits = pages.size.toString().length
        return pages.mapIndexed { index, page -> "$name ${(index + 1).toString().padStart(digits, '0')}.jpg" to page }
    }

    /** Closes the files not being uploaded and deletes the scanner's copies; the uploads keep
     *  their open descriptors, which still read after the files are deleted. */
    fun discard(keep: Collection<ScanFile> = emptyList()) {
        (pages + listOfNotNull(pdf)).forEach { scanned ->
            if (scanned !in keep) runCatching { scanned.file.close() }
            if (scanned.uri.scheme == "file") scanned.uri.path?.let { runCatching { File(it).delete() } }
        }
    }

    companion object {
        /** `null` when nothing could be opened. */
        fun open(context: Context, pdf: Uri?, pages: List<Uri>): Scan? {
            fun open(uri: Uri) = runCatching { context.contentResolver.openFileDescriptor(uri, "r") }.getOrNull()?.let { ScanFile(uri, it) }
            val opened = pages.mapNotNull(::open)
            val document = pdf?.let(::open)
            return if (opened.isEmpty() && document == null) null else Scan(document, opened)
        }
    }
}

private tailrec fun Context.activity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.activity()
    else -> null
}

/**
 * Google's document scanner (ML Kit, run by Google Play services): it finds the page's edges
 * and takes the photo when the page is steady and in focus, straightens it, and offers crop,
 * rotate, filters (colour, greyscale, black and white), cleaning stains and fingers away, more
 * pages, reordering, and importing photos from the gallery. Returns a function that opens it.
 */
@Composable
fun rememberDocumentScanner(onScanned: (Scan) -> Unit, onError: (String) -> Unit): () -> Unit {
    val context = LocalContext.current
    val scanned by rememberUpdatedState(onScanned)
    val failed by rememberUpdatedState(onError)
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) { result ->
        if (result.resultCode != Activity.RESULT_OK) return@rememberLauncherForActivityResult
        val scan = GmsDocumentScanningResult.fromActivityResultIntent(result.data) ?: return@rememberLauncherForActivityResult
        val pages = scan.pages.orEmpty().map { it.imageUri }
        if (pages.isEmpty() && scan.pdf == null) return@rememberLauncherForActivityResult
        Scan.open(context, scan.pdf?.uri, pages)?.let(scanned) ?: failed("The scan could not be read")
    }
    val scanner = remember {
        GmsDocumentScanning.getClient(
            GmsDocumentScannerOptions.Builder()
                .setScannerMode(GmsDocumentScannerOptions.SCANNER_MODE_FULL)
                .setGalleryImportAllowed(true)
                .setResultFormats(GmsDocumentScannerOptions.RESULT_FORMAT_JPEG, GmsDocumentScannerOptions.RESULT_FORMAT_PDF)
                .build(),
        )
    }
    return {
        val activity = context.activity()
        if (activity == null) {
            failed("The scanner could not be opened")
        } else {
            scanner.getStartScanIntent(activity)
                .addOnSuccessListener { launcher.launch(IntentSenderRequest.Builder(it).build()) }
                .addOnFailureListener { err ->
                    failed("The scanner needs Google Play services" + (err.message?.let { ": $it" } ?: ""))
                }
        }
    }
}

/** Names a scan and chooses one PDF or a photo per page before it is uploaded. */
@Composable
fun ScanDialog(scan: Scan, onDismiss: () -> Unit, onUpload: (name: String, pdf: Boolean) -> Unit) {
    var value by remember { mutableStateOf(TextFieldValue(scan.defaultName, TextRange(0, scan.defaultName.length))) }
    var pdf by remember { mutableStateOf(scan.pdf != null) }
    val name = value.text.trim()
    val valid = name.isNotEmpty() && name != "." && name != ".." && '/' !in name && '\\' !in name
    val count = scan.pages.size
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Save scan") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                if (count > 0) {
                    LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        itemsIndexed(scan.pages) { _, page -> PageThumbnail(page.uri) }
                    }
                }
                OutlinedTextField(
                    value = value,
                    onValueChange = { value = it },
                    singleLine = true,
                    label = { Text("Name") },
                    isError = name.isNotEmpty() && !valid,
                    supportingText = { Text(if (name.isNotEmpty() && !valid) "Names cannot contain / or \\" else pageCount(count)) },
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { if (valid) onUpload(name, pdf) }),
                    modifier = Modifier.fillMaxWidth(),
                )
                if (scan.pdf != null && count > 0) {
                    SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                        SegmentedButton(
                            selected = pdf,
                            onClick = { pdf = true },
                            shape = SegmentedButtonDefaults.itemShape(0, 2),
                        ) { Text("PDF") }
                        SegmentedButton(
                            selected = !pdf,
                            onClick = { pdf = false },
                            shape = SegmentedButtonDefaults.itemShape(1, 2),
                        ) { Text(if (count == 1) "JPEG" else "$count JPEGs") }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = { onUpload(name, pdf) }, enabled = valid) { Text("Upload") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Discard") } },
    )
}

private fun pageCount(count: Int) = when (count) {
    0 -> ""
    1 -> "1 page"
    else -> "$count pages"
}

@Composable
private fun PageThumbnail(uri: Uri) {
    val context = LocalContext.current
    val image by produceState<ImageBitmap?>(null, uri) {
        value = withContext(Dispatchers.IO) { runCatching { decodeSmall(context, uri, 240) }.getOrNull() }
    }
    val shape = MaterialTheme.shapes.extraSmall
    val modifier = Modifier.height(96.dp).width(72.dp).border(1.dp, MaterialTheme.colorScheme.outlineVariant, shape)
    image?.let { Image(it, contentDescription = null, contentScale = ContentScale.Fit, modifier = modifier) }
        ?: androidx.compose.foundation.layout.Box(modifier)
}

private fun decodeSmall(context: Context, uri: Uri, longest: Int): ImageBitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= longest) sample *= 2
    val options = BitmapFactory.Options().apply { inSampleSize = sample }
    return context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, options) }?.asImageBitmap()
}
