package app.syncboard.ui

import android.Manifest
import android.app.ActivityManager
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
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
import kotlinx.coroutines.launch
import kotlin.concurrent.thread

class MainActivity : ComponentActivity() {
    private lateinit var client: DaemonClient

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.isNavigationBarContrastEnforced = false
        client = DaemonClient(applicationContext)
        val scanAtStart = intent?.getBooleanExtra(EXTRA_SCAN, false) == true
        setContent {
            var mode by remember { mutableStateOf(AppPrefs.theme(this)) }
            val dark = isDark(mode)
            // Status/navigation bar icons must follow the app theme, not the system one.
            LaunchedEffect(dark) {
                val bar = if (dark) {
                    androidx.activity.SystemBarStyle.dark(android.graphics.Color.TRANSPARENT)
                } else {
                    androidx.activity.SystemBarStyle.light(android.graphics.Color.TRANSPARENT, android.graphics.Color.TRANSPARENT)
                }
                enableEdgeToEdge(bar, bar)
            }
            SyncBoardTheme(mode) {
                App(client, scanAtStart, mode) { mode = it; AppPrefs.setTheme(this, it) }
            }
        }
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

/** A newer release found at startup (or by 检查更新), shared by the home banner and settings. */
private object Updates {
    var latest by mutableStateOf<String?>(null)
    /** The home banner was tapped: settings opens the update dialog right away. */
    var askOnOpen by mutableStateOf(false)
    @Volatile var checked = false

