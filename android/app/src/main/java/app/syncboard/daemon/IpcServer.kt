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
 * Control channel between the app UI and the daemon: plain frames (u32 + JSON) on
 * 127.0.0.1:[PORT]. The first frame must carry the key the app wrote to [keyFile]; only
 * the app (and shell) can read that file.
 *
 * Requests: {"cmd": status|pairQr|pairCode|unpair|settings|discover|reconnect|stop, ...}
 * A connection whose first frame is {"cmd":"sendFile",name,size,pc?} is followed by
 * `size` raw bytes, which are streamed to the PC.
 */
class IpcServer private constructor(private val server: ServerSocket, private val keyFile: File) {
    private val clients = CopyOnWriteArrayList<DataOutputStream>()
    private lateinit var engine: Engine

    fun broadcast(event: JSONObject) {
        clients.forEach { out -> runCatching { writePlain(out, event) }.onFailure { clients.remove(out) } }
    }

    fun start(engine: Engine) {
        this.engine = engine
        Thread({
            while (true) {
                val s = runCatching { server.accept() }.getOrNull() ?: continue
                Thread({ serve(s) }, "ipc-client").start()
            }
        }, "ipc").start()
    }

    private fun serve(sock: Socket) {
        sock.use {
            val input = DataInputStream(BufferedInputStream(sock.getInputStream()))
            val out = DataOutputStream(BufferedOutputStream(sock.getOutputStream()))
            val hello = runCatching { readPlain(input) }.getOrNull() ?: return
            val key = runCatching { keyFile.readText().trim() }.getOrDefault("")
            if (key.length < 16 || hello.optString("key") != key) {
                writePlain(out, JSONObject().put("event", "error").put("reason", "bad key"))
                return
            }
            if (hello.optString("cmd") == "sendFile") {
                val size = hello.optLong("size")
                val ok = engine.sendFile(hello.optString("pc").ifEmpty { null }, Engine.sanitize(hello.optString("name")),
                    size, Limited(input, size))
                writePlain(out, JSONObject().put("event", "sendFileResult").put("ok", ok))
                return
            }
            writePlain(out, JSONObject().put("event", "hello").put("version", VERSION)
                .put("pid", Process.myPid()).put("uid", Process.myUid()))
            writePlain(out, engine.status())
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
            "pairQr" -> engine.pairQr(req.getString("uri"))
            "pairCode" -> engine.pairCode(req.getString("host"), req.optInt("port", Config.PORT))
            "unpair" -> engine.unpair(req.getString("id"))
            "settings" -> engine.updateSettings(req.getJSONObject("settings"))
            "reconnect" -> engine.reconnect()
            "discover" -> Thread {
                val found = engine.discover(1500)
                runCatching { writePlain(out, JSONObject().put("event", "discovered").put("pcs", JSONArray(found))) }
            }.start()
            "stop" -> {
                writePlain(out, JSONObject().put("event", "stopping"))
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

        /** Throws if another daemon already holds the port. */
        fun bind(keyFile: File): IpcServer =
            IpcServer(ServerSocket(PORT, 16, InetAddress.getByName("127.0.0.1")), keyFile)
    }
}
