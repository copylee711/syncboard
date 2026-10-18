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

    /** Drops PCs that said they removed this phone. */
    fun forget(context: Context, pcIds: Set<String>) {
        val info = load(context) ?: return
        val pcs = info.getJSONArray("pcs")
        val kept = org.json.JSONArray()
        for (i in 0 until pcs.length()) pcs.getJSONObject(i).takeIf { it.optString("id") !in pcIds }?.let(kept::put)
        save(context, info.put("pcs", kept))
    }

    fun pcIds(context: Context): Set<String> {
        val pcs = load(context)?.optJSONArray("pcs") ?: return emptySet()
        return (0 until pcs.length()).map { pcs.getJSONObject(it).optString("id") }.toSet()
    }

    fun load(context: Context): JSONObject? =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString("info", null)
            ?.let { runCatching { JSONObject(it) }.getOrNull() }
            ?.takeIf { (it.optJSONArray("pcs")?.length() ?: 0) > 0 }
}
