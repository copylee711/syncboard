package app.syncboard.ipc

import android.content.Context
import org.json.JSONObject

/** Cached copy of the daemon's pairing data, so the app can wake a PC after a reboot. */
object WakeInfo {
    private const val PREFS = "wake"

    fun save(context: Context, info: JSONObject) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString("info", info.toString()).apply()
    }

    fun load(context: Context): JSONObject? =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString("info", null)
            ?.let { runCatching { JSONObject(it) }.getOrNull() }
            ?.takeIf { (it.optJSONArray("pcs")?.length() ?: 0) > 0 }
}
