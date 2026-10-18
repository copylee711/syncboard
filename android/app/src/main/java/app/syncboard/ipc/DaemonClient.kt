package app.syncboard.ipc

import android.content.Context
import app.syncboard.proto.readPlain
import app.syncboard.proto.writePlain
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.File
import java.io.InputStream
import java.net.InetSocketAddress
import java.net.Socket

/**
 * The app's side of the daemon control channel (see daemon/IpcServer.kt). Keeps a
 * connection while [start]ed, exposes the latest status and forwards other events.
 */
class DaemonClient(private val context: Context) {
    sealed interface Conn {
        data object Checking : Conn
        data object NotRunning : Conn
        data class Running(val status: JSONObject) : Conn
    }

    private val _conn = MutableStateFlow<Conn>(Conn.Checking)
    val conn: StateFlow<Conn> = _conn.asStateFlow()
    val events = MutableSharedFlow<JSONObject>(extraBufferCapacity = 32)

    @Volatile private var out: DataOutputStream? = null
    @Volatile private var running = false
    private var thread: Thread? = null

    fun start() {
        if (running) return
        running = true
        thread = Thread({ loop() }, "daemon-client").apply { isDaemon = true; start() }
    }

    fun stop() {
        running = false
        runCatching { out?.close() }
        thread?.interrupt()
    }

    fun send(cmd: JSONObject): Boolean {
        val o = out ?: return false
        return runCatching { writePlain(o, cmd) }.isSuccess
    }

    fun cmd(name: String, vararg kv: Pair<String, Any?>): Boolean =
        send(JSONObject().put("cmd", name).apply { kv.forEach { (k, v) -> put(k, v) } })

    private fun loop() {
        while (running) {
            try {
                Socket().use { s ->
                    s.connect(InetSocketAddress("127.0.0.1", PORT), 800)
                    val input = DataInputStream(BufferedInputStream(s.getInputStream()))
                    val o = DataOutputStream(BufferedOutputStream(s.getOutputStream()))
                    writePlain(o, JSONObject().put("cmd", "hello"))
                    out = o
                    send(JSONObject().put("cmd", "wakeInfo"))
                    while (running) {
                        val ev = readPlain(input)
                        when (ev.optString("event")) {
                            "status" -> {
                                _conn.value = Conn.Running(ev)
                                // Keep the cached wake list in step with pairing changes.
                                val pcs = ev.optJSONArray("pcs")
                                val ids = (0 until (pcs?.length() ?: 0)).map { pcs!!.getJSONObject(it).optString("id") }.toSet()
                                if (ids != WakeInfo.pcIds(context)) send(JSONObject().put("cmd", "wakeInfo"))
                            }
                            "wakeInfo" -> WakeInfo.save(context, ev)
                            "error" -> throw java.io.IOException(ev.optString("reason"))
                        }
                        events.tryEmit(ev)
                    }
                }
            } catch (e: Exception) {
                out = null
                if (running) _conn.value = Conn.NotRunning
            }
            if (running) runCatching { Thread.sleep(1500) }
        }
    }

    companion object {
        const val PORT = 53318

        /** Fetches the image the daemon put on the clipboard (see ClipProvider). */
        fun fetchClip(name: String, into: File): Boolean = runCatching {
            Socket().use { s ->
                s.connect(InetSocketAddress("127.0.0.1", PORT), 1000)
                s.soTimeout = 5000
                writePlain(DataOutputStream(BufferedOutputStream(s.getOutputStream())), JSONObject().put("cmd", "getClip").put("name", name))
                val input = DataInputStream(BufferedInputStream(s.getInputStream()))
                val size = readPlain(input).optInt("size", -1)
                if (size < 0) return@use false
                val buf = ByteArray(size)
                input.readFully(buf)
                into.parentFile?.mkdirs()
                into.writeBytes(buf)
                true
            }
        }.getOrDefault(false)

        /**
         * Sends an image to the PCs' clipboards through the daemon. Blocking.
         * Returns null on success, or a reason to show the user.
         */
        fun pushImage(input: InputStream): String? {
            val bytes = try {
                val out = java.io.ByteArrayOutputStream()
                val buf = ByteArray(256 * 1024)
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    if (out.size() > 64 * 1024 * 1024) return "图片太大"
                }
                out.toByteArray()
            } catch (e: Exception) {
                android.util.Log.w("SyncBoard", "pushImage: reading the shared image failed", e)
                return "无法读取这张图片"
            }
            return try {
                Socket().use { s ->
                    s.connect(InetSocketAddress("127.0.0.1", PORT), 1000)
                    s.soTimeout = 30_000
                    val o = DataOutputStream(BufferedOutputStream(s.getOutputStream(), 256 * 1024))
                    writePlain(o, JSONObject().put("cmd", "pushImage").put("size", bytes.size))
                    o.write(bytes)
                    o.flush()
                    val r = readPlain(DataInputStream(BufferedInputStream(s.getInputStream())))
                    if (r.optBoolean("ok")) null else r.optString("reason", "发送失败")
                }
            } catch (e: java.net.ConnectException) {
                android.util.Log.w("SyncBoard", "pushImage: daemon not running")
                "手机服务未运行，请打开 SyncBoard 启动"
            } catch (e: Exception) {
                android.util.Log.w("SyncBoard", "pushImage failed", e)
                "发送失败：${e.message}"
            }
        }

        /** Streams one file to the PC through the daemon. Blocking. */
        fun sendFile(context: Context, name: String, size: Long, input: InputStream, pc: String? = null): Boolean =
            runCatching {
                Socket().use { s ->
                    s.connect(InetSocketAddress("127.0.0.1", PORT), 1000)
                    val o = DataOutputStream(BufferedOutputStream(s.getOutputStream(), 256 * 1024))
                    writePlain(o, JSONObject().put("cmd", "sendFile")
                        .put("name", name).put("size", size).put("pc", pc ?: ""))
                    val buf = ByteArray(256 * 1024)
                    var left = size
                    while (left > 0) {
                        val n = input.read(buf, 0, minOf(buf.size.toLong(), left).toInt())
                        if (n < 0) break
                        o.write(buf, 0, n)
                        left -= n
                    }
                    o.flush()
                    val r = readPlain(DataInputStream(BufferedInputStream(s.getInputStream())))
                    r.optBoolean("ok")
                }
            }.getOrDefault(false)
    }
}
