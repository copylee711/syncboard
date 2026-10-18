package app.syncboard.daemon

import android.os.Looper
import android.os.Process
import android.util.Log
import org.json.JSONObject
import java.io.File
import kotlin.system.exitProcess

/**
 * Daemon entry point, run as shell through app_process (see [LAUNCH_COMMAND]) or by
 * Shizuku. It keeps the sync engine alive independently of the app's process, so
 * Android's background limits and vendor task killers don't apply to it.
 */
object Main {
    const val PACKAGE = "app.syncboard"
    private const val NICE_NAME = "syncboard_daemon"

    /**
     * Shell command that starts the daemon detached; safe to run more than once. The
     * background job is a plain command so `adb shell` returns at once (a backgrounded
     * `[ ... ] && cmd` subshell would hold adb's output open while the daemon runs).
     */
    const val LAUNCH_COMMAND =
        "p=\$(pm path $PACKAGE | head -n1 | cut -d: -f2); [ -n \"\$p\" ] || exit 1; " +
            "CLASSPATH=\$p nohup setsid app_process /system/bin --nice-name=$NICE_NAME " +
            "app.syncboard.daemon.Main </dev/null >/dev/null 2>&1 &"

    val stateDir = File("/data/local/tmp/syncboard")
    private val downloadDir = File("/storage/emulated/0/Download/SyncBoard")

    @JvmStatic
    fun main(args: Array<String>) {
        Thread.setDefaultUncaughtExceptionHandler { t, e ->
            Log.e(Engine.TAG, "daemon crashed in ${t.name}", e)
            exitProcess(1)
        }
        if (!ShellContext.isShell) {
            System.err.println("SyncBoard daemon must run as shell (uid ${Process.myUid()})")
            exitProcess(2)
        }
        // `--pair <uri>`: pair with the PC that launched us (via adb), whether or not a
        // daemon is already running.
        val pairUri = args.indexOf("--pair").takeIf { it >= 0 }?.let { args.getOrNull(it + 1) }

        Looper.prepareMainLooper()
        val ctx = ShellContext.init()

        val ipc = try {
            IpcServer.bind { appUid(ctx) }
        } catch (e: Exception) {
            if (pairUri != null) IpcServer.sendTo(JSONObject().put("cmd", "pairQr").put("uri", pairUri))
            println("SyncBoard daemon already running")
            exitProcess(0)
        }
        stateDir.mkdirs()
        stateDir.setReadable(false, false); stateDir.setReadable(true, true)
        stateDir.setExecutable(false, false); stateDir.setExecutable(true, true)
        val config = Config.load(File(stateDir, "config.json"))
        val bridge = ClipboardBridge(ctx)
        val engine = Engine(config, bridge, downloadDir) { ipc.broadcast(it) }
        ipc.start(engine, bridge)
        engine.start()
        pairUri?.let { engine.pairQr(it) }
        watchApk()
        println("SyncBoard daemon running, pid ${Process.myPid()}")
        Log.i(Engine.TAG, "daemon started, pid ${Process.myPid()}")
        Looper.loop()
    }

    private fun appUid(ctx: android.content.Context): Int =
        runCatching { ctx.packageManager.getApplicationInfo(PACKAGE, 0).uid }.getOrDefault(-1)

    /**
     * When the app is updated its APK moves and this process runs stale code: start a
     * fresh daemon from the new APK and exit. If the app was uninstalled, just exit.
     */
    private fun watchApk() {
        val apk = System.getProperty("java.class.path")?.split(':')?.firstOrNull() ?: return
        Thread({
            while (true) {
                Thread.sleep(30_000)
                if (!File(apk).exists()) {
                    Log.i(Engine.TAG, "APK changed, restarting daemon")
                    runCatching {
                        ProcessBuilder("sh", "-c", "sleep 2; $LAUNCH_COMMAND").start()
                    }
                    exitProcess(0)
                }
            }
        }, "apk-watch").apply { isDaemon = true }.start()
    }
}