    /** Once per process, quietly; being offline is not an error worth showing. */
    fun checkOnce(current: String) {
        if (checked) return
        checked = true
        thread {
            runCatching { app.syncboard.ipc.Releases.latestVersion() }.onSuccess { v ->
                if (app.syncboard.ipc.Releases.isNewer(v, current)) latest = v
            }
        }
    }
}

@Composable
private fun App(client: DaemonClient, scanAtStart: Boolean, theme: String, setTheme: (String) -> Unit) {
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
                "unpairedByPc" -> Toast.makeText(ctx, "「${ev.optString("name")}」已移除这台手机", Toast.LENGTH_LONG).show()
                "fileFailed" -> Toast.makeText(ctx, "${ev.optString("name")}：${ev.optString("reason")}", Toast.LENGTH_SHORT).show()
            }
        }
    }

    LaunchedEffect(Unit) { Updates.checkOnce(appVersion(ctx)) }

    BackHandler(screen != Screen.Home) { screen = Screen.Home }

    Box(Modifier.fillMaxSize().background(p.bg)) {
        when (screen) {
            Screen.Home -> HomeScreen(conn, client, discovered, onScan = { screen = Screen.Scan }, onSettings = { screen = Screen.Settings })
            Screen.Scan -> ScanScreen(onBack = { screen = Screen.Home }) { uri ->
                client.cmd("pairQr", "uri" to uri)
                screen = Screen.Home
                Toast.makeText(ctx, "正在配对…", Toast.LENGTH_SHORT).show()
            }
            Screen.Settings -> SettingsScreen(conn, client, theme, setTheme, onBack = { screen = Screen.Home })
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
        Updates.latest?.let { v ->
            Row(
                Modifier.fillMaxWidth().padding(top = 4.dp).clip(RoundedCornerShape(12.dp)).background(p.surface)
                    .clickable { Updates.askOnOpen = true; onSettings() }.padding(horizontal = 14.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Dot(p.green, 7)
                Spacer(Modifier.width(8.dp))
                Text("有新版本 v$v", fontSize = 14.sp, color = p.ink, modifier = Modifier.weight(1f))
                Text("更新 ›", fontSize = 14.sp, color = p.ink, fontWeight = FontWeight.SemiBold)
            }
        }

        when (conn) {
            DaemonClient.Conn.Checking -> Spacer(Modifier.height(200.dp))
            DaemonClient.Conn.NotRunning -> ServiceDown()
            is DaemonClient.Conn.Running -> {
                val st = conn.status
                val pcs = st.optJSONArray("pcs")
                val list = (0 until (pcs?.length() ?: 0)).map { pcs!!.getJSONObject(it) }
                val online = list.filter { it.optBoolean("connected") }
                // Every paired PC refused us: waiting won't help, it has to be paired again.
                val repair = list.isNotEmpty() && online.isEmpty() &&
                    list.all { it.optString("state") in setOf("rejected", "keyMismatch") }
                val paused = st.optJSONObject("settings")?.optBoolean("paused") == true

                // status block
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 12.dp)) {
                    Dot(if (online.isNotEmpty()) p.green else if (repair) p.danger else if (list.isNotEmpty()) p.amber else p.muted)
                    Spacer(Modifier.width(8.dp))
                    Text(
                        when {
                            online.isNotEmpty() -> "已连接 · ${online[0].optString("addr")}"
                            repair -> "需要重新配对"
                            list.isNotEmpty() -> "正在寻找电脑…"
                            else -> "未配对"
                        },
                        fontSize = 13.sp, color = p.muted, modifier = Modifier.weight(1f),
                    )
                    // A pause switch, not a status: nothing to pause until a PC is paired.
                    if (list.isNotEmpty()) PausePill(paused) { client.send(JSONObject().put("cmd", "settings").put("settings", JSONObject().put("paused", !paused))) }
                }
                Text(
                    when {
                        online.isNotEmpty() -> online.joinToString("、") { it.optString("name") }
                        repair -> "请重新配对电脑"
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
                        repair -> "电脑上已没有这台手机的配对。先「移除」下面的电脑，再在电脑 SyncBoard 点「添加手机」，用这里的「扫码配对」扫码"
                        list.isNotEmpty() -> "确认电脑上的 SyncBoard 正在运行，且两边网络互通"
                        else -> "在电脑 SyncBoard 点「添加手机」进入第 2 步，然后点这里的「扫码配对」扫那个二维码"
                    },
                    fontSize = 14.sp, color = p.muted, lineHeight = 20.sp, modifier = Modifier.padding(top = 4.dp),
                )
                Row(Modifier.fillMaxWidth().padding(top = 20.dp), horizontalArrangement = Gap) {
                    InkButton("发送文件", Modifier.weight(1f), enabled = online.isNotEmpty()) { pickFiles.launch(arrayOf("*/*")) }
                    LineButton(if (list.isEmpty() || repair) "扫码配对" else "添加电脑", Modifier.weight(1f)) {
                        if (list.isEmpty() || repair) onScan() else showAdd = true
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
        Text(if (paused) "同步已暂停" else "同步已开启", fontSize = 12.sp, color = p.ink)
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
                val (state, needsRepair) = when (pc.optString("state")) {
                    "connected" -> "在线" to false
                    "rejected" -> "电脑已移除此手机，请重新配对" to true
                    "keyMismatch" -> "配对信息不一致，请重新配对" to true
                    else -> "离线" to false
                }
                val host = if (online) pc.optString("addr") else pc.optJSONArray("hosts")?.optString(0).orEmpty()
                Text(if (host.isNotEmpty() && !needsRepair) "$state · $host" else state, fontSize = 12.sp, color = p.muted)
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

/**
 * Shown when the daemon isn't running: restart it (after a reboot) through a paired PC,
 * or walk through setting the phone up on the PC again.
 */
@Composable
private fun ServiceDown() {
    val p = LocalPalette.current
    val ctx = LocalContext.current
    var known by remember { mutableStateOf(WakeInfo.load(ctx) != null) }
    // Set up once over adb: the app can switch the debugging options itself, so starting
    // the service needs no taps at all.
    val oneTap = known && Recovery.canToggleWifiDebugging(ctx)
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    var showSetup by remember { mutableStateOf(false) }

    fun start() {
        busy = true
        message = null
        thread {
            val r = Recovery.run(ctx.applicationContext)
            busy = false
            when (r) {
                Recovery.Result.Started -> {}
                Recovery.Result.NoPc, Recovery.Result.Forgotten -> {
                    known = false
                    message = if (r == Recovery.Result.Forgotten) "电脑已移除这台手机，请按下面的步骤重新添加。" else null
                }
                Recovery.Result.NoWifiDebugging -> message = "请先打开「无线调试」（见下方），再点一键启动。"
                is Recovery.Result.Failed -> message = r.reason
            }
        }
    }
    LaunchedEffect(oneTap) { if (oneTap && !busy && !AppPrefs.userStopped(ctx)) start() }
    // Needs pairing: have the port reporter ready whichever way the user reaches the scanner.
    LaunchedEffect(oneTap) { if (!oneTap) app.syncboard.ipc.PairHelper.start(ctx.applicationContext) }

    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 12.dp)) {
        Dot(p.amber)
        Spacer(Modifier.width(8.dp))
        Text("手机服务未运行", fontSize = 13.sp, color = p.muted)
    }
    Text(
        when {
            oneTap && busy -> "正在启动手机服务…"
            known -> "服务需要重新启动"
            else -> "在电脑上添加这台手机"
        },
        style = androidx.compose.material3.MaterialTheme.typography.headlineSmall, modifier = Modifier.padding(top = 10.dp),
    )
    Text(
        when {
            oneTap -> "SyncBoard 会临时打开无线调试，请电脑启动后台服务${if (AppPrefs.autoDisableDev(ctx)) "，完成后关闭开发者选项" else ""}。需要电脑上的 SyncBoard 正在运行，并和手机在同一网络。"
            known -> "手机重启后，后台同步服务会停止。SyncBoard 可以通过「无线调试」请电脑重新启动它，不需要数据线。"
            Recovery.canToggleWifiDebugging(ctx) && !showSetup ->
                "在电脑 SyncBoard 点「添加手机」进入第 2 步「扫码连接」，再点下面的「去扫码」，在打开的页面点「使用二维码配对设备」扫码。无线调试会自动打开${if (AppPrefs.autoDisableDev(ctx)) "，完成后自动关闭" else ""}。"
            else -> "SyncBoard 的后台服务需要电脑帮忙启动一次：在电脑上打开 SyncBoard，点「添加手机」进入第 2 步「扫码连接」，再按下面的步骤操作，其余步骤会自动完成。"
        },
        fontSize = 14.sp, color = p.muted, lineHeight = 21.sp, modifier = Modifier.padding(top = 6.dp),
    )
    if (known) {
        Spacer(Modifier.height(20.dp))
        InkButton(if (busy) "正在启动…" else if (oneTap && message != null) "重试" else "一键启动", Modifier.fillMaxWidth(), enabled = !busy) { start() }
    }
    message?.let { Text(it, color = p.danger, fontSize = 13.sp, modifier = Modifier.padding(top = 12.dp)) }

    if (oneTap && !showSetup) {
        // The checklist is only for pairing again; keep it out of the way.
        TextButton({ showSetup = true }, modifier = Modifier.padding(top = 12.dp)) { Text("重新配对 ›", color = p.muted) }
    } else if (!known && Recovery.canToggleWifiDebugging(ctx) && !showSetup) {
        // Set up over adb before (then removed from the PC): the app flips the debugging
        // switches itself, so only the scan is left.
        QuickRepair(onMore = { showSetup = true })
    } else {
        SectionLabel(if (known) "或者重新添加" else "手机上的准备")
        PhoneSetupSteps()
    }
}

/**
 * Adding the phone again when the app may still write secure settings: turn developer
 * options and wireless debugging on directly and go straight to the QR scanner page.
 */
@Composable
private fun QuickRepair(onMore: () -> Unit) {
    val p = LocalPalette.current
    val ctx = LocalContext.current
    var bg by remember { mutableStateOf(DevOptions.backgroundAllowed(ctx)) }
    var error by remember { mutableStateOf<String?>(null) }
    androidx.lifecycle.compose.LifecycleResumeEffect(Unit) {
        bg = DevOptions.backgroundAllowed(ctx)
        DevOptions.dismissGuide(ctx)
        onPauseOrDispose {}
    }
    if (!bg) {
        Spacer(Modifier.height(16.dp))
        Group {
            StepRow(
                "允许后台运行",
                if (DevOptions.isVivo()) "扫码时 SyncBoard 要在后台告诉电脑配对端口：在「后台耗电管理」里选「允许后台耗电」"
                else "扫码时 SyncBoard 要在后台告诉电脑配对端口",
                false, action = "去设置",
            ) { DevOptions.openBackgroundSetting(ctx) }
        }
    }
    Spacer(Modifier.height(20.dp))
    InkButton("去扫码", Modifier.fillMaxWidth()) {
        error = null
        if (DevOptions.enableDeveloperOptions(ctx) && DevOptions.enableWirelessDebugging(ctx)) {
            DevOptions.openWirelessDebuggingPage(ctx)
        } else {
            error = "无法自动打开无线调试，请按「其他方式」里的步骤手动打开"
        }
    }
    error?.let { Text(it, color = p.danger, fontSize = 13.sp, modifier = Modifier.padding(top = 12.dp)) }
    TextButton(onMore, modifier = Modifier.padding(top = 12.dp)) { Text("其他方式（数据线、手动设置）›", color = p.muted) }
}

/**
 * Developer-option checklist with shortcuts that jump straight to the switch to turn on,
 * so pairing (or re-pairing) with the PC takes a few taps.
 */
@Composable
private fun PhoneSetupSteps() {
    val p = LocalPalette.current
    val ctx = LocalContext.current
    var devState by remember { mutableStateOf(DevOptions.devState(ctx)) }
    val dev = devState == DevOptions.DevState.On
    var wifi by remember { mutableStateOf(DevOptions.wirelessDebuggingOn(ctx)) }
    var usb by remember { mutableStateOf(DevOptions.usbDebuggingOn(ctx)) }
    var bg by remember { mutableStateOf(DevOptions.backgroundAllowed(ctx)) }
    // Re-read when coming back from Settings.
    androidx.lifecycle.compose.LifecycleResumeEffect(Unit) {
        devState = DevOptions.devState(ctx)
        wifi = DevOptions.wirelessDebuggingOn(ctx)
        usb = DevOptions.usbDebuggingOn(ctx)
        bg = DevOptions.backgroundAllowed(ctx)
        DevOptions.dismissGuide(ctx)
        onPauseOrDispose {}
    }
    // The guide banner needs the notification permission; ask once, then jump either way.
    var pending by remember { mutableStateOf<(() -> Unit)?>(null) }
    val askNotify = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        pending?.invoke()
        pending = null
    }
    fun go(open: () -> Unit) {
        if (DevOptions.needsNotificationPermission(ctx) && !AppPrefs.askedNotify(ctx)) {
            AppPrefs.setAskedNotify(ctx)
            pending = open
            askNotify.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            open()
        }
    }
    Group {
        val hidden = devState == DevOptions.DevState.Hidden
        StepRow(
            "1. 开发者选项",
            when (devState) {
                DevOptions.DevState.Hidden -> "在「版本号」上连续点击 7 次解锁"
                DevOptions.DevState.Off -> "已解锁，再打开「开发者选项」开关"
                DevOptions.DevState.On -> "已解锁并打开"
            },
            dev, action = if (hidden) "去解锁" else "去打开",
        ) {
            // Already set up once over adb: just flip it. Otherwise walk through Settings.
            if (DevOptions.enableDeveloperOptions(ctx)) {
                devState = DevOptions.devState(ctx)
            } else if (hidden) {
                go { DevOptions.openBuildNumber(ctx) }
            } else {
                go { DevOptions.openDeveloperOptionsSwitch(ctx) }
            }
        }
        // Android 10 has no wireless debugging: only the cable works there.
        if (android.os.Build.VERSION.SDK_INT < 30) {
            Divider()
            Hint("Android 10 不支持无线调试，请用下方的数据线方式。")
        } else {
        Divider()
        StepRow("2. 无线调试", "手机和电脑连同一个 Wi-Fi", wifi, enabled = dev) {
            if (DevOptions.enableWirelessDebugging(ctx)) wifi = true
            else go { DevOptions.openWirelessDebuggingToTurnOn(ctx) }
        }
        Divider()
        StepRow(
            "3. 允许后台运行",
            if (DevOptions.isVivo()) "扫码时 SyncBoard 要在后台告诉电脑配对端口：在「后台耗电管理」里选「允许后台耗电」"
            else "扫码时 SyncBoard 要在后台告诉电脑配对端口",
            bg, action = "去设置",
        ) { go { DevOptions.openBackgroundSetting(ctx) } }
        Divider()
        ActionRow(
            "4. 扫描电脑上的二维码",
            "电脑 SyncBoard 点「添加手机」→ 第 2 步「扫码连接」，然后在这里点「使用二维码配对设备」扫码",
            if (dev && wifi) p.ink else p.muted,
        ) { go { DevOptions.openWirelessDebuggingPage(ctx) } }
        }
    }
    SectionLabel("或者用数据线")
    Group {
        StepRow("USB 调试", "电脑「添加手机」第 2 步点「改用数据线」，插上线并在手机弹窗中点「允许」", usb, enabled = dev) {
            go { DevOptions.openUsbDebuggingSwitch(ctx) }
        }
    }
}

@Composable
private fun StepRow(title: String, subtitle: String, done: Boolean, enabled: Boolean = true, action: String = "去开启", onOpen: () -> Unit) {
    val p = LocalPalette.current
    Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 10.dp, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, fontSize = 15.sp, color = if (enabled || done) p.ink else p.muted)
            Text(subtitle, fontSize = 12.sp, color = p.muted, lineHeight = 16.sp)
        }
        if (done) {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(horizontal = 8.dp)) {
                Dot(p.green, 7)
                Spacer(Modifier.width(6.dp))
                Text("已开启", fontSize = 13.sp, color = p.muted)
            }
        } else {
            TextButton(onOpen, enabled = enabled) { Text(action, color = if (enabled) p.ink else p.muted, fontWeight = FontWeight.SemiBold) }
        }
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
            QrScanner(
                Modifier.fillMaxSize(),
                // The PC's wireless-debugging QR also carries its addresses and a pairing token.
                accept = { it.startsWith("syncboard://pair") || app.syncboard.ipc.AdbQr.parse(it) != null },
                onResult = { onResult(app.syncboard.ipc.AdbQr.parse(it)?.let(app.syncboard.ipc.AdbQr::toPairUri) ?: it) },
            )
        }
        Box(Modifier.align(Alignment.Center).size(240.dp).clip(RoundedCornerShape(20.dp))
            .background(androidx.compose.ui.graphics.Color.White.copy(alpha = 0.08f)))
        Column(Modifier.fillMaxWidth().statusBarsPadding().padding(16.dp)) {
            TextButton(onBack) { Text("‹ 返回", color = androidx.compose.ui.graphics.Color.White, fontSize = 16.sp) }
        }
        Text(
            if (granted) "扫描电脑 SyncBoard「添加手机」第 2 步的二维码" else "需要相机权限才能扫码",
            color = androidx.compose.ui.graphics.Color.White, fontSize = 14.sp,
            modifier = Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 48.dp, start = 24.dp, end = 24.dp),
        )
    }
}

