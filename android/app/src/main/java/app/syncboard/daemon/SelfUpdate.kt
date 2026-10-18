package app.syncboard.daemon

import android.util.Log
import app.syncboard.ipc.Releases
import org.json.JSONObject
import java.io.File

/**
 * Installs a newer app release as shell, so updating needs no installer prompt (some ROMs
 * still ask once). The new APK replaces the running one, so [Main]'s APK watcher then
 * restarts the daemon from it.
 */
object SelfUpdate {
    @Volatile private var running = false

    fun start(version: String, dir: File, emit: (JSONObject) -> Unit) {
        if (running) return
        running = true
        fun event(stage: String, reason: String? = null, done: Long = 0, total: Long = 0) =
            emit(JSONObject().put("event", "update").put("stage", stage).put("version", version).put("reason", reason)
                .put("done", done).put("total", total))
        Thread({
            val apk = File(dir, "update.apk")
            try {
                event("downloading")
                Releases.downloadApk(version, apk) { done, total -> event("downloading", done = done, total = total) }
                event("installing")
                val p = ProcessBuilder("pm", "install", "-r", apk.path).redirectErrorStream(true).start()
                val out = p.inputStream.bufferedReader().readText()
                p.waitFor()
                Log.i(Engine.TAG, "self-update to $version: ${out.trim()}")
                if ("Success" in out) event("done") else event("failed", installError(out))
            } catch (e: Exception) {
                Log.w(Engine.TAG, "self-update failed", e)
                event("failed", e.message ?: e.toString())
            } finally {
                apk.delete()
                running = false
            }
        }, "self-update").start()
    }

    private fun installError(out: String) = when {
        "INSTALL_FAILED_UPDATE_INCOMPATIBLE" in out -> "签名不同，无法覆盖安装。请卸载后从发布页安装新版。"
        "INSTALL_FAILED_USER_RESTRICTED" in out || "INSTALL_FAILED_ABORTED" in out -> "安装被手机拒绝，请在弹窗中点「允许」后重试。"
        else -> "安装失败：" + out.trim().lineSequence().lastOrNull().orEmpty()
    }
}
