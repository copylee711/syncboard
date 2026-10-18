package app.syncboard.ipc

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.provider.Settings
import android.util.Base64
import android.util.Log
import app.syncboard.proto.Crypto
import app.syncboard.proto.readPlain
import app.syncboard.proto.writePlain
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Restarting the phone service without a cable (protocol §6): turn wireless debugging
 * on, find its port, and ask a paired PC to `adb connect` and start the daemon.
 */
object Recovery {
    private const val TAG = "SyncBoard"
    private const val ADB_WIFI = "adb_wifi_enabled"

    sealed interface Result {
        data object Started : Result
        data object NoWifiDebugging : Result
        data object NoPc : Result
        /** Every paired PC answered that it removed this phone; it must be set up again. */
        data object Forgotten : Result
        data class Failed(val reason: String) : Result
    }

    fun canToggleWifiDebugging(ctx: Context) =
        ctx.checkSelfPermission("android.permission.WRITE_SECURE_SETTINGS") == PackageManager.PERMISSION_GRANTED

    private fun wifiDebuggingOn(ctx: Context) = Settings.Global.getInt(ctx.contentResolver, ADB_WIFI, 0) == 1

    private fun daemonUp(): Boolean = runCatching {
        Socket().use { it.connect(InetSocketAddress("127.0.0.1", DaemonClient.PORT), 500) }
    }.isSuccess

    /** Blocking; call off the main thread. One at a time: a second caller waits and then finds the daemon up. */
    @Synchronized
    fun run(ctx: Context): Result {
        if (daemonUp()) return Result.Started
        val info = WakeInfo.load(ctx) ?: return Result.NoPc
        // Wireless debugging only works with developer options on; switch both on for
        // the restart when allowed, and back off afterwards.
        val devOff = Settings.Global.getInt(ctx.contentResolver, Settings.Global.DEVELOPMENT_SETTINGS_ENABLED, 0) != 1
        val weEnabledDev = devOff && canToggleWifiDebugging(ctx)
        if (weEnabledDev) Settings.Global.putInt(ctx.contentResolver, Settings.Global.DEVELOPMENT_SETTINGS_ENABLED, 1)
        val weEnabled = !wifiDebuggingOn(ctx) && canToggleWifiDebugging(ctx)
        if (weEnabled) {
            Settings.Global.putInt(ctx.contentResolver, ADB_WIFI, 1)
        } else if (!wifiDebuggingOn(ctx)) {
            return Result.NoWifiDebugging
        }
        try {
            val port = findAdbPort(ctx, 15_000) ?: return Result.Failed("没有找到无线调试端口（手机需要连接 Wi-Fi）")
            when (wakeAnyPc(ctx, info, port)) {
                Wake.Sent -> {}
                Wake.Forgotten -> return Result.Forgotten
                Wake.Refused -> return Result.Failed("电脑拒绝了启动请求，请在电脑上重新添加手机")
                Wake.Unreachable -> return Result.Failed("联系不到电脑，请确认电脑上的 SyncBoard 正在运行")
            }
            val deadline = System.currentTimeMillis() + 45_000
            while (System.currentTimeMillis() < deadline) {
                if (daemonUp()) return Result.Started
                Thread.sleep(1000)
            }
            return Result.Failed("电脑没有完成启动，请检查电脑端 SyncBoard")
        } finally {
            // The daemon is detached from adbd, so wireless debugging can go off again.
            // With 配对后关闭开发者选项 on, leave them off even if they were on before.
            val offAfter = AppPrefs.autoDisableDev(ctx) && canToggleWifiDebugging(ctx)
            if (weEnabled || weEnabledDev || offAfter) Thread.sleep(1500)
            if (weEnabled || offAfter) Settings.Global.putInt(ctx.contentResolver, ADB_WIFI, 0)
            if (weEnabledDev || offAfter) Settings.Global.putInt(ctx.contentResolver, Settings.Global.DEVELOPMENT_SETTINGS_ENABLED, 0)
        }
    }