// ---------------- settings ----------------

@Composable
private fun SettingsScreen(
    conn: DaemonClient.Conn,
    client: DaemonClient,
    theme: String,
    setTheme: (String) -> Unit,
    onBack: () -> Unit,
) {
    val p = LocalPalette.current
    val ctx = LocalContext.current
    val activity = ctx as? MainActivity
    val s = (conn as? DaemonClient.Conn.Running)?.status?.optJSONObject("settings")
    fun set(key: String, v: Boolean) = client.send(JSONObject().put("cmd", "settings").put("settings", JSONObject().put(key, v)))
    var hideRecents by remember { mutableStateOf(AppPrefs.hideFromRecents(ctx)) }
    var autoRecover by remember { mutableStateOf(AppPrefs.autoRecover(ctx)) }
    var autoDisableDev by remember { mutableStateOf(AppPrefs.autoDisableDev(ctx)) }
    var confirmStop by remember { mutableStateOf(false) }
    var pickFolders by remember { mutableStateOf(false) }
    val scroll = rememberScrollState()
    // Opened from the home banner: show the update row (its progress) at the bottom.
    LaunchedEffect(scroll.maxValue) { if (Updates.askOnOpen && scroll.maxValue > 0) scroll.scrollTo(scroll.maxValue) }

    Column(
        Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().verticalScroll(scroll)
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
                ToggleRow("截屏后自动复制", s.optBoolean("copyScreenshots"),
                    "新截图会放进手机和电脑剪贴板，可直接粘贴") { set("copyScreenshots", it) }
                Divider()
                ToggleRow("拍照后自动复制", s.optBoolean("copyCameraPhotos"),
                    "相机拍的新照片（DCIM/Camera）会放进手机和电脑剪贴板") { set("copyCameraPhotos", it) }
                Divider()
                val folders = s.optJSONArray("copyFolders")?.let { a -> List(a.length()) { a.getString(it) } }.orEmpty()
                ActionRow(
                    "自动复制的文件夹",
                    if (folders.isEmpty()) "未选择。选中文件夹里的新图片也会自动复制"
                    else folders.joinToString("、") { it.substringAfterLast('/') },
                ) { pickFolders = true }
                Divider()
                ToggleRow("自动接收文件", s.optBoolean("autoAcceptFiles"), "保存到 下载/SyncBoard") { set("autoAcceptFiles", it) }
                Divider()
                ToggleRow("隐藏「已粘贴」提示", s.optBoolean("hideAccessToast"),
                    "Android 12+ 读取剪贴板时系统会弹提示；这是系统级开关") { set("hideAccessToast", it) }
            }

            SectionLabel("短信与通知")
            Group {
                ToggleRow("验证码同步到电脑", s.optBoolean("syncSmsCodes", true),
                    "收到验证码短信时，只把验证码发到电脑并复制，短信内容不会发送") { set("syncSmsCodes", it) }
                Divider()
                ToggleRow("新短信在电脑上提醒", s.optBoolean("syncSms"),
                    "每条新短信（发件人和内容）都会发到电脑，用 Windows 通知显示") { set("syncSms", it) }
                Divider()
                ToggleRow("App 通知同步到电脑", s.optBoolean("syncNotifications"),
                    "微信、QQ 等 App 的新通知用 Windows 通知显示，5G 消息里的验证码也能同步。开启后会多一个约 11 MB 的通知接收进程") { set("syncNotifications", it) }
            }
        }

        SectionLabel("外观")
        Group { ThemePicker(theme, setTheme) }

        SectionLabel("后台")
        Group {
            ToggleRow("从最近任务中隐藏", hideRecents) {
                hideRecents = it
                AppPrefs.setHideFromRecents(ctx, it)
                activity?.applyRecents()
            }
            Divider()
            ToggleRow(
                "启动后关闭开发者选项", autoDisableDev,
                "系统不建议一直开着。需要启动服务时，SyncBoard 会临时打开再关闭（需要先用电脑完成一次设置）",
                enabled = Recovery.canToggleWifiDebugging(ctx),
            ) {
                autoDisableDev = it
                AppPrefs.setAutoDisableDev(ctx, it)
                set("autoDisableDevOptions", it)
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
        SectionLabel("关于")
        Group { UpdateRow(client, running = s != null) }
        Spacer(Modifier.height(24.dp))
    }

    if (pickFolders && s != null) {
        val chosen = s.optJSONArray("copyFolders")?.let { a -> List(a.length()) { a.getString(it) } }.orEmpty()
        FolderPicker(client, chosen, onDismiss = { pickFolders = false }) { picked ->
            pickFolders = false
            client.send(JSONObject().put("cmd", "settings").put("settings", JSONObject().put("copyFolders", org.json.JSONArray(picked))))
        }
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

/** Picks folders whose new images are copied, from the folders the phone service finds images in. */
@Composable
private fun FolderPicker(client: DaemonClient, chosen: List<String>, onDismiss: () -> Unit, onDone: (List<String>) -> Unit) {
    val p = LocalPalette.current
    var folders by remember { mutableStateOf<List<Pair<String, Int>>?>(null) }
    val picked = remember { mutableStateListOf<String>().apply { addAll(chosen) } }
    LaunchedEffect(client) {
        client.cmd("imageFolders")
        client.events.collect { ev ->
            if (ev.optString("event") != "imageFolders") return@collect
            val a = ev.optJSONArray("folders") ?: return@collect
            folders = List(a.length()) { a.getJSONObject(it).let { o -> o.optString("path") to o.optInt("count") } }
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("自动复制的文件夹") },
        text = {
            val list = folders
            Column(Modifier.verticalScroll(rememberScrollState())) {
                Text("选中文件夹里新增的图片会自动放进手机和电脑剪贴板。", color = p.muted, fontSize = 13.sp,
                    modifier = Modifier.padding(bottom = 8.dp))
                when {
                    list == null -> Text("正在读取…", color = p.muted)
                    list.isEmpty() && picked.isEmpty() -> Text("没有找到图片文件夹", color = p.muted)
                }
                // Keep chosen folders listed even when they hold no images right now.
                val rows = (list.orEmpty() + picked.filter { c -> list.orEmpty().none { it.first == c } }.map { it to 0 })
                rows.forEach { (path, n) ->
                    val on = path in picked
                    Row(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp))
                            .clickable { if (on) picked.remove(path) else picked.add(path) }.padding(vertical = 4.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        androidx.compose.material3.Checkbox(on, { if (on) picked.remove(path) else picked.add(path) },
                            colors = androidx.compose.material3.CheckboxDefaults.colors(checkedColor = p.ink, checkmarkColor = p.bg))
                        Column(Modifier.weight(1f)) {
                            Text(path.substringAfterLast('/'), fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(if (n > 0) "$path · $n 张" else path, fontSize = 12.sp, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton({ onDone(picked.toList()) }) { Text("完成", color = p.ink) } },
        dismissButton = { TextButton(onDismiss) { Text("取消", color = p.muted) } },
        containerColor = p.surface,
    )
}

/** Version row with "检查更新": installs through the daemon, or opens the download otherwise. */
@Composable
private fun UpdateRow(client: DaemonClient, running: Boolean) {
    val p = LocalPalette.current
    val ctx = LocalContext.current
    val current = remember { appVersion(ctx) }
    val latest = Updates.latest
    var status by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var confirm by remember { mutableStateOf(false) }
    var heardBack by remember { mutableStateOf(false) }
    /** Download progress 0..1, -1 while the size is unknown, null when not downloading. */
    var progress by remember { mutableStateOf<Float?>(null) }
    fun showProgress(v: String, done: Long, total: Long) {
        progress = if (total > 0) (done.toFloat() / total).coerceIn(0f, 1f) else -1f
        val mb = "%.1f".format(done / 1048576f)
        status = if (done == 0L && total <= 0) "正在下载 v$v…"
        else if (total > 0) "正在下载 v$v… ${(done * 100 / total)}%（$mb / ${"%.1f".format(total / 1048576f)} MB）"
        else "正在下载 v$v… $mb MB"
    }
    val scope = androidx.compose.runtime.rememberCoroutineScope()

    // Without the phone service: download here (checksum-verified) and hand the APK to the
    // system installer. Not via the browser: Chrome opened from another app (a Custom Tab)
    // can stall at 100% and never finish the file.
    fun openDownload(v: String) {
        status = "正在下载 v$v…"
        thread {
            val apk = java.io.File(ctx.cacheDir, "updates/${app.syncboard.ipc.Releases.apkName(v)}")
            val r = runCatching {
                apk.parentFile?.mkdirs()
                apk.parentFile?.listFiles()?.forEach { it.delete() }
                app.syncboard.ipc.Releases.downloadApk(v, apk) { done, total -> showProgress(v, done, total) }
            }
            busy = false
            progress = null
            r.onFailure { status = "下载失败：${it.message}" }.onSuccess {
                status = "下载完成，请在系统安装界面点「安装」"
                val uri = androidx.core.content.FileProvider.getUriForFile(ctx, "app.syncboard.updates", apk)
                runCatching {
                    ctx.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, "application/vnd.android.package-archive")
                        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK))
                }.onFailure { status = "无法打开安装界面：${it.message}" }
            }
        }
    }

    LaunchedEffect(Updates.askOnOpen) {
        if (Updates.askOnOpen && !busy && latest != null) confirm = true
    }

    LaunchedEffect(client) {
        client.events.collect { ev ->
            if (ev.optString("event") != "update") return@collect
            heardBack = true
            when (ev.optString("stage")) {
                "downloading" -> showProgress(ev.optString("version"), ev.optLong("done"), ev.optLong("total"))
                "installing" -> { progress = null; status = "正在安装…（如手机弹出确认，请点「允许」）" }
                "done" -> { progress = null; status = "已更新，正在重启 SyncBoard" }
                "failed" -> { busy = false; progress = null; status = ev.optString("reason") }
            }
        }
    }

    Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 10.dp, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text("SyncBoard $current", fontSize = 15.sp)
            Text(status ?: latest?.let { "有新版本 v$it" } ?: "从 GitHub 发布页检查新版本", fontSize = 12.sp, color = p.muted, lineHeight = 16.sp)
            progress?.let { f ->
                val mod = Modifier.fillMaxWidth().padding(top = 6.dp).height(3.dp).clip(RoundedCornerShape(2.dp))
                if (f < 0) {
                    androidx.compose.material3.LinearProgressIndicator(mod, color = p.ink, trackColor = p.line)
                } else {
                    androidx.compose.material3.LinearProgressIndicator({ f }, mod, color = p.ink, trackColor = p.line,
                        gapSize = 0.dp, drawStopIndicator = {})
                }
            }
        }
        TextButton(
            onClick = {
                latest?.let { confirm = true; return@TextButton }
                busy = true
                status = "正在检查…"
                thread {
                    val r = runCatching { app.syncboard.ipc.Releases.latestVersion() }
                    busy = false
                    r.onSuccess { v ->
                        if (app.syncboard.ipc.Releases.isNewer(v, current)) { Updates.latest = v; status = null; confirm = true }
                        else status = "已是最新版本"
                    }.onFailure { status = "检查失败：${it.message}" }
                }
            },
            enabled = !busy,
        ) { Text(if (latest != null) "更新" else "检查更新", color = if (busy) p.muted else p.ink, fontWeight = FontWeight.SemiBold) }
    }

    if (confirm) {
        val v = latest!!
        AlertDialog(
            onDismissRequest = { confirm = false; Updates.askOnOpen = false },
            title = { Text("更新到 v$v？") },
            text = {
                Text(
                    if (running) "将由手机服务下载并安装，完成后 SyncBoard 会自动重启。" else "手机服务未运行，将下载安装包并打开系统安装界面。",
                    color = p.muted,
                )
            },
            confirmButton = {
                TextButton({
                    confirm = false
                    Updates.askOnOpen = false
                    busy = true
                    heardBack = false
                    if (running && client.cmd("selfUpdate", "version" to v)) {
                        status = "正在准备…"
                        // A service from before self-update ignores the command.
                        scope.launch { kotlinx.coroutines.delay(4000); if (!heardBack) openDownload(v) }
                    } else {
                        openDownload(v)
                    }
                }) { Text("更新", color = p.ink) }
            },
            dismissButton = {
                TextButton({ confirm = false; Updates.askOnOpen = false; runCatching { ctx.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(app.syncboard.ipc.Releases.pageUrl(v)))) } }) {
                    Text("查看发布说明", color = p.muted)
                }
            },
            containerColor = p.surface,
        )
    }
}

