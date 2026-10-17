package app.syncboard.ui

import android.Manifest
import android.app.ActivityManager
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.provider.Settings
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.syncboard.ipc.AppPrefs
import app.syncboard.ipc.DaemonClient
import app.syncboard.ipc.Recovery
import app.syncboard.ipc.WakeInfo
import org.json.JSONObject
import kotlin.concurrent.thread

class MainActivity : ComponentActivity() {
    private lateinit var client: DaemonClient

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        window.isNavigationBarContrastEnforced = false
        client = DaemonClient(applicationContext)
        val scanAtStart = intent?.getBooleanExtra(EXTRA_SCAN, false) == true
        setContent { SyncBoardTheme { App(client, scanAtStart) } }
    }

    override fun onStart() {
        super.onStart()
        client.start()
    }

    override fun onResume() {
        super.onResume()
        applyRecents()
    }

    override fun onStop() {
        super.onStop()
        client.stop()
    }

    fun applyRecents() {
        val hide = AppPrefs.hideFromRecents(this)
        getSystemService(ActivityManager::class.java).appTasks.forEach { runCatching { it.setExcludeFromRecents(hide) } }
    }

    companion object {
        const val EXTRA_SCAN = "scan"
    }
}

private enum class Screen { Home, Scan, Settings }

@Composable
private fun App(client: DaemonClient, scanAtStart: Boolean) {
    val p = LocalPalette.current
    val ctx = LocalContext.current
    val conn by client.conn.collectAsStateWithLifecycle()
    var screen by remember { mutableStateOf(if (scanAtStart) Screen.Scan else Screen.Home) }
    var pairCode by remember { mutableStateOf<Pair<String, String>?>(null) }
    val discovered = remember { mutableStateListOf<JSONObject>() }

    LaunchedEffect(client) {
        client.events.collect { ev ->
            when (ev.optString("event")) {
                "pairCode" -> pairCode = ev.optString("pc") to ev.optString("code")
                "paired" -> {
                    pairCode = null
                    Toast.makeText(ctx, "已与「${ev.optString("name")}」配对", Toast.LENGTH_SHORT).show()
                }
                "pairFailed" -> {
                    pairCode = null
                    Toast.makeText(ctx, ev.optString("reason"), Toast.LENGTH_LONG).show()
                }
                "discovered" -> {
                    discovered.clear()
                    val a = ev.optJSONArray("pcs")
                    for (i in 0 until (a?.length() ?: 0)) discovered.add(a!!.getJSONObject(i))
                }
                "fileDone" -> if (ev.optBoolean("incoming")) {
                    Toast.makeText(ctx, "已收到 ${ev.optString("name")}（下载/SyncBoard）", Toast.LENGTH_SHORT).show()
                }
                "fileFailed" -> Toast.makeText(ctx, "${ev.optString("name")}：${ev.optString("reason")}", Toast.LENGTH_SHORT).show()
            }
        }
    }

    BackHandler(screen != Screen.Home) { screen = Screen.Home }

    Box(Modifier.fillMaxSize().background(p.bg)) {
        when (screen) {
            Screen.Home -> HomeScreen(conn, client, discovered, onScan = { screen = Screen.Scan }, onSettings = { screen = Screen.Settings })
            Screen.Scan -> ScanScreen(onBack = { screen = Screen.Home }) { uri ->
                client.cmd("pairQr", "uri" to uri)
                screen = Screen.Home
                Toast.makeText(ctx, "正在配对…", Toast.LENGTH_SHORT).show()
            }
            Screen.Settings -> SettingsScreen(conn, client, onBack = { screen = Screen.Home })
        }
    }

    pairCode?.let { (pc, code) ->
        AlertDialog(
            onDismissRequest = {},
            title = { Text("在电脑上确认配对") },
            text = {
                Column {
                    Text("请确认「$pc」上显示的数字与下方一致，并在电脑上点「接受」。", color = p.muted)
                    Text(code, fontSize = 34.sp, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold,
                        letterSpacing = 6.sp, modifier = Modifier.padding(top = 18.dp))
                }
            },
            confirmButton = { TextButton({ pairCode = null }) { Text("关闭", color = p.ink) } },
            containerColor = p.surface,
        )
    }
}

