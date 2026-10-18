package app.syncboard.daemon

import android.util.Log
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Warnings that also go to `daemon.log` in the state directory: some ROMs (OriginOS) drop
 * logcat lines from non-system processes, so this is how problems on them can be read
 * (`adb shell cat /data/local/tmp/syncboard/daemon.log`). Kept under 64 KB.
 */
object DaemonLog {
    private const val TAG = "SyncBoard"
    private val file get() = File(Main.stateDir, "daemon.log")
    private val fmt = SimpleDateFormat("MM-dd HH:mm:ss", Locale.US)

    fun i(msg: String) = write("I", msg, null).also { Log.i(TAG, msg) }

    fun w(msg: String, t: Throwable? = null) = write("W", msg, t).also { Log.w(TAG, msg, t) }

    @Synchronized
    private fun write(level: String, msg: String, t: Throwable?) {
        runCatching {
            val f = file
            if (f.length() > 64 * 1024) f.renameTo(File(f.path + ".1"))
            f.appendText("${fmt.format(Date())} $level $msg${t?.let { "\n  " + it.stackTraceToString().lines().take(8).joinToString("\n  ") } ?: ""}\n")
        }
    }
}
