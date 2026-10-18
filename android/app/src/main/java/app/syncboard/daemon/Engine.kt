package app.syncboard.daemon

import android.util.Base64
import android.util.Log
import app.syncboard.proto.CHUNK_SIZE
import app.syncboard.proto.Crypto
import app.syncboard.proto.Ecdh
import app.syncboard.proto.Message
import app.syncboard.proto.RecordCipher
import app.syncboard.proto.SecureReader
import app.syncboard.proto.SecureWriter
import app.syncboard.proto.readPlain
import app.syncboard.proto.toHex
import app.syncboard.proto.writePlain
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.net.Socket
import java.net.SocketTimeoutException
import java.security.GeneralSecurityException
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Phone side of docs/protocol.md: keeps a connection to every paired PC, syncs the
 * clipboard both ways and moves files. Reports state changes through [emit].
 */
class Engine(
    private val config: Config,
    private val bridge: ClipboardBridge,
    private val downloadDir: File,
    private val emit: (JSONObject) -> Unit,
) {
    private val io = Executors.newCachedThreadPool()
    private val timer = Executors.newSingleThreadScheduledExecutor()
    private val sessions = ConcurrentHashMap<String, Session>()
    private val loops = ConcurrentHashMap<String, ConnLoop>()
    private val waiters = ConcurrentHashMap<String, Waiter>()
    private val seen = ArrayDeque<String>()

    private val media = MediaWatcher(bridge.resolver) { f, uri, mime ->
        val png = ClipboardBridge.toPng(f.readBytes(), MAX_IMAGE) ?: return@MediaWatcher
        // Paste on the phone too (our own clip is not re-sent). The image's MediaStore URI
        // works without the app running; when inotify saw it first it may take a moment
        // to be indexed.
        val known = uri?.let { it to (mime ?: "image/png") } ?: (0 until 5).firstNotNullOfOrNull { i ->
            if (i > 0) Thread.sleep(300)
            bridge.mediaUriOf(f.path)
        }
        if (known != null) bridge.setMediaImage(known.first, known.second) else bridge.setImage(png)
        val n = pushImage(png)
        Log.i(TAG, "new image copied (${png.size} bytes, $n PC(s))")
    }

    private fun applyMediaSettings() = with(config.settings) {
        media.configure(MediaWatcher.Targets(copyScreenshots, copyCameraPhotos, copyFolders.toList()))
    }

    /** Hash of the clipboard content both sides agree on (echo suppression). */
    @Volatile private var lastHash = ""

    fun start() {
        config.pcs.forEach { ensureLoop(it.id) }
        bridge.listen { io.execute { onLocalChange() } }
        applyToastSetting()
        applyMediaSettings()
        timer.scheduleWithFixedDelay({
            sessions.values.forEach { s -> io.execute { runCatching { s.writer.write(Message.of("t" to "ping")) } } }
        }, 30, 30, TimeUnit.SECONDS)
    }

    // ---- status / control (called from IPC) ----

    fun status(): JSONObject {
        val pcs = JSONArray()
        synchronized(config) {
            config.pcs.forEach { pc ->
                val s = sessions[pc.id]
                pcs.put(JSONObject().put("id", pc.id).put("name", pc.name).put("hosts", JSONArray(pc.hosts))
                    .put("connected", s != null).put("addr", s?.addr ?: JSONObject.NULL)
                    .put("state", if (s != null) "connected" else loops[pc.id]?.state ?: "idle"))
            }
        }
        return JSONObject().put("event", "status").put("deviceId", config.deviceId).put("name", config.name)
            .put("pcs", pcs).put("settings", config.settings.toJson())
    }

    private fun pushStatus() = emit(status())

    /**
     * What the app needs to wake a PC after a reboot, when this daemon is gone:
     * device id, and each PC's addresses and key (see protocol §6).
     */
    fun wakeInfo(): JSONObject = synchronized(config) {
        JSONObject().put("event", "wakeInfo").put("deviceId", config.deviceId)
            .put("pcs", JSONArray(config.pcs.map { it.toJson() }))
    }

    fun updateSettings(o: JSONObject) {
        val before = config.settings.hideAccessToast
        config.settings.update(o)
        config.save()
        if (before != config.settings.hideAccessToast) applyToastSetting()
        applyMediaSettings()
        pushStatus()
    }

    private fun disableDevOptionsSoon() = Thread {
        // Let the PC finish with adb first; the daemon itself doesn't need it.
        Thread.sleep(3000)
        Log.i(TAG, "paired over adb, switching developer options off")
        // On vivo also hide the entry again (vivo_development_show), as before the build-number
        // taps; the app can still switch them on for a restart.
        val keys = listOf("adb_wifi_enabled", "development_settings_enabled") +
            if (android.os.Build.MANUFACTURER.equals("vivo", ignoreCase = true)) listOf("vivo_development_show") else emptyList()
        for (key in keys) {
            runCatching { ProcessBuilder("settings", "put", "global", key, "0").start().waitFor() }
        }
    }.start()

    fun unpair(id: String) {
        loops.remove(id)?.shutdown()
        sessions.remove(id)?.let { s ->
            // Lets the PC forget this phone too (ignored when the PC removed us first).
            runCatching { s.writer.write(Message.of("t" to "unpaired")) }
            s.close()
        }
        config.remove(id)
        pushStatus()
    }

    /** Re-try connections now (e.g. the UI was opened or the network changed). */
    fun reconnect() = loops.values.forEach { it.wake() }

    private fun applyToastSetting() {
        val cmd = if (config.settings.hideAccessToast) {
            arrayOf("settings", "put", "secure", "clipboard_show_access_notifications", "0")
        } else {
            arrayOf("settings", "delete", "secure", "clipboard_show_access_notifications")
        }
        runCatching { ProcessBuilder(*cmd).start().waitFor() }
    }

    // ---- clipboard ----

    private fun onLocalChange() {
        val s = config.settings
        if (s.paused) return
        val clip = runCatching { bridge.readIfNew(MAX_IMAGE) }
            .onFailure { Log.w(TAG, "reading the clipboard failed", it) }
            .getOrNull() ?: return
        if (clip.hash == lastHash) return
        lastHash = clip.hash
        when (clip) {
            is Clip.Text -> if (s.syncText && clip.text.length <= MAX_TEXT) broadcast {
                it.writer.write(Message.of("t" to "clip.text", "id" to Crypto.randomId(), "hash" to clip.hash,
                    body = clip.text.toByteArray()))
            }
            is Clip.Image -> if (s.syncImages) broadcast { sendImage(it, clip) }
        }
    }

    private fun broadcast(action: (Session) -> Unit) {
        sessions.values.forEach { s ->
            io.execute {
                runCatching { action(s) }.onFailure {
                    Log.w(TAG, "sending to ${s.pc.name} failed", it)
                    s.close()
                }
            }
        }
    }

    /**
     * Puts an image on the PCs' clipboards because the user asked to (share sheet), so it
     * ignores the pause and image-sync settings. Returns how many PCs it was sent to.
     */
    fun pushImage(png: ByteArray): Int {
        val clip = Clip.Image(png)
        lastHash = clip.hash
        val n = sessions.size
        broadcast { sendImage(it, clip) }
        Log.i(TAG, "pushed image (${png.size} bytes) to $n PC(s)")
        return n
    }

    private fun sendImage(s: Session, clip: Clip.Image) {
        val id = Crypto.randomId()
        s.writer.write(Message.of("t" to "clip.image", "id" to id, "hash" to clip.hash, "mime" to "image/png",
            "size" to clip.png.size))
        var seq = 0
        var off = 0
        while (off < clip.png.size) {
            val end = minOf(off + CHUNK_SIZE, clip.png.size)
            s.writer.write(Message.of("t" to "chunk", "id" to id, "seq" to seq++, body = clip.png.copyOfRange(off, end)))
            off = end
        }
        s.writer.write(Message.of("t" to "end", "id" to id, "sha256" to clip.hash))
    }

    private fun applyRemote(clip: Clip) {
        val s = config.settings
        if (s.paused) return
        when (clip) {
            is Clip.Text -> if (!s.syncText) return
            is Clip.Image -> if (!s.syncImages) return
        }
        lastHash = clip.hash
        when (clip) {
            is Clip.Text -> bridge.setText(clip.text)
            is Clip.Image -> bridge.setImage(clip.png)
        }
    }

    // ---- pairing ----

    /**
     * [viaAdb]: a PC started us over adb to pair. Once paired, developer options can go
     * off again (see [Settings.autoDisableDevOptions]); not when the pairing went through
     * the USB cable's reverse tunnel, which needs adb to stay up.
     */
    fun pairQr(uri: String, viaAdb: Boolean = false) = io.execute {
        try {
            val q = android.net.Uri.parse(uri)
            val hosts = q.getQueryParameter("h").orEmpty().split(',').filter { it.isNotBlank() }
            val port = q.getQueryParameter("p")?.toIntOrNull() ?: Config.PORT
            val token = Base64.decode(q.getQueryParameter("t").orEmpty(), Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
            if (hosts.isEmpty() || token.size != 32) throw IOException("无效的二维码")
            var lastErr: Exception? = null
            for (host in hosts) {
                try {
                    pairWith(host, port, "qr", token, hosts)
                    if (viaAdb && host != "127.0.0.1" && config.settings.autoDisableDevOptions) disableDevOptionsSoon()
                    return@execute
                } catch (e: PairRejected) {
                    throw e
                } catch (e: Exception) {
                    lastErr = e
                }
            }
            throw IOException("无法连接电脑（${hosts.joinToString()}）: ${lastErr?.message}")
        } catch (e: Exception) {
            emit(JSONObject().put("event", "pairFailed").put("reason", e.message ?: e.toString()))
        }
    }

    fun pairCode(host: String, port: Int) = io.execute {
        try {
            pairWith(host, port, "code", null, listOf(host))
        } catch (e: Exception) {
            emit(JSONObject().put("event", "pairFailed").put("reason", e.message ?: e.toString()))
        }
    }

    private class PairRejected(msg: String) : IOException(msg)

    private fun pairWith(host: String, port: Int, mode: String, token: ByteArray?, allHosts: List<String>) {
        Socket().use { sock ->
            sock.connect(InetSocketAddress(host, port), 4000)
            sock.soTimeout = if (mode == "code") 130_000 else 15_000
            val input = DataInputStream(BufferedInputStream(sock.getInputStream()))
            val out = DataOutputStream(BufferedOutputStream(sock.getOutputStream()))
            val ecdh = Ecdh()
            val nonceC = Crypto.random(32)
            writePlain(out, JSONObject().put("t", "pair").put("v", 1).put("mode", mode)
                .put("id", config.deviceId).put("name", config.name).put("platform", "android")
                .put("pub", b64(ecdh.public)).put("nonce", b64(nonceC)))
            val resp = readPlain(input)
            val pubS = unb64(resp.getString("pub"))
            val nonceS = unb64(resp.getString("nonce"))
            val shared = ecdh.agree(pubS)
            val th = Crypto.transcript(ecdh.public, pubS, nonceC, nonceS)
            val pcName = resp.optString("name", "PC")
            if (token != null) {
                writePlain(out, JSONObject().put("t", "pair_proof").put("mac", b64(Crypto.hmac(token, "client".toByteArray(), th))))
            } else {
                emit(JSONObject().put("event", "pairCode").put("code", Crypto.pairingCode(th)).put("pc", pcName))
            }
            val ok = readPlain(input)
            if (ok.optString("t") != "pair_ok") throw PairRejected("电脑拒绝了配对")
            if (token != null) {
                val mac = unb64(ok.optString("mac"))
                if (!Crypto.ctEq(mac, Crypto.hmac(token, "server".toByteArray(), th))) throw PairRejected("电脑身份校验失败")
            }
            val hosts = (listOf(host) + allHosts).distinct().toMutableList()
            val pc = Pc(resp.getString("id"), pcName, Crypto.derivePsk(shared, token, th), hosts, port)
            config.upsert(pc)
            emit(JSONObject().put("event", "paired").put("id", pc.id).put("name", pc.name))
            loops.remove(pc.id)?.shutdown()
            ensureLoop(pc.id)
            pushStatus()
        }
    }

    // ---- connections ----

    private fun ensureLoop(id: String) {
        loops.computeIfAbsent(id) { ConnLoop(id).also { it.start() } }
    }

    private inner class ConnLoop(val pcId: String) : Thread("conn-$pcId") {
        @Volatile var running = true
        @Volatile var state = "connecting"
        private val lock = Object()

        fun shutdown() {
            running = false
            wake()
        }

        fun wake() = synchronized(lock) { lock.notifyAll() }

        override fun run() {
            var backoff = 1_000L
            while (running) {
                val pc = config.pc(pcId) ?: break
                var connected = false
                var wrongPc = false
                var refused = false
                for (host in pc.hosts.toList()) {
                    if (!running) break
                    try {
                        val s = connect(pc, host)
                        connected = true
                        state = "connected"
                        backoff = 1_000L
                        s.readLoop()
                    } catch (e: UnknownDevice) {
                        state = "rejected"
                        refused = true
                        pushStatus()
                        break
                    } catch (e: KeyMismatch) {
                        Log.w(TAG, "${pc.name}: pairing keys differ")
                        state = "keyMismatch"
                        refused = true
                        pushStatus()
                        break
                    } catch (e: WrongPc) {
                        // Another PC (or this one after a reset) now answers at this address.
                        Log.w(TAG, "$host: ${e.message}")
                        wrongPc = true
                    } catch (e: Exception) {
                        Log.d(TAG, "connect $host: $e")
                    }
                    if (connected) break
                }
                if (!running) break
                if (!connected && !refused) {
                    // Maybe the PC changed address; ask the LAN. (Not when it answered and
                    // refused us: it would be found at the same address and retried at once.)
                    discover(1200).firstOrNull { it.optString("id") == pcId }?.let { d ->
                        synchronized(config) { pc.rememberHost(d.getString("addr")) }
                        config.save()
                        backoff = 0
                    }
                    // Nowhere answers as this PC, but its address does with another id.
                    if (backoff != 0L && wrongPc) state = "rejected"
                }
                val stuck = state == "rejected" || state == "keyMismatch"
                if (!stuck) state = "connecting"
                pushStatus()
                if (backoff > 0) synchronized(lock) { lock.wait(backoff) }
                backoff = if (stuck) 60_000L else (backoff * 2).coerceIn(1_000L, 30_000L)
            }
            state = "idle"
        }
    }

    private class UnknownDevice : IOException("PC no longer knows this device")

    /** The PC at this address has a different device id than the one we paired with. */
    private class WrongPc(id: String) : IOException("answered as $id")

    /** The handshake reached the encrypted auth step but the PC's record didn't open: different psk. */
    private class KeyMismatch(cause: Throwable) : IOException("pairing key mismatch", cause)

    private fun connect(pc: Pc, host: String): Session {
        val sock = Socket()
        try {
            sock.connect(InetSocketAddress(host, pc.port), 3000)
            sock.tcpNoDelay = true
            sock.keepAlive = true
            sock.soTimeout = 15_000
            val input = DataInputStream(BufferedInputStream(sock.getInputStream(), 64 * 1024))
            val out = DataOutputStream(BufferedOutputStream(sock.getOutputStream(), 64 * 1024))
            val nonceC = Crypto.random(32)
            writePlain(out, JSONObject().put("t", "hello").put("v", 1).put("id", config.deviceId).put("nonce", b64(nonceC)))
            val resp = readPlain(input)
            if (resp.optString("t") != "hello") {
                if (resp.optString("code") == "unknown_device") throw UnknownDevice()
                throw IOException("refused: $resp")
            }
            val pcId = resp.optString("id")
            if (pcId.isNotEmpty() && pcId != pc.id) throw WrongPc(pcId)
            val (c2s, s2c) = Crypto.sessionKeys(pc.psk, nonceC, unb64(resp.getString("nonce")))
            val reader = SecureReader(input, RecordCipher(s2c))
            val writer = SecureWriter(out, RecordCipher(c2s))
            writer.write(Message.of("t" to "auth", "name" to config.name, "platform" to "android"))
            val auth = try {
                reader.read()
            } catch (e: GeneralSecurityException) {
                // AEADBadTagException: our psk for this PC isn't the one it holds for us.
                throw KeyMismatch(e)
            }
            if (auth.kind != "auth") throw IOException("expected auth")
            sock.soTimeout = 90_000
            val s = Session(pc, sock, reader, writer, host)
            sessions.put(pc.id, s)?.close()
            synchronized(config) { pc.rememberHost(host) }
            config.save()
            pushStatus()
            return s
        } catch (e: Exception) {
            runCatching { sock.close() }
            throw e
        }
    }

    private sealed class Inbound {
        class Image(val size: Int) : Inbound() {
            val md: MessageDigest = MessageDigest.getInstance("SHA-256")
            val buf = java.io.ByteArrayOutputStream(size)
        }

        class FileIn(val file: File, val name: String, val size: Long) : Inbound() {
            val md: MessageDigest = MessageDigest.getInstance("SHA-256")
            val out = FileOutputStream(file)
            var done = 0L
        }
    }

    private class Waiter {
        val latch = CountDownLatch(1)
        @Volatile var accepted = false
    }

    private inner class Session(
        val pc: Pc,
        val socket: Socket,
        val reader: SecureReader,
        val writer: SecureWriter,
        val addr: String,
    ) {
        private val inbound = HashMap<String, Inbound>()

        fun close() = runCatching { socket.close() }

        fun readLoop() {
            try {
                while (true) handle(reader.read())
            } catch (e: Exception) {
                Log.d(TAG, "session ${pc.name} ended: $e")
            } finally {
                close()
                sessions.remove(pc.id, this)
                inbound.values.forEach { if (it is Inbound.FileIn) { runCatching { it.out.close() }; it.file.delete() } }
                pushStatus()
            }
        }

        private fun handle(m: Message) {
            val id = m.str("id")
            when (m.kind) {
                "ping" -> io.execute { runCatching { writer.write(Message.of("t" to "pong")) } }
                "unpaired" -> {
                    Log.i(TAG, "${pc.name} removed this phone")
                    emit(JSONObject().put("event", "unpairedByPc").put("name", pc.name))
                    io.execute { unpair(pc.id) }
                }
                "service.stop" -> {
                    Log.i(TAG, "stopped by ${pc.name}")
                    emit(JSONObject().put("event", "stopping"))
                    Thread { Thread.sleep(200); System.exit(0) }.start()
                }
                "clip.text" -> if (remember(id) && m.body.size <= MAX_TEXT) applyRemote(Clip.Text(String(m.body, Charsets.UTF_8)))
                "clip.image" -> {
                    val size = m.header.optLong("size")
                    if (remember(id) && size <= MAX_IMAGE) inbound[id] = Inbound.Image(size.toInt())
                }
                "file.offer" -> {
                    val name = sanitize(m.str("name"))
                    val size = m.header.optLong("size")
                    if (!config.settings.autoAcceptFiles) {
                        send(Message.of("t" to "file.reject", "id" to id))
                        return
                    }
                    try {
                        downloadDir.mkdirs()
                        val f = uniqueFile(downloadDir, name)
                        inbound[id] = Inbound.FileIn(f, name, size)
                        send(Message.of("t" to "file.accept", "id" to id))
                        emitFile("fileProgress", id, name, 0, size, true)
                    } catch (e: IOException) {
                        send(Message.of("t" to "file.reject", "id" to id))
                    }
                }
                "chunk" -> when (val ib = inbound[id]) {
                    is Inbound.Image -> {
                        ib.md.update(m.body)
                        ib.buf.write(m.body)
                        if (ib.buf.size() > ib.size) inbound.remove(id)
                    }
                    is Inbound.FileIn -> {
                        ib.md.update(m.body)
                        ib.out.write(m.body)
                        val before = ib.done * 50 / maxOf(ib.size, 1)
                        ib.done += m.body.size
                        if (ib.done * 50 / maxOf(ib.size, 1) != before) emitFile("fileProgress", id, ib.name, ib.done, ib.size, true)
                    }
                    null -> {}
                }
                "end" -> {
                    val want = m.str("sha256")
                    when (val ib = inbound.remove(id)) {
                        is Inbound.Image -> if (ib.md.digest().toHex() == want) applyRemote(Clip.Image(ib.buf.toByteArray()))
                        is Inbound.FileIn -> {
                            ib.out.close()
                            if (ib.md.digest().toHex() == want) {
                                emit(JSONObject().put("event", "fileDone").put("id", id).put("name", ib.name)
                                    .put("path", ib.file.path).put("incoming", true))
                            } else {
                                ib.file.delete()
                                emitFailed(id, ib.name, "校验失败")
                            }
                        }
                        null -> {}
                    }
                }
                "file.cancel" -> (inbound.remove(id) as? Inbound.FileIn)?.let {
                    it.out.close()
                    it.file.delete()
                    emitFailed(id, it.name, "对方已取消")
                }
                "file.accept", "file.reject" -> waiters[id]?.let {
                    it.accepted = m.kind == "file.accept"
                    it.latch.countDown()
                }
            }
        }

        fun send(m: Message) = io.execute { runCatching { writer.write(m) }.onFailure { close() } }
    }

    // ---- files ----

    /** Streams a file to a PC (the first connected one if [pcId] is null). Blocking. */
    fun sendFile(pcId: String?, name: String, size: Long, input: InputStream): Boolean {
        val s = (pcId?.let { sessions[it] } ?: sessions.values.firstOrNull()) ?: run {
            emitFailed("", name, "电脑未连接")
            return false
        }
        val id = Crypto.randomId()
        val w = Waiter()
        waiters[id] = w
        try {
            s.writer.write(Message.of("t" to "file.offer", "id" to id, "name" to name, "size" to size))
            if (!w.latch.await(120, TimeUnit.SECONDS) || !w.accepted) {
                emitFailed(id, name, "对方未接收")
                return false
            }
            val md = MessageDigest.getInstance("SHA-256")
            val buf = ByteArray(CHUNK_SIZE)
            var done = 0L
            var seq = 0
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                if (n == 0) continue
                md.update(buf, 0, n)
                s.writer.write(Message.of("t" to "chunk", "id" to id, "seq" to seq++, body = buf.copyOf(n)))
                val before = done * 50 / maxOf(size, 1)
                done += n
                if (done * 50 / maxOf(size, 1) != before) emitFile("fileProgress", id, name, done, size, false)
            }
            s.writer.write(Message.of("t" to "end", "id" to id, "sha256" to md.digest().toHex()))
            emit(JSONObject().put("event", "fileDone").put("id", id).put("name", name).put("incoming", false))
            return true
        } catch (e: Exception) {
            runCatching { s.writer.write(Message.of("t" to "file.cancel", "id" to id)) }
            emitFailed(id, name, e.message ?: "发送失败")
            return false
        } finally {
            waiters.remove(id)
        }
    }

    private fun emitFile(event: String, id: String, name: String, done: Long, total: Long, incoming: Boolean) =
        emit(JSONObject().put("event", event).put("id", id).put("name", name).put("done", done).put("total", total)
            .put("incoming", incoming))

    private fun emitFailed(id: String, name: String, reason: String) =
        emit(JSONObject().put("event", "fileFailed").put("id", id).put("name", name).put("reason", reason))

    // ---- discovery ----

    /** Broadcasts a discovery probe and returns the PCs that answered (with "addr"). */
    fun discover(waitMs: Int): List<JSONObject> {
        val found = LinkedHashMap<String, JSONObject>()
        runCatching {
            DatagramSocket().use { sock ->
                sock.broadcast = true
                sock.soTimeout = 300
                val probe = JSONObject().put("t", "discover").put("v", 1).put("id", config.deviceId).toString().toByteArray()
                broadcastAddresses().forEach { addr ->
                    runCatching { sock.send(DatagramPacket(probe, probe.size, addr, Config.PORT)) }
                }
                val deadline = System.currentTimeMillis() + waitMs
                val buf = ByteArray(2048)
                while (System.currentTimeMillis() < deadline) {
                    val p = DatagramPacket(buf, buf.size)
                    try {
                        sock.receive(p)
                    } catch (e: SocketTimeoutException) {
                        continue
                    }
                    runCatching {
                        val o = JSONObject(String(p.data, 0, p.length, Charsets.UTF_8))
                        if (o.optString("t") == "here") found[o.getString("id")] = o.put("addr", p.address.hostAddress)
                    }
                }
            }
        }
        return found.values.toList()
    }

    private fun broadcastAddresses(): List<InetAddress> {
        val out = mutableListOf<InetAddress>(InetAddress.getByName("255.255.255.255"))
        runCatching {
            NetworkInterface.getNetworkInterfaces()?.toList()?.filter { it.isUp && !it.isLoopback }?.forEach { ni ->
                ni.interfaceAddresses.mapNotNullTo(out) { it.broadcast }
            }
        }
        return out.distinct()
    }

    // ---- helpers ----

    private fun remember(id: String): Boolean = synchronized(seen) {
        if (id in seen) return false
        seen.addLast(id)
        if (seen.size > 128) seen.removeFirst()
        true
    }

    companion object {
        const val TAG = "SyncBoard"
        const val MAX_TEXT = 1024 * 1024
        const val MAX_IMAGE = 20 * 1024 * 1024

        fun b64(b: ByteArray): String = Base64.encodeToString(b, Base64.NO_WRAP)
        fun unb64(s: String): ByteArray = Base64.decode(s, Base64.NO_WRAP)

        fun sanitize(name: String): String {
            val base = name.substringAfterLast('/').substringAfterLast('\\')
            val cleaned = base.map { if (it.isISOControl() || it in "<>:\"/\\|?*") '_' else it }.joinToString("")
                .trim().trim('.')
            return cleaned.ifEmpty { "file" }
        }

        fun uniqueFile(dir: File, name: String): File {
            var f = File(dir, name)
            if (!f.exists()) return f
            val dot = name.lastIndexOf('.').takeIf { it > 0 } ?: name.length
            val stem = name.substring(0, dot)
            val ext = name.substring(dot)
            var n = 1
            while (f.exists()) f = File(dir, "$stem (${n++})$ext")
            return f
        }
    }
}