// ---------------- home ----------------

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun HomeScreen(
    conn: DaemonClient.Conn,
    client: DaemonClient,
    discovered: List<JSONObject>,
    onScan: () -> Unit,
    onSettings: () -> Unit,
) {
    val p = LocalPalette.current
    val ctx = LocalContext.current
    var showAdd by remember { mutableStateOf(false) }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) sendUris(ctx, uris)
    }

    Column(
        Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp, vertical = 8.dp),
    ) {
        Row(Modifier.fillMaxWidth().height(48.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("SyncBoard", fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = p.muted, modifier = Modifier.weight(1f))
            TextButton(onSettings) { Text("设置", color = p.ink) }
        }

        when (conn) {
            DaemonClient.Conn.Checking -> Spacer(Modifier.height(200.dp))
            DaemonClient.Conn.NotRunning -> ServiceDown()
            is DaemonClient.Conn.Running -> {
                val st = conn.status
                val pcs = st.optJSONArray("pcs")
                val list = (0 until (pcs?.length() ?: 0)).map { pcs!!.getJSONObject(it) }
                val online = list.filter { it.optBoolean("connected") }
                val paused = st.optJSONObject("settings")?.optBoolean("paused") == true

                // status block
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 12.dp)) {
                    Dot(if (online.isNotEmpty()) p.green else if (list.isNotEmpty()) p.amber else p.muted)
                    Spacer(Modifier.width(8.dp))
                    Text(
                        when {
                            online.isNotEmpty() -> "已连接 · ${online[0].optString("addr")}"
                            list.isNotEmpty() -> "正在寻找电脑…"
                            else -> "未配对"
                        },
                        fontSize = 13.sp, color = p.muted, modifier = Modifier.weight(1f),
                    )
                    PausePill(paused) { client.send(JSONObject().put("cmd", "settings").put("settings", JSONObject().put("paused", !paused))) }
                }
                Text(
                    when {
                        online.isNotEmpty() -> online.joinToString("、") { it.optString("name") }
                        list.isNotEmpty() -> "等待电脑上线"
                        else -> "还没有配对的电脑"
                    },
                    style = androidx.compose.material3.MaterialTheme.typography.headlineSmall,
                    maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 10.dp),
                )
                Text(
                    when {
                        paused -> "同步已暂停"
                        online.isNotEmpty() -> "复制的文字和图片会自动出现在电脑上"
                        list.isNotEmpty() -> "确认电脑上的 SyncBoard 正在运行，且两边网络互通"
                        else -> "在电脑上打开 SyncBoard，点「添加手机」→「已安装 App」，然后在这里扫码"
                    },
                    fontSize = 14.sp, color = p.muted, lineHeight = 20.sp, modifier = Modifier.padding(top = 4.dp),
                )
                Row(Modifier.fillMaxWidth().padding(top = 20.dp), horizontalArrangement = Gap) {
                    InkButton("发送文件", Modifier.weight(1f), enabled = online.isNotEmpty()) { pickFiles.launch(arrayOf("*/*")) }
                    LineButton(if (list.isEmpty()) "扫码配对" else "添加电脑", Modifier.weight(1f)) {
                        if (list.isEmpty()) onScan() else showAdd = true
                    }
                }

                SectionLabel("电脑")
                Group {
                    if (list.isEmpty()) Hint("没有电脑")
                    list.forEachIndexed { i, pc ->
                        if (i > 0) Divider()
                        PcRow(pc) { client.cmd("unpair", "id" to pc.optString("id")) }
                    }
                }
            }
        }
        Spacer(Modifier.height(24.dp))
    }

    if (showAdd) {
        LaunchedEffect(Unit) { client.cmd("discover") }
        ModalBottomSheet(onDismissRequest = { showAdd = false }, containerColor = p.surface) {
            AddPcSheet(discovered, onScan = { showAdd = false; onScan() }) { host ->
                showAdd = false
                client.cmd("pairCode", "host" to host)
            }
        }
    }
}