private fun appVersion(ctx: android.content.Context) =
    runCatching { ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName }.getOrNull().orEmpty()

/**
 * Sends picked or shared files to the PC through the daemon, one at a time.
 * [onProgress] gets (index, total, name) before each file; [onDone] runs on the main
 * thread with a summary, after the toast.
 */
fun sendUris(
    ctx: android.content.Context,
    uris: List<Uri>,
    onProgress: (Int, Int, String) -> Unit = { _, _, _ -> },
    onDone: () -> Unit = {},
) {
    val app = ctx.applicationContext
    val main = android.os.Handler(android.os.Looper.getMainLooper())
    thread {
        var ok = 0
        uris.forEachIndexed { i, uri ->
            val (name, size) = runCatching {
                app.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
                    ?.use { c -> if (c.moveToFirst()) (c.getString(0) ?: "file") to c.getLong(1) else null }
            }.getOrNull() ?: ("file" to -1L)
            main.post { onProgress(i, uris.size, name) }
            if (size < 0) {
                android.util.Log.w("SyncBoard", "send: unknown size for ${uri.authority}, skipped")
                return@forEachIndexed
            }
            val sent = runCatching {
                app.contentResolver.openInputStream(uri)?.use { DaemonClient.sendFile(app, name, size, it) } ?: false
            }.onFailure { android.util.Log.w("SyncBoard", "send: ${uri.authority} failed", it) }.getOrDefault(false)
            if (sent) ok++
        }
        main.post {
            Toast.makeText(app, if (ok == uris.size) "已发送 $ok 个文件" else "发送完成 $ok/${uris.size}，请确认电脑在线", Toast.LENGTH_SHORT).show()
            onDone()
        }
    }
}
