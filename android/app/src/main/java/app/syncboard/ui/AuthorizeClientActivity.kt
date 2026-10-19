package app.syncboard.ui

import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.graphics.drawable.toBitmap
import app.syncboard.ipc.AppPrefs
import app.syncboard.ipc.DaemonClient
import app.syncboard.proto.readPlain
import app.syncboard.proto.writePlain
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import kotlin.concurrent.thread

/**
 * Another app (TwinFile) asks to use the phone service: browse the connected PCs' files
 * through it and reach Android/data with its rights. Started with startActivityForResult,
 * so the caller's package is known for certain; the user decides here, and the daemon then
 * accepts connections from that app's uid. Revoked in 设置 → 已授权的应用.
 */
class AuthorizeClientActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val pkg = callingPackage
        if (pkg == null || pkg == packageName) {
            setResult(RESULT_CANCELED)
            return finish()
        }
        val pm = packageManager
        val info = runCatching { pm.getApplicationInfo(pkg, 0) }.getOrNull()
        val label = info?.loadLabel(pm)?.toString() ?: pkg
        val icon = info?.loadIcon(pm)?.toBitmap(144, 144)?.asImageBitmap()
        setContent {
            SyncBoardTheme(AppPrefs.theme(this)) {
                var busy by remember { mutableStateOf(false) }
                ApprovalDialog(label, icon, busy,
                    onAllow = { busy = true; approve(pkg) },
                    onDeny = { setResult(RESULT_CANCELED); finish() })
            }
        }
    }

    private fun approve(pkg: String) {
        thread {
            val error = try {
                val uid = packageManager.getPackageUid(pkg, 0)
                Socket().use { s ->
                    s.connect(InetSocketAddress("127.0.0.1", DaemonClient.PORT), 1000)
                    s.soTimeout = 5000
                    writePlain(DataOutputStream(BufferedOutputStream(s.getOutputStream())),
                        JSONObject().put("cmd", "authorizeClient").put("pkg", pkg).put("uid", uid))
                    val r = readPlain(DataInputStream(BufferedInputStream(s.getInputStream())))
                    if (r.optBoolean("ok")) null else "授权失败"
                }
            } catch (e: java.net.ConnectException) {
                "手机服务未运行，请先打开 SyncBoard 启动服务"
            } catch (e: Exception) {
                e.message ?: "授权失败"
            }
            runOnUiThread {
                if (error != null) Toast.makeText(this, error, Toast.LENGTH_LONG).show()
                setResult(if (error == null) RESULT_OK else RESULT_CANCELED)
                finish()
            }
        }
    }
}

@Composable
private fun ApprovalDialog(
    label: String,
    icon: androidx.compose.ui.graphics.ImageBitmap?,
    busy: Boolean,
    onAllow: () -> Unit,
    onDeny: () -> Unit,
) {
    val p = LocalPalette.current
    AlertDialog(
        onDismissRequest = onDeny,
        containerColor = p.surface,
        shape = RoundedCornerShape(24.dp),
        icon = icon?.let { { Image(it, null, Modifier.size(48.dp).clip(RoundedCornerShape(12.dp))) } },
        title = { Text("允许「$label」使用 SyncBoard？", color = p.ink, fontSize = 19.sp, fontWeight = FontWeight.SemiBold) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Point("浏览电脑上的文件", "通过 SyncBoard 已连接的电脑浏览、复制和管理文件。电脑上需开启「允许已配对设备浏览文件」")
                Point("访问受保护的文件夹", "以 SyncBoard 手机服务的权限读写 Android/data、obb")
                Text("可以随时在 SyncBoard 设置 → 已授权的应用 里撤销。", color = p.muted, fontSize = 13.sp)
            }
        },
        confirmButton = {
            Button(onAllow, enabled = !busy, colors = ButtonDefaults.buttonColors(containerColor = p.ink, contentColor = p.onInk)) {
                Text(if (busy) "正在授权…" else "允许")
            }
        },
        dismissButton = { TextButton(onDeny, enabled = !busy) { Text("拒绝", color = p.ink) } },
    )
}

@Composable
private fun Point(title: String, detail: String) {
    val p = LocalPalette.current
    Row(verticalAlignment = Alignment.Top) {
        Column(Modifier.fillMaxWidth()) {
            Text(title, color = p.ink, fontSize = 15.sp, fontWeight = FontWeight.Medium)
            Spacer(Modifier.height(2.dp))
            Text(detail, color = p.muted, fontSize = 13.sp, lineHeight = 19.sp)
        }
    }
}