@Composable
private fun PausePill(paused: Boolean, onClick: () -> Unit) {
    val p = LocalPalette.current
    Row(
        Modifier.clip(RoundedCornerShape(50)).background(p.surface).clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Dot(if (paused) p.muted else p.green, 7)
        Spacer(Modifier.width(6.dp))
        Text(if (paused) "已暂停" else "同步中", fontSize = 12.sp, color = p.ink)
    }
}

@Composable
private fun PcRow(pc: JSONObject, onRemove: () -> Unit) {
    val p = LocalPalette.current
    var confirm by remember { mutableStateOf(false) }
    val online = pc.optBoolean("connected")
    Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 6.dp, top = 10.dp, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(pc.optString("name"), fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Row(verticalAlignment = Alignment.CenterVertically) {
                Dot(if (online) p.green else p.muted, 6)
                Spacer(Modifier.width(6.dp))
                val state = when (pc.optString("state")) {
                    "connected" -> "在线"
                    "rejected" -> "电脑已移除此手机，请重新配对"
                    else -> "离线"
                }
                val host = if (online) pc.optString("addr") else pc.optJSONArray("hosts")?.optString(0).orEmpty()
                Text(if (host.isNotEmpty() && state != "电脑已移除此手机，请重新配对") "$state · $host" else state, fontSize = 12.sp, color = p.muted)
            }
        }
        TextButton({ confirm = true }) { Text("移除", color = p.muted) }
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("移除「${pc.optString("name")}」？") },
            text = { Text("之后需要重新扫码配对。", color = p.muted) },
            confirmButton = { TextButton({ confirm = false; onRemove() }) { Text("移除", color = p.danger) } },
            dismissButton = { TextButton({ confirm = false }) { Text("取消", color = p.ink) } },
            containerColor = p.surface,
        )
    }
}

@Composable
private fun AddPcSheet(discovered: List<JSONObject>, onScan: () -> Unit, onManual: (String) -> Unit) {
    val p = LocalPalette.current
    var ip by remember { mutableStateOf("") }
    Column(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 28.dp)) {
        Text("添加电脑", style = androidx.compose.material3.MaterialTheme.typography.titleMedium)
        Spacer(Modifier.height(16.dp))
        InkButton("扫码配对", Modifier.fillMaxWidth(), onClick = onScan)
        if (discovered.isNotEmpty()) {
            SectionLabel("局域网中的电脑")
            Group {
                discovered.forEachIndexed { i, pc ->
                    if (i > 0) Divider()
                    ActionRow(pc.optString("name"), pc.optString("addr")) { onManual(pc.optString("addr")) }
                }
            }
        }
        SectionLabel("手动输入电脑 IP")
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Gap) {
            OutlinedTextField(
                ip, { ip = it.trim() }, Modifier.weight(1f), singleLine = true,
                placeholder = { Text("例如 192.168.1.10", color = p.muted) },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                shape = RoundedCornerShape(10.dp),
                colors = OutlinedTextFieldDefaults.colors(focusedBorderColor = p.ink, unfocusedBorderColor = p.line, cursorColor = p.ink),
            )
            LineButton("连接", enabled = ip.isNotEmpty()) { onManual(ip) }
        }
    }
}

