package app.syncboard.ui

import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
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

/** Quick Settings tile: pause / resume clipboard sync. */
class SyncTileService : TileService() {
    /** Connects, optionally sends [cmd], and returns the daemon's settings (null if not running). */
    private fun exchange(cmd: JSONObject?): JSONObject? = runCatching {
        Socket().use { s ->
            s.connect(InetSocketAddress("127.0.0.1", DaemonClient.PORT), 600)
            s.soTimeout = 1500
            val o = DataOutputStream(BufferedOutputStream(s.getOutputStream()))
            val input = DataInputStream(BufferedInputStream(s.getInputStream()))
            writePlain(o, JSONObject().put("cmd", "hello"))
            if (cmd != null) writePlain(o, cmd)
            var settings: JSONObject? = null
            // hello, status, then (after a settings change) a fresh status
            repeat(if (cmd != null) 3 else 2) {
                val ev = readPlain(input)
                if (ev.optString("event") == "status") settings = ev.optJSONObject("settings")
            }
            settings
        }
    }.getOrNull()

    private fun render(settings: JSONObject?) {
        val tile = qsTile ?: return
        tile.label = "SyncBoard"
        when {
            settings == null -> { tile.state = Tile.STATE_UNAVAILABLE; tile.subtitle = "服务未运行" }
            settings.optBoolean("paused") -> { tile.state = Tile.STATE_INACTIVE; tile.subtitle = "已暂停" }
            else -> { tile.state = Tile.STATE_ACTIVE; tile.subtitle = "同步中" }
        }
        tile.updateTile()
    }

    override fun onStartListening() {
        thread { val s = exchange(null); mainExecutor.execute { render(s) } }
    }

    override fun onClick() {
        thread {
            val cur = exchange(null) ?: return@thread
            val paused = !cur.optBoolean("paused")
            val s = exchange(JSONObject().put("cmd", "settings").put("settings", JSONObject().put("paused", paused)))
            mainExecutor.execute { render(s ?: cur.put("paused", paused)) }
        }
    }
}
