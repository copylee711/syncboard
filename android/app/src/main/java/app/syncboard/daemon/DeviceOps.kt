package app.syncboard.daemon

import android.media.AudioManager
import android.media.ToneGenerator
import android.os.Build
import android.os.StatFs
import android.util.Log
import org.json.JSONObject

/**
 * Phone controls the PC asks for over the request channel (protocol §9): status, ring
 * (find my phone), media keys and opening a link. Nothing here runs unless asked.
 */
object DeviceOps {
    private const val TAG = "SyncBoard"

    fun handles(op: String) = op in setOf("status", "ring", "media", "open")

    fun run(req: JSONObject, ctx: android.content.Context): JSONObject = when (req.optString("op")) {
        "status" -> status()
        "ring" -> ring(ctx, req.optBoolean("stop"))
        "media" -> media(ctx, req.optString("action"))
        "open" -> open(req.optString("url"))
        else -> throw FileOps.OpError("不支持")
    }

    private fun ok() = JSONObject().put("ok", true)

    private fun sh(vararg cmd: String): String = runCatching {
        val p = ProcessBuilder(*cmd).redirectErrorStream(true).start()
        p.inputStream.bufferedReader().readText().also { p.waitFor() }
    }.getOrDefault("")

    /** Battery from `dumpsys battery` (no receiver to keep registered), storage, model. */
    private fun status(): JSONObject {
        val bat = sh("dumpsys", "battery")
        fun field(name: String) = Regex("^\\s*$name:\\s*(.+)$", RegexOption.MULTILINE).find(bat)?.groupValues?.get(1)?.trim()
        val level = field("level")?.toIntOrNull()
        val scale = field("scale")?.toIntOrNull()?.takeIf { it > 0 } ?: 100
        val plugged = listOf("AC powered", "USB powered", "Wireless powered", "Dock powered").any { field(it) == "true" }
        // BatteryManager.BATTERY_STATUS_CHARGING = 2, FULL = 5
        val statusCode = field("status")?.toIntOrNull()
        val st = runCatching { StatFs(FileOps.HOME) }.getOrNull()
        return ok()
            .put("model", Build.MODEL).put("android", Build.VERSION.RELEASE)
            .put("battery", level?.let { it * 100 / scale } ?: JSONObject.NULL)
            .put("charging", plugged && statusCode != 5).put("full", statusCode == 5)
            .put("temp", field("temperature")?.toIntOrNull()?.let { it / 10.0 } ?: JSONObject.NULL)
            .put("storageTotal", st?.totalBytes ?: 0).put("storageFree", st?.availableBytes ?: 0)
    }

    @Volatile private var ringing: Thread? = null

    /**
     * Beeps loudly on the alarm stream for up to a minute (or until stopped), so a phone
     * on silent can still be found; the alarm volume is put back afterwards.
     */
    private fun ring(ctx: android.content.Context, stop: Boolean): JSONObject {
        ringing?.interrupt()
        ringing = null
        if (stop) return ok().put("ringing", false)
        val t = Thread({
            val am = ctx.getSystemService(AudioManager::class.java)
            val before = runCatching { am.getStreamVolume(AudioManager.STREAM_ALARM) }.getOrNull()
            runCatching { am.setStreamVolume(AudioManager.STREAM_ALARM, am.getStreamMaxVolume(AudioManager.STREAM_ALARM), 0) }
                .onFailure { Log.w(TAG, "ring: cannot raise alarm volume", it) }
            val tone = runCatching { ToneGenerator(AudioManager.STREAM_ALARM, ToneGenerator.MAX_VOLUME) }.getOrNull()
            try {
                val end = System.currentTimeMillis() + 60_000
                while (System.currentTimeMillis() < end) {
                    tone?.startTone(ToneGenerator.TONE_CDMA_HIGH_PBX_L, 900)
                    sh("cmd", "vibrator_manager", "synced", "oneshot", "600")
                    Thread.sleep(1200)
                }
            } catch (_: InterruptedException) {
            } finally {
                tone?.stopTone()
                tone?.release()
                if (before != null) runCatching { am.setStreamVolume(AudioManager.STREAM_ALARM, before, 0) }
            }
        }, "ring")
        ringing = t
        t.start()
        return ok().put("ringing", true)
    }

    private val KEYS = mapOf(
        "playPause" to "KEYCODE_MEDIA_PLAY_PAUSE", "next" to "KEYCODE_MEDIA_NEXT", "previous" to "KEYCODE_MEDIA_PREVIOUS",
        "volumeUp" to "KEYCODE_VOLUME_UP", "volumeDown" to "KEYCODE_VOLUME_DOWN", "mute" to "KEYCODE_VOLUME_MUTE",
        "lock" to "KEYCODE_SLEEP",
    )

    /** Volume buttons change the media volume (a volume key would pick the ringer when nothing plays). */
    private fun media(ctx: android.content.Context, action: String): JSONObject {
        val adjust = mapOf("volumeUp" to AudioManager.ADJUST_RAISE, "volumeDown" to AudioManager.ADJUST_LOWER,
            "mute" to AudioManager.ADJUST_TOGGLE_MUTE)[action]
        if (adjust != null) {
            val am = ctx.getSystemService(AudioManager::class.java)
            val done = runCatching { am.adjustStreamVolume(AudioManager.STREAM_MUSIC, adjust, AudioManager.FLAG_SHOW_UI) }.isSuccess
            if (done) {
                return ok().put("volume", am.getStreamVolume(AudioManager.STREAM_MUSIC)).put("max", am.getStreamMaxVolume(AudioManager.STREAM_MUSIC))
            }
        }
        val key = KEYS[action] ?: throw FileOps.OpError("未知操作")
        sh("input", "keyevent", key)
        return ok()
    }

    /** Opens an http(s) link in the phone's browser (or the app that handles it). */
    private fun open(url: String): JSONObject {
        if (!(url.startsWith("http://") || url.startsWith("https://"))) throw FileOps.OpError("只能打开网页链接")
        val out = sh("am", "start", "-a", "android.intent.action.VIEW", "-d", url)
        if ("Error" in out) throw FileOps.OpError("手机上没有能打开它的应用")
        return ok()
    }
}