/** Shown when the daemon isn't running: restart it (after a reboot) or explain setup. */
@Composable
private fun ServiceDown() {
    val p = LocalPalette.current
    val ctx = LocalContext.current
    val known = remember { WakeInfo.load(ctx) != null }
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }

    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 12.dp)) {
        Dot(p.amber)
        Spacer(Modifier.width(8.dp))
        Text("手机服务未运行", fontSize = 13.sp, color = p.muted)
    }
    Text(
        if (known) "服务需要重新启动" else "先在电脑上完成设置",
        style = androidx.compose.material3.MaterialTheme.typography.headlineSmall, modifier = Modifier.padding(top = 10.dp),
    )
    Text(
        if (known) {
            "手机重启后，后台同步服务会停止。SyncBoard 可以通过「无线调试」请电脑重新启动它，不需要数据线。"
        } else {
            "SyncBoard 的后台服务需要电脑帮忙启动一次：在电脑上打开 SyncBoard，点「添加手机」，按提示用无线调试扫码或插上数据线，其余步骤会自动完成。"
        },
        fontSize = 14.sp, color = p.muted, lineHeight = 21.sp, modifier = Modifier.padding(top = 6.dp),
    )
    if (known) {
        Spacer(Modifier.height(20.dp))
        InkButton(if (busy) "正在启动…" else "一键启动", Modifier.fillMaxWidth(), enabled = !busy) {
            busy = true
            message = null
            thread {
                val r = Recovery.run(ctx.applicationContext)
                busy = false
                message = when (r) {
                    Recovery.Result.Started -> null
                    Recovery.Result.NoPc -> "还没有配对的电脑"
                    Recovery.Result.NoWifiDebugging -> "请先在开发者选项中打开「无线调试」，再点一键启动。"
                    is Recovery.Result.Failed -> r.reason
                }
            }
        }
        if (!Recovery.canToggleWifiDebugging(ctx)) {
            Spacer(Modifier.height(10.dp))
            LineButton("打开开发者选项", Modifier.fillMaxWidth()) {
                runCatching { ctx.startActivity(Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS)) }
            }
        }
        message?.let { Text(it, color = p.danger, fontSize = 13.sp, modifier = Modifier.padding(top = 12.dp)) }
    }
    SectionLabel("或者")
    Group {
        Hint("插上数据线并打开 USB 调试，电脑上的 SyncBoard 会自动启动手机服务。")
    }
}

// ---------------- scan ----------------

@Composable
private fun ScanScreen(onBack: () -> Unit, onResult: (String) -> Unit) {
    val p = LocalPalette.current
    val ctx = LocalContext.current
    var granted by remember { mutableStateOf(ctx.checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }
    LaunchedEffect(Unit) { if (!granted) ask.launch(Manifest.permission.CAMERA) }

    Box(Modifier.fillMaxSize().background(androidx.compose.ui.graphics.Color.Black)) {
        if (granted) {
            QrScanner(Modifier.fillMaxSize(), accept = { it.startsWith("syncboard://pair") }, onResult = onResult)
        }
        Box(Modifier.align(Alignment.Center).size(240.dp).clip(RoundedCornerShape(20.dp))
            .background(androidx.compose.ui.graphics.Color.White.copy(alpha = 0.08f)))
        Column(Modifier.fillMaxWidth().statusBarsPadding().padding(16.dp)) {
            TextButton(onBack) { Text("‹ 返回", color = androidx.compose.ui.graphics.Color.White, fontSize = 16.sp) }
        }
        Text(
            if (granted) "扫描电脑 SyncBoard「添加手机 → 已安装 App」中的二维码" else "需要相机权限才能扫码",
            color = androidx.compose.ui.graphics.Color.White, fontSize = 14.sp,
            modifier = Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 48.dp, start = 24.dp, end = 24.dp),
        )
    }
}

// ---------------- settings ----------------

