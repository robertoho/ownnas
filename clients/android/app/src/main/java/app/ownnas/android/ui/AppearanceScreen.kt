package app.ownnas.android.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.outlined.Wallpaper
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.ownnas.android.ui.theme.SYSTEM_THEME
import app.ownnas.android.ui.theme.ThemeChoice
import app.ownnas.android.ui.theme.WebTheme
import app.ownnas.android.ui.theme.webThemes

/** The web app's themes, to colour this app the same way; or the phone's own colours. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AppearanceScreen(vm: AppViewModel) {
    val context = LocalContext.current
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Appearance") },
                navigationIcon = { IconButton(onClick = { vm.pop() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") } },
            )
        },
    ) { padding ->
        LazyVerticalGrid(
            columns = GridCells.Adaptive(150.dp),
            modifier = Modifier.padding(padding).fillMaxSize(),
            contentPadding = PaddingValues(12.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    "The same themes as the web app. The phone's colours follow your wallpaper and its light or dark mode.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(4.dp),
                )
            }
            item(span = { GridItemSpan(maxLineSpan) }) {
                SystemCard(chosen = ThemeChoice.id == SYSTEM_THEME) { ThemeChoice.choose(context, SYSTEM_THEME) }
            }
            item(span = { GridItemSpan(maxLineSpan) }) { Heading("Dark") }
            items(webThemes.filter { it.dark }, key = { it.id }) { theme ->
                ThemeCard(theme, chosen = ThemeChoice.id == theme.id) { ThemeChoice.choose(context, theme.id) }
            }
            item(span = { GridItemSpan(maxLineSpan) }) { Heading("Light") }
            items(webThemes.filter { !it.dark }, key = { it.id }) { theme ->
                ThemeCard(theme, chosen = ThemeChoice.id == theme.id) { ThemeChoice.choose(context, theme.id) }
            }
        }
    }
}

@Composable
private fun Heading(text: String) {
    Text(text, style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 4.dp, top = 8.dp))
}

@Composable
private fun SystemCard(chosen: Boolean, onClick: () -> Unit) {
    Surface(
        shape = RoundedCornerShape(14.dp),
        color = MaterialTheme.colorScheme.surfaceContainer,
        border = BorderStroke(if (chosen) 2.dp else 1.dp, if (chosen) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant),
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick),
    ) {
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Outlined.Wallpaper, contentDescription = null, tint = MaterialTheme.colorScheme.primary)
            Column(Modifier.weight(1f).padding(start = 12.dp)) {
                Text("Phone colours")
                Text("From the wallpaper (Material You)", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (chosen) Icon(Icons.Filled.CheckCircle, contentDescription = "Chosen", tint = MaterialTheme.colorScheme.primary)
        }
    }
}

/** A theme drawn in its own colours: a little window with a bar, text lines, and its accent. */
@Composable
private fun ThemeCard(theme: WebTheme, chosen: Boolean, onClick: () -> Unit) {
    val bg = Color(theme.bg)
    Surface(
        shape = RoundedCornerShape(14.dp),
        color = bg,
        border = BorderStroke(if (chosen) 3.dp else 1.dp, if (chosen) MaterialTheme.colorScheme.primary else Color(theme.line)),
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick),
    ) {
        Column {
            Box(Modifier.fillMaxWidth().height(22.dp).background(Color(theme.bg2)))
            Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(5.dp)) {
                Box(Modifier.fillMaxWidth(0.8f).height(6.dp).background(Color(theme.text), RoundedCornerShape(3.dp)))
                Box(Modifier.fillMaxWidth(0.55f).height(6.dp).background(Color(theme.muted), RoundedCornerShape(3.dp)))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(width = 44.dp, height = 14.dp).background(Color(theme.accent), RoundedCornerShape(7.dp)))
                    Box(Modifier.weight(1f))
                    if (chosen) Icon(Icons.Filled.CheckCircle, contentDescription = "Chosen", tint = Color(theme.accent), modifier = Modifier.size(18.dp).background(bg, CircleShape))
                }
                Text(theme.name, color = Color(theme.text), style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}