    /**
     * Finds the port wireless debugging listens on, from this phone's own
     * `_adb-tls-connect` mDNS service. Discovery can also return stale services from
     * before wireless debugging was last restarted (each restart picks a new port), so
     * every candidate is checked by connecting to it on loopback.
     */
    internal fun findAdbPort(ctx: Context, timeoutMs: Long): Int? {
        val nsd = ctx.getSystemService(NsdManager::class.java)
        val found = java.util.concurrent.LinkedBlockingQueue<NsdServiceInfo>()
        val listener = object : NsdManager.DiscoveryListener {
            override fun onServiceFound(s: NsdServiceInfo) { found.put(s) }
            override fun onServiceLost(s: NsdServiceInfo) {}
            override fun onDiscoveryStarted(t: String) {}
            override fun onDiscoveryStopped(t: String) {}
            override fun onStartDiscoveryFailed(t: String, code: Int) {}
            override fun onStopDiscoveryFailed(t: String, code: Int) {}
        }
        nsd.discoverServices("_adb-tls-connect._tcp", NsdManager.PROTOCOL_DNS_SD, listener)
        val deadline = System.currentTimeMillis() + timeoutMs
        val tried = mutableSetOf<Int>()
        try {
            while (true) {
                val left = deadline - System.currentTimeMillis()
                if (left <= 0) return null
                val s = found.poll(left, TimeUnit.MILLISECONDS) ?: return null
                // One resolve at a time: older Android rejects concurrent ones.
                val port = resolve(nsd, s)?.takeIf { si ->
                    runCatching { java.net.NetworkInterface.getByInetAddress(si.host) != null }.getOrDefault(false)
                }?.port ?: continue
                if (!tried.add(port)) continue
                val open = runCatching {
                    Socket().use { it.connect(InetSocketAddress("127.0.0.1", port), 800) }
                }.isSuccess
                Log.d(TAG, "adb port candidate $port: ${if (open) "listening" else "stale"}")
                if (open) return port
            }
        } finally {
            runCatching { nsd.stopServiceDiscovery(listener) }
        }
    }

    internal fun resolve(nsd: NsdManager, s: NsdServiceInfo): NsdServiceInfo? {
        val done = CountDownLatch(1)
        var out: NsdServiceInfo? = null
        @Suppress("DEPRECATION")
        nsd.resolveService(s, object : NsdManager.ResolveListener {
            override fun onResolveFailed(si: NsdServiceInfo, code: Int) = done.countDown()
            override fun onServiceResolved(si: NsdServiceInfo) {
                out = si
                done.countDown()
            }
        })
        done.await(3, TimeUnit.SECONDS)
        return out
    }

    private enum class Wake { Sent, Forgotten, Refused, Unreachable }