@Composable
private fun SettingsScreen(conn: DaemonClient.Conn, client: DaemonClient, onBack: () -> Unit) {
    val p = LocalPalette.current
    val ctx = LocalContext.current
    val activity = ctx as? MainActivity
    val s = (conn as? DaemonClient.Conn.Running)?.status?.optJSONObject("settings")
    fun set(key: String, v: Boolean) = client.send(JSONObject().put("cmd", "settings").put("settings", JSONObject().put(key, v)))
    var hideRecents by remember { mutableStateOf(AppPrefs.hideFromRecents(ctx)) }
    var autoRecover by remember { mutableStateOf(AppPrefs.autoRecover(ctx)) }
    var confirmStop by remember { mutableStateOf(false) }

    Column(
        Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp, vertical = 8.dp),
    ) {
        Row(Modifier.fillMaxWidth().height(48.dp), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onBack) { Text("‹ 返回", color = p.ink) }
        }
        Text("设置", style = androidx.compose.material3.MaterialTheme.typography.headlineSmall, modifier = Modifier.padding(start = 4.dp))

        if (s != null) {
            SectionLabel("同步")
            Group {
                ToggleRow("文本", s.optBoolean("syncText")) { set("syncText", it) }
                Divider()
                ToggleRow("图片", s.optBoolean("syncImages")) { set("syncImages", it) }
                Divider()
                ToggleRow("自动接收文件", s.optBoolean("autoAcceptFiles"), "保存到 下载/SyncBoard") { set("autoAcceptFiles", it) }
                Divider()
                ToggleRow("隐藏「已粘贴」提示", s.optBoolean("hideAccessToast"),
                    "Android 12+ 读取剪贴板时系统会弹提示；这是系统级开关") { set("hideAccessToast", it) }
            }
        }

        SectionLabel("后台")
        Group {
            ToggleRow("从最近任务中隐藏", hideRecents) {
                hideRecents = it
                AppPrefs.setHideFromRecents(ctx, it)
                activity?.applyRecents()
            }
            Divider()
            ToggleRow(
                "重启后自动恢复服务", autoRecover,
                if (Recovery.canToggleWifiDebugging(ctx)) "开机后临时打开无线调试，请电脑重新启动服务，完成后自动关闭"
                else "需要先用电脑完成一次设置",
                enabled = Recovery.canToggleWifiDebugging(ctx),
            ) { autoRecover = it; AppPrefs.setAutoRecover(ctx, it) }
        }

        if (s != null) {
            SectionLabel("服务")
            Group {
                ActionRow("停止手机服务", "停止后台同步，直到下次由电脑或「一键启动」重新启动", p.danger) { confirmStop = true }
            }
        }
        Text("SyncBoard ${appVersion(ctx)}", fontSize = 12.sp, color = p.muted, modifier = Modifier.padding(top = 24.dp, start = 4.dp))
        Spacer(Modifier.height(24.dp))
    }

    if (confirmStop) {
        AlertDialog(
            onDismissRequest = { confirmStop = false },
            title = { Text("停止手机服务？") },
            text = { Text("剪贴板和文件将不再同步。", color = p.muted) },
            confirmButton = { TextButton({ confirmStop = false; client.cmd("stop") }) { Text("停止", color = p.danger) } },
            dismissButton = { TextButton({ confirmStop = false }) { Text("取消", color = p.ink) } },
            containerColor = p.surface,
        )
    }
}

private fun appVersion(ctx: android.content.Context) =
    runCatching { ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName }.getOrNull().orEmpty()

/** Sends picked or shared files to the PC through the daemon, one at a time. */
fun sendUris(ctx: android.content.Context, uris: List<Uri>) {
    val app = ctx.applicationContext
    thread {
        var ok = 0
        for (uri in uris) {
            val (name, size) = app.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
                ?.use { c -> if (c.moveToFirst()) (c.getString(0) ?: "file") to c.getLong(1) else null } ?: ("file" to -1L)
            if (size < 0) continue
            val sent = app.contentResolver.openInputStream(uri)?.use { DaemonClient.sendFile(app, name, size, it) } ?: false
            if (sent) ok++
        }
        android.os.Handler(android.os.Looper.getMainLooper()).post {
            Toast.makeText(app, if (ok == uris.size) "已发送 $ok 个文件" else "发送完成 $ok/${uris.size}，请确认电脑在线", Toast.LENGTH_SHORT).show()
        }
    }
}
