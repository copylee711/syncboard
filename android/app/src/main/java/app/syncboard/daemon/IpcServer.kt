package app.syncboard.daemon

import android.os.Process
import android.util.Log
import app.syncboard.proto.readPlain
import app.syncboard.proto.writePlain
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.File
import java.io.FilterInputStream
import java.io.InputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.CopyOnWriteArrayList

/**
 * Control channel between the app and the daemon: plain frames (u32 + JSON) on
 * 127.0.0.1:[PORT]. Callers are identified by the uid that owns their socket in
 * /proc/net/tcp*: only the SyncBoard app and shell/root may talk to the daemon.
 *
 * Requests: {"cmd": status|pairQr|pairCode|unpair|settings|discover|reconnect|wakeInfo|imageFolders|openOnPc|selfUpdate|stop, ...}
 * Two connection types carry raw bytes after their first frame:
 *  - {"cmd":"sendFile",name,size,pc?} then `size` bytes, streamed to the PC;
 *  - {"cmd":"getClip",name} answered with {"size"} then the clipboard image bytes;
 *  - {"cmd":"pushImage",size} then `size` image bytes, put on the PCs' clipboards.
 */
class IpcServer private constructor(private val server: ServerSocket, private val appUid: () -> Int) {
    private val clients = CopyOnWriteArrayList<DataOutputStream>()
    private lateinit var engine: Engine
    private lateinit var bridge: ClipboardBridge

    fun broadcast(event: JSONObject) {
        clients.forEach { out -> runCatching { writePlain(out, event) }.onFailure { clients.remove(out) } }
    }

    fun start(engine: Engine, bridge: ClipboardBridge) {
        this.engine = engine
        this.bridge = bridge
        Thread({
            while (true) {
                val s = runCatching { server.accept() }.getOrNull() ?: continue
                Thread({ serve(s) }, "ipc-client").start()
            }
        }, "ipc").start()
    }

    private fun allowed(sock: Socket): Boolean {
        val uid = peerUid(sock.port) ?: return false
        return uid == 0 || uid == Process.SHELL_UID || uid == appUid()
    }

    private fun serve(sock: Socket) {
        sock.use {
            val input = DataInputStream(BufferedInputStream(sock.getInputStream()))
            val out = DataOutputStream(BufferedOutputStream(sock.getOutputStream()))
            if (!allowed(sock)) {
                Log.w(Engine.TAG, "ipc: refused connection from port ${sock.port}")
                return
            }
            val hello = runCatching { readPlain(input) }.getOrNull() ?: return
            when (hello.optString("cmd")) {
                "sendFile" -> {
                    val size = hello.optLong("size")
                    val ok = engine.sendFile(hello.optString("pc").ifEmpty { null }, Engine.sanitize(hello.optString("name")),
                        size, Limited(input, size))
                    writePlain(out, JSONObject().put("event", "sendFileResult").put("ok", ok))
                    return
                }
                "pushImage" -> {
                    val size = hello.optLong("size")
                    val reply = JSONObject()
                    if (size <= 0 || size > ClipboardBridge.MAX_RAW_IMAGE) {
                        reply.put("ok", false).put("reason", "图片太大")
                    } else {
                        val raw = ByteArray(size.toInt()).also { input.readFully(it) }
                        val png = ClipboardBridge.toPng(raw, Engine.MAX_IMAGE)
                        val sent = png?.let { engine.pushImage(it) } ?: 0
                        when {
                            png == null -> reply.put("ok", false).put("reason", "无法读取这张图片")
                            sent == 0 -> reply.put("ok", false).put("reason", "电脑未连接")
                            else -> reply.put("ok", true).put("pcs", sent)
                        }
                    }
                    writePlain(out, reply)
                    return
                }
                "getClip" -> {
                    val png = bridge.image(hello.optString("name"))
                    writePlain(out, JSONObject().put("size", png?.size ?: -1))
                    if (png != null) out.write(png)
                    out.flush()
                    return
                }
            }
            writePlain(out, JSONObject().put("event", "hello").put("version", VERSION)
                .put("pid", Process.myPid()).put("uid", Process.myUid()))
            writePlain(out, engine.status())
            // A one-shot client sends its command as the first frame.
            if (hello.optString("cmd") !in setOf("", "hello", "status")) runCatching { handle(hello, out) }
            clients.add(out)
            try {
                while (true) handle(readPlain(input), out)
            } catch (e: Exception) {
                Log.d(Engine.TAG, "ipc client gone: $e")
            } finally {
                clients.remove(out)
            }
        }
    }

