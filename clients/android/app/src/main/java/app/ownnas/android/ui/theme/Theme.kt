package app.ownnas.android.ui.theme

import android.app.Activity
import android.content.Context
import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat

private val Light = lightColorScheme(
    primary = Color(0xFF1F4E79),
    secondary = Color(0xFF4F6070),
    tertiary = Color(0xFF6A5778),
)

private val Dark = darkColorScheme(
    primary = Color(0xFF9CCBFB),
    secondary = Color(0xFFB7C8DB),
    tertiary = Color(0xFFD6BEE4),
)

/** "system": Material You colours from the wallpaper, following the phone's light or dark mode. */
const val SYSTEM_THEME = "system"

/** The chosen theme, remembered on this phone. */
object ThemeChoice {
    var id by mutableStateOf(SYSTEM_THEME)
        private set

    fun load(context: Context) {
        id = prefs(context).getString("theme", SYSTEM_THEME) ?: SYSTEM_THEME
    }

    fun choose(context: Context, theme: String) {
        prefs(context).edit().putString("theme", theme).apply()
        id = theme
    }

    private fun prefs(context: Context) = context.getSharedPreferences("appearance", Context.MODE_PRIVATE)
}

/** A web theme's colours as Material 3 roles: backgrounds and surfaces from its three
 *  background shades, text, accent, and danger, so every screen follows it. */
fun WebTheme.colorScheme(): ColorScheme {
    val bg = Color(bg)
    val bg2 = Color(bg2)
    val bg3 = Color(bg3)
    val text = Color(text)
    val muted = Color(muted)
    val accent = Color(accent)
    val accentInk = Color(accentInk)
    val danger = Color(danger)
    val line = Color(line)
    val base = if (dark) darkColorScheme() else lightColorScheme()
    return base.copy(
        primary = accent,
        onPrimary = accentInk,
        primaryContainer = lerp(bg2, accent, 0.28f),
        onPrimaryContainer = text,
        inversePrimary = accent,
        secondary = accent,
        onSecondary = accentInk,
        secondaryContainer = lerp(bg2, accent, 0.2f),
        onSecondaryContainer = text,
        tertiary = accent,
        onTertiary = accentInk,
        tertiaryContainer = lerp(bg2, accent, 0.2f),
        onTertiaryContainer = text,
        background = bg,
        onBackground = text,
        surface = bg,
        onSurface = text,
        surfaceVariant = bg2,
        onSurfaceVariant = muted,
        surfaceTint = accent,
        inverseSurface = text,
        inverseOnSurface = bg,
        error = danger,
        onError = if (dark) bg else Color.White,
        errorContainer = lerp(bg2, danger, 0.25f),
        onErrorContainer = text,
        outline = line,
        outlineVariant = lerp(bg, line, 0.6f),
        scrim = Color.Black,
        surfaceBright = bg3,
        surfaceDim = bg,
        surfaceContainerLowest = lerp(bg, bg2, 0.25f),
        surfaceContainerLow = lerp(bg, bg2, 0.5f),
        surfaceContainer = bg2,
        surfaceContainerHigh = lerp(bg2, bg3, 0.5f),
        surfaceContainerHighest = bg3,
    )
}

/** The chosen web theme, or Material You colours from the wallpaper (Android 12+, OwnNAS blue
 *  before that) when the choice is "system". */
@Composable
fun OwnNasTheme(content: @Composable () -> Unit) {
    val systemDark = isSystemInDarkTheme()
    val chosen = webThemes.firstOrNull { it.id == ThemeChoice.id }
    val dark = chosen?.dark ?: systemDark
    val colors = when {
        chosen != null -> chosen.colorScheme()
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            val context = LocalContext.current
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        }
        dark -> Dark
        else -> Light
    }
    // Status and navigation bar icons readable on the theme's background, not the phone's mode.
    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = (view.context as? Activity)?.window ?: return@SideEffect
            WindowCompat.getInsetsController(window, view).apply {
                isAppearanceLightStatusBars = !dark
                isAppearanceLightNavigationBars = !dark
            }
        }
    }
    MaterialTheme(colorScheme = colors, content = content)
}
