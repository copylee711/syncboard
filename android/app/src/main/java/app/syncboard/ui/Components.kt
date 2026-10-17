package app.syncboard.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

@Composable
fun SectionLabel(text: String, modifier: Modifier = Modifier, trailing: @Composable RowScope.() -> Unit = {}) {
    Row(
        modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, top = 22.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, color = LocalPalette.current.muted, modifier = Modifier.weight(1f))
        trailing()
    }
}

/** Bordered group of rows separated by hairlines, like the desktop app. */
@Composable
fun Group(content: @Composable ColumnScope.() -> Unit) {
    val p = LocalPalette.current
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(p.surface)
            .border(1.dp, p.line, RoundedCornerShape(12.dp)),
        content = content,
    )
}

@Composable
fun Divider() = HorizontalDivider(color = LocalPalette.current.line, thickness = 1.dp)

@Composable
fun ToggleRow(title: String, checked: Boolean, subtitle: String? = null, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    val p = LocalPalette.current
    Row(
        Modifier.fillMaxWidth().clickable(enabled = enabled) { onChange(!checked) }.padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, fontSize = 15.sp, color = if (enabled) p.ink else p.muted)
            if (subtitle != null) Text(subtitle, fontSize = 12.sp, color = p.muted, lineHeight = 16.sp)
        }
        Switch(
            checked = checked, onCheckedChange = onChange, enabled = enabled,
            colors = SwitchDefaults.colors(
                checkedTrackColor = p.ink, checkedThumbColor = p.onInk, checkedBorderColor = p.ink,
                uncheckedTrackColor = p.surface, uncheckedThumbColor = p.muted, uncheckedBorderColor = p.muted,
            ),
        )
    }
}

@Composable
fun ActionRow(title: String, subtitle: String? = null, color: Color? = null, onClick: () -> Unit) {
    val p = LocalPalette.current
    Column(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 14.dp)) {
        Text(title, fontSize = 15.sp, color = color ?: p.ink)
        if (subtitle != null) Text(subtitle, fontSize = 12.sp, color = p.muted, lineHeight = 16.sp)
    }
}

@Composable
fun InkButton(text: String, modifier: Modifier = Modifier, enabled: Boolean = true, onClick: () -> Unit) {
    val p = LocalPalette.current
    Button(
        onClick, modifier.height(46.dp), enabled = enabled, shape = RoundedCornerShape(10.dp),
        colors = ButtonDefaults.buttonColors(containerColor = p.ink, contentColor = p.onInk),
        contentPadding = PaddingValues(horizontal = 16.dp),
    ) { Text(text, fontSize = 15.sp, fontWeight = FontWeight.Medium) }
}

@Composable
fun LineButton(text: String, modifier: Modifier = Modifier, enabled: Boolean = true, onClick: () -> Unit) {
    val p = LocalPalette.current
    OutlinedButton(
        onClick, modifier.height(46.dp), enabled = enabled, shape = RoundedCornerShape(10.dp),
        border = BorderStroke(1.dp, p.line),
        colors = ButtonDefaults.outlinedButtonColors(containerColor = p.surface, contentColor = p.ink),
        contentPadding = PaddingValues(horizontal = 16.dp),
    ) { Text(text, fontSize = 15.sp) }
}

@Composable
fun Dot(color: Color, size: Int = 8) = Box(Modifier.size(size.dp).clip(CircleShape).background(color))

@Composable
fun Hint(text: String) = Text(
    text, fontSize = 13.sp, lineHeight = 19.sp, color = LocalPalette.current.muted,
    modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
)

val Gap = Arrangement.spacedBy(10.dp)