    private fun handle(req: JSONObject, out: DataOutputStream) {
        when (req.optString("cmd")) {
            "status" -> writePlain(out, engine.status())
            "pairQr" -> engine.pairQr(req.getString("uri"), req.optBoolean("viaAdb"))
            "pairCode" -> engine.pairCode(req.getString("host"), req.optInt("port", Config.PORT))
            "unpair" -> engine.unpair(req.getString("id"))
            "settings" -> engine.updateSettings(req.getJSONObject("settings"))
            "reconnect" -> engine.reconnect()
            "wakeInfo" -> writePlain(out, engine.wakeInfo())
            "discover" -> Thread {
                val found = engine.discover(1500)
                runCatching { writePlain(out, JSONObject().put("event", "discovered").put("pcs", JSONArray(found))) }
            }.start()
            "imageFolders" -> Thread {
                val folders = JSONArray(MediaWatcher.imageFolders(bridge.resolver).map { (path, n) ->
                    JSONObject().put("path", path).put("count", n)
                })
                runCatching { writePlain(out, JSONObject().put("event", "imageFolders").put("folders", folders)) }
            }.start()
            "openOnPc" -> writePlain(out, JSONObject().put("event", "openOnPc").put("pcs", engine.openOnPc(req.optString("url"))))
            "selfUpdate" -> SelfUpdate.start(req.getString("version"), Main.stateDir) { broadcast(it) }
            "stop" -> {
                broadcast(JSONObject().put("event", "stopping"))
                Thread { Thread.sleep(200); System.exit(0) }.start()
            }
        }
    }

    /** Reads at most [left] bytes from the underlying stream. */
    private class Limited(input: InputStream, private var left: Long) : FilterInputStream(input) {
        override fun read(): Int = if (left <= 0) -1 else super.read().also { if (it >= 0) left-- }
        override fun read(b: ByteArray, off: Int, len: Int): Int {
            if (left <= 0) return -1
            val n = super.read(b, off, minOf(len.toLong(), left).toInt())
            if (n > 0) left -= n
            return n
        }
        override fun close() {}
    }

    companion object {
        const val PORT = 53318
        const val VERSION = 1

        /** Owner uid of the loopback socket whose local port is [clientPort] and peer is us. */
        fun peerUid(clientPort: Int): Int? {
            val want = "%04X".format(clientPort)
            val ours = "%04X".format(PORT)
            for (f in listOf("/proc/net/tcp6", "/proc/net/tcp")) {
                val lines = runCatching { File(f).readLines() }.getOrNull() ?: continue
                for (line in lines.drop(1)) {
                    val cols = line.trim().split(Regex("\\s+"))
                    if (cols.size < 8) continue
                    if (cols[1].substringAfterLast(':') == want && cols[2].substringAfterLast(':') == ours) {
                        return cols[7].toIntOrNull()
                    }
                }
            }
            return null
        }

        /** Sends one command to the running daemon (used by a second `--pair` launch). */
        fun sendTo(cmd: JSONObject) {
            runCatching {
                Socket(InetAddress.getByName("127.0.0.1"), PORT).use { s ->
                    s.soTimeout = 3000
                    val out = DataOutputStream(BufferedOutputStream(s.getOutputStream()))
                    writePlain(out, JSONObject().put("cmd", "hello"))
                    writePlain(out, cmd)
                    Thread.sleep(300)
                }
            }
        }

        /** Throws if another daemon already holds the port. */
        fun bind(appUid: () -> Int): IpcServer =
            IpcServer(ServerSocket(PORT, 16, InetAddress.getByName("127.0.0.1")), appUid)
    }
}