    /**
     * Asks each paired PC in turn to start the daemon. PCs that answer `unknown_device`
     * removed this phone, so they are dropped from [WakeInfo].
     */
    private fun wakeAnyPc(ctx: Context, info: JSONObject, adbPort: Int): Wake {
        val pcs = info.getJSONArray("pcs")
        val forgotten = mutableSetOf<String>()
        var reached = false
        fun done(r: Wake): Wake {
            if (forgotten.isNotEmpty()) WakeInfo.forget(ctx, forgotten)
            return r
        }
        for (i in 0 until pcs.length()) {
            val pc = pcs.getJSONObject(i)
            val psk = Base64.decode(pc.getString("psk"), Base64.NO_WRAP)
            val hosts = pc.optJSONArray("hosts") ?: continue
            for (j in 0 until hosts.length()) {
                val host = hosts.getString(j)
                if (host == "127.0.0.1") continue
                val ts = System.currentTimeMillis() / 1000
                val mac = Crypto.hmac(psk, "wake".toByteArray(), adbPort.toString().toByteArray(), ts.toString().toByteArray())
                val reply = runCatching {
                    Socket().use { s ->
                        s.connect(InetSocketAddress(host, pc.optInt("port", 53317)), 3000)
                        s.soTimeout = 5000
                        writePlain(DataOutputStream(BufferedOutputStream(s.getOutputStream())), JSONObject()
                            .put("t", "adb_wake").put("v", 1).put("id", info.getString("deviceId"))
                            .put("port", adbPort).put("ts", ts).put("mac", Base64.encodeToString(mac, Base64.NO_WRAP)))
                        // Older PCs close without answering.
                        runCatching { readPlain(DataInputStream(BufferedInputStream(s.getInputStream()))) }
                            .getOrDefault(JSONObject().put("t", "wake_ok"))
                    }
                }.getOrNull()
                Log.d(TAG, "wake $host: $reply")
                if (reply == null) continue
                if (reply.optString("t") == "wake_ok") return done(Wake.Sent)
                if (reply.optString("code") == "unknown_device") forgotten += pc.getString("id")
                reached = true
                break
            }
        }
        return done(when {
            forgotten.size == pcs.length() -> Wake.Forgotten
            reached -> Wake.Refused
            else -> Wake.Unreachable
        })
    }

    // ---- scheduling after boot ----

    private const val JOB_ID = 53317

    fun schedule(ctx: Context) {
        val js = ctx.getSystemService(JobScheduler::class.java)
        js.schedule(JobInfo.Builder(JOB_ID, ComponentName(ctx, Job::class.java))
            .setRequiredNetworkType(JobInfo.NETWORK_TYPE_UNMETERED)
            .setOverrideDeadline(10 * 60 * 1000L)
            .build())
    }

    class Job : JobService() {
        override fun onStartJob(params: JobParameters): Boolean {
            Thread {
                val r = runCatching { run(applicationContext) }.getOrElse { Result.Failed(it.toString()) }
                Log.i(TAG, "boot recovery: $r")
                jobFinished(params, r is Result.Failed)
            }.start()
            return true
        }

        override fun onStopJob(params: JobParameters) = true
    }

    class BootReceiver : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent) {
            if (intent.action == Intent.ACTION_BOOT_COMPLETED && AppPrefs.autoRecover(ctx)) schedule(ctx)
        }
    }
}

object AppPrefs {
    private fun p(ctx: Context) = ctx.getSharedPreferences("app", Context.MODE_PRIVATE)
    fun autoRecover(ctx: Context) = p(ctx).getBoolean("autoRecover", true)
    fun setAutoRecover(ctx: Context, v: Boolean) = p(ctx).edit().putBoolean("autoRecover", v).apply()
    fun theme(ctx: Context): String = p(ctx).getString("theme", "system") ?: "system"
    fun setTheme(ctx: Context, v: String) = p(ctx).edit().putString("theme", v).apply()
    /** The user stopped the service (here or from the PC): don't start it again by itself. */
    fun userStopped(ctx: Context) = p(ctx).getBoolean("userStopped", false)
    fun setUserStopped(ctx: Context, v: Boolean) = p(ctx).edit().putBoolean("userStopped", v).apply()
    /** Mirror of the daemon's autoDisableDevOptions, for when the daemon isn't running. */
    fun autoDisableDev(ctx: Context) = p(ctx).getBoolean("autoDisableDev", true)
    fun setAutoDisableDev(ctx: Context, v: Boolean) = p(ctx).edit().putBoolean("autoDisableDev", v).apply()
    fun askedNotify(ctx: Context) = p(ctx).getBoolean("askedNotify", false)
    fun setAskedNotify(ctx: Context) = p(ctx).edit().putBoolean("askedNotify", true).apply()
    fun hideFromRecents(ctx: Context) = p(ctx).getBoolean("hideRecents", false)
    fun setHideFromRecents(ctx: Context, v: Boolean) = p(ctx).edit().putBoolean("hideRecents", v).apply()
}
