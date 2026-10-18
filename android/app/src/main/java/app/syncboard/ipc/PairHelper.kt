package app.syncboard.ipc

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import app.syncboard.proto.readPlain
import app.syncboard.proto.writePlain
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * While the user scans the PC's QR with the wireless-debugging scanner, watches for this
 * phone's own `_adb-tls-pairing` service and tells the PC its port (protocol §7). The PC
 * finds it by mDNS on most networks, but campus and office Wi-Fi often drop multicast
 * between devices, leaving the phone at 正在配对设备… forever. A short foreground service,
 * so the ROM doesn't freeze the app while Settings is in front.
 */
class PairHelper : Service() {
    @Volatile private var thread: Thread? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "扫码配对", NotificationManager.IMPORTANCE_LOW))
        val n = Notification.Builder(this, CHANNEL)
            .setSmallIcon(app.syncboard.R.drawable.ic_tile)
            .setContentTitle("正在等待扫码配对电脑")
            .setContentText("扫码后会自动告诉电脑配对端口")
            .build()
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTE_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(NOTE_ID, n)
        }
        if (thread?.isAlive != true) {
            thread = Thread({
                runCatching { watch(applicationContext) }.onFailure { Log.w(TAG, "pair helper", it) }
                stopSelf()
            }, "pair-helper").apply { start() }
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        thread?.interrupt()
        super.onDestroy()
    }

    companion object {
        private const val TAG = "SyncBoard"
        private const val CHANNEL = "pairing"
        private const val NOTE_ID = 8
        private const val WATCH_MS = 3 * 60_000L

        fun start(ctx: Context) {
            runCatching { ctx.startForegroundService(Intent(ctx, PairHelper::class.java)) }
                .onFailure { Log.w(TAG, "pair helper not started", it) }
        }

        // Some ROMs (vivo) hide app logcat; the last run is also kept in
        // Android/data/app.syncboard/files/pair.log for diagnosis.
        private var traceFile: java.io.File? = null
        private fun trace(msg: String) {
            Log.d(TAG, msg)
            runCatching { traceFile?.appendText("${java.time.LocalTime.now()} $msg\n") }
        }

        private fun watch(ctx: Context) {
            traceFile = ctx.getExternalFilesDir(null)?.let { java.io.File(it, "pair.log") }
            runCatching { traceFile?.writeText("") }
            trace("watch start")
            val nsd = ctx.getSystemService(NsdManager::class.java)
            val found = LinkedBlockingQueue<NsdServiceInfo>()
            val listener = object : NsdManager.DiscoveryListener {
                override fun onServiceFound(s: NsdServiceInfo) { found.put(s) }
                override fun onServiceLost(s: NsdServiceInfo) {}
                override fun onDiscoveryStarted(t: String) {}
                override fun onDiscoveryStopped(t: String) {}
                override fun onStartDiscoveryFailed(t: String, code: Int) {}
                override fun onStopDiscoveryFailed(t: String, code: Int) {}
            }
            nsd.discoverServices("_adb-tls-pairing._tcp", NsdManager.PROTOCOL_DNS_SD, listener)
            val deadline = System.currentTimeMillis() + WATCH_MS
            try {
                while (true) {
                    val left = deadline - System.currentTimeMillis()
                    if (left <= 0) return
                    val s = found.poll(left, TimeUnit.MILLISECONDS) ?: return
                    val hosts = AdbQr.hostsOf(s.serviceName)
                    trace("found ${s.serviceName} hosts=$hosts")
                    if (hosts.isEmpty()) continue
                    val si = Recovery.resolve(nsd, s)
                    trace("resolved host=${si?.host} port=${si?.port}")
                    if (si == null) continue
                    val local = runCatching { java.net.NetworkInterface.getByInetAddress(si.host) != null }.getOrDefault(false)
                    trace("local=$local")
                    if (!local) continue
                    // Wireless debugging is on (it is pairing), so its connect port is up too.
                    val connectPort = Recovery.findAdbPort(ctx, 8_000)
                    trace("connect port $connectPort")
                    if (report(hosts, s.serviceName, si.port, connectPort)) return
                }
            } finally {
                runCatching { nsd.stopServiceDiscovery(listener) }
            }
        }

        private fun report(hosts: List<String>, name: String, pairPort: Int, connectPort: Int?): Boolean {
            val msg = JSONObject().put("t", "adb_pair").put("v", 1).put("name", name)
                .put("pair_port", pairPort).put("connect_port", connectPort ?: JSONObject.NULL)
            for (host in hosts) {
                val ok = runCatching {
                    Socket().use { sock ->
                        sock.connect(InetSocketAddress(host, 53317), 3000)
                        sock.soTimeout = 5000
                        writePlain(DataOutputStream(BufferedOutputStream(sock.getOutputStream())), msg)
                        readPlain(DataInputStream(BufferedInputStream(sock.getInputStream()))).optString("t") == "ok"
                    }
                }.onFailure { trace("report $host failed: $it") }.getOrDefault(false)
                trace("pair hint to $host: $ok")
                if (ok) return true
            }
            return false
        }
    }
}
