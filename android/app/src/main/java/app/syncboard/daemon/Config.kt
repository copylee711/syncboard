package app.syncboard.daemon

import android.os.Build
import android.util.Base64
import app.syncboard.proto.Crypto
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

class Pc(
    val id: String,
    var name: String,
    val psk: ByteArray,
    /** Known addresses, most recent first. */
    val hosts: MutableList<String>,
    var port: Int,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("id", id).put("name", name).put("port", port)
        .put("psk", Base64.encodeToString(psk, Base64.NO_WRAP))
        .put("hosts", JSONArray(hosts))

    fun rememberHost(h: String) {
        hosts.remove(h)
        hosts.add(0, h)
        while (hosts.size > 6) hosts.removeAt(hosts.size - 1)
    }

    companion object {
        fun fromJson(o: JSONObject) = Pc(
            id = o.getString("id"),
            name = o.optString("name", "PC"),
            psk = Base64.decode(o.getString("psk"), Base64.NO_WRAP),
            hosts = o.optJSONArray("hosts")?.let { a -> MutableList(a.length()) { a.getString(it) } } ?: mutableListOf(),
            port = o.optInt("port", Config.PORT),
        )
    }
}

class Settings(
    var syncText: Boolean = true,
    var syncImages: Boolean = true,
    var autoAcceptFiles: Boolean = true,
    var paused: Boolean = false,
    /** Turn off Android 12+'s "pasted from clipboard" toast (a system-wide setting). */
    var hideAccessToast: Boolean = true,
    /** Put new screenshots on the phone and PC clipboards. */
    var copyScreenshots: Boolean = false,
    /** Put new camera photos (DCIM/Camera) on the phone and PC clipboards. */
    var copyCameraPhotos: Boolean = false,
    /** Also new images in these folders (MediaStore relative paths, e.g. "Pictures/WeiXin"). */
    var copyFolders: List<String> = emptyList(),
    /** Send the verification code of each new text message to the PCs (not the message). */
    var syncSmsCodes: Boolean = true,
    /** Send every new text message (sender and text) to the PCs, to show as a notification. */
    var syncSms: Boolean = false,
    /**
     * Pass app notifications to the PCs (and find codes in 5G messages, which never reach
     * the SMS database). Needs the app's notification listener, granted by the daemon.
     */
    var syncNotifications: Boolean = false,
    /**
     * Switch developer options off once a PC has set this phone up over adb (the system
     * advises against leaving them on). The app switches them on briefly whenever the
     * service has to be restarted.
     */
    var autoDisableDevOptions: Boolean = true,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("syncText", syncText).put("syncImages", syncImages)
        .put("autoAcceptFiles", autoAcceptFiles).put("paused", paused)
        .put("hideAccessToast", hideAccessToast).put("copyScreenshots", copyScreenshots)
        .put("autoDisableDevOptions", autoDisableDevOptions)
        .put("syncSmsCodes", syncSmsCodes).put("syncSms", syncSms).put("syncNotifications", syncNotifications).put("copyCameraPhotos", copyCameraPhotos).put("copyFolders", JSONArray(copyFolders))

    fun update(o: JSONObject) {
        if (o.has("syncText")) syncText = o.getBoolean("syncText")
        if (o.has("syncImages")) syncImages = o.getBoolean("syncImages")
        if (o.has("autoAcceptFiles")) autoAcceptFiles = o.getBoolean("autoAcceptFiles")
        if (o.has("paused")) paused = o.getBoolean("paused")
        if (o.has("hideAccessToast")) hideAccessToast = o.getBoolean("hideAccessToast")
        if (o.has("copyScreenshots")) copyScreenshots = o.getBoolean("copyScreenshots")
        if (o.has("autoDisableDevOptions")) autoDisableDevOptions = o.getBoolean("autoDisableDevOptions")
        if (o.has("syncSmsCodes")) syncSmsCodes = o.getBoolean("syncSmsCodes")
        if (o.has("syncSms")) syncSms = o.getBoolean("syncSms")
        if (o.has("syncNotifications")) syncNotifications = o.getBoolean("syncNotifications")
        if (o.has("copyCameraPhotos")) copyCameraPhotos = o.getBoolean("copyCameraPhotos")
        o.optJSONArray("copyFolders")?.let { a -> copyFolders = List(a.length()) { a.getString(it) }.distinct() }
    }
}

/** Daemon state, persisted as JSON. */
class Config private constructor(private val file: File) {
    var deviceId: String = Crypto.randomId()
    var name: String = Build.MODEL ?: "Android"
    val pcs = mutableListOf<Pc>()
    val settings = Settings()
    /** Apps the user let use this daemon (TwinFile): package name -> uid at approval time. */
    val clients = mutableMapOf<String, Int>()

    @Synchronized
    fun save() {
        val o = JSONObject()
            .put("deviceId", deviceId).put("name", name)
            .put("pcs", JSONArray(pcs.map { it.toJson() }))
            .put("settings", settings.toJson())
            .put("clients", JSONObject().also { o -> clients.forEach { (k, v) -> o.put(k, v) } })
        file.parentFile?.mkdirs()
        val tmp = File(file.path + ".tmp")
        tmp.writeText(o.toString(2))
        tmp.renameTo(file)
    }

    @Synchronized
    fun pc(id: String) = pcs.firstOrNull { it.id == id }

    @Synchronized
    fun upsert(pc: Pc) {
        pcs.removeAll { it.id == pc.id }
        pcs.add(pc)
        save()
    }

    @Synchronized
    fun remove(id: String) {
        pcs.removeAll { it.id == id }
        save()
    }

    companion object {
        const val PORT = 53317

        fun load(file: File): Config {
            val c = Config(file)
            runCatching {
                val o = JSONObject(file.readText())
                c.deviceId = o.getString("deviceId")
                c.name = o.optString("name", c.name)
                o.optJSONArray("pcs")?.let { a -> for (i in 0 until a.length()) c.pcs.add(Pc.fromJson(a.getJSONObject(i))) }
                o.optJSONObject("settings")?.let { c.settings.update(it) }
                o.optJSONObject("clients")?.let { cl -> cl.keys().forEach { k -> c.clients[k] = cl.getInt(k) } }
            }
            c.save()
            return c
        }
    }
}
