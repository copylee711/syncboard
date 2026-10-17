package app.syncboard.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/** Same paper/ink palette as the desktop app; green only marks "online". */
@Immutable
data class Palette(
    val bg: Color,
    val surface: Color,
    val ink: Color,
    val muted: Color,
    val line: Color,
    val green: Color,
    val amber: Color,
    val danger: Color,
    val onInk: Color,
)

private val Light = Palette(
    bg = Color(0xFFF5F4F0), surface = Color(0xFFFFFFFF), ink = Color(0xFF1C1C1A),
    muted = Color(0xFF74726B), line = Color(0xFFE4E2DB), green = Color(0xFF1F8A5B),
    amber = Color(0xFFC27C0E), danger = Color(0xFFC93B3B), onInk = Color(0xFFF5F4F0),
)

private val Dark = Palette(
    bg = Color(0xFF161615), surface = Color(0xFF1F1F1D), ink = Color(0xFFECEBE6),
    muted = Color(0xFF93918A), line = Color(0xFF2E2E2B), green = Color(0xFF4CC38A),
    amber = Color(0xFFE3A23B), danger = Color(0xFFEC6A6A), onInk = Color(0xFF161615),
)

val LocalPalette = staticCompositionLocalOf { Light }

/** "system", "light" or "dark" (see AppPrefs.theme). */
@Composable
fun isDark(mode: String): Boolean = when (mode) {
    "light" -> false
    "dark" -> true
    else -> isSystemInDarkTheme()
}

@Composable
fun SyncBoardTheme(mode: String = "system", content: @Composable () -> Unit) {
    val dark = isDark(mode)
    val p = if (dark) Dark else Light
    val scheme = (if (dark) darkColorScheme() else lightColorScheme()).copy(
        primary = p.ink, onPrimary = p.onInk, background = p.bg, onBackground = p.ink,
        surface = p.surface, onSurface = p.ink, surfaceVariant = p.bg, onSurfaceVariant = p.muted,
        outline = p.line, outlineVariant = p.line, error = p.danger,
        surfaceContainer = p.surface, surfaceContainerHigh = p.surface, surfaceContainerLow = p.surface,
        secondaryContainer = p.line, onSecondaryContainer = p.ink,
    )
    val base = Typography()
    val type = base.copy(
        headlineSmall = TextStyle(fontSize = 26.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.2).sp),
        titleMedium = TextStyle(fontSize = 16.sp, fontWeight = FontWeight.SemiBold),
        bodyMedium = base.bodyMedium.copy(fontSize = 14.sp),
        labelMedium = TextStyle(fontSize = 12.sp, fontWeight = FontWeight.SemiBold, color = p.muted),
    )
    androidx.compose.runtime.CompositionLocalProvider(LocalPalette provides p) {
        MaterialTheme(colorScheme = scheme, typography = type) {
            // Text without an explicit colour uses LocalContentColor, which is black unless a
            // Surface sets it; provide it here so headings follow the theme.
            androidx.compose.runtime.CompositionLocalProvider(
                androidx.compose.material3.LocalContentColor provides p.ink,
                content = content,
            )
        }
    }
}
