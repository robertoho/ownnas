package app.ownnas.android.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext

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

/** Material You colors from the wallpaper on Android 12+, OwnNAS blue before that. */
@Composable
fun OwnNasTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val colors = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            val context = LocalContext.current
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        }
        dark -> Dark
        else -> Light
    }
    MaterialTheme(colorScheme = colors, content = content)
}
