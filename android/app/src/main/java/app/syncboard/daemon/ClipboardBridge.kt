package app.syncboard.daemon

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Handler
import android.os.Looper
import app.syncboard.proto.Crypto
import java.io.ByteArrayOutputStream
import java.io.File

/** Local clipboard content, as seen by the sync engine. */
sealed class Clip {
    abstract val hash: String

    class Text(val text: String) : Clip() {
        override val hash: String by lazy { Crypto.sha256(text.toByteArray()).toHexString() }
    }

    /** PNG bytes. */
    class Image(val png: ByteArray) : Clip() {
        override val hash: String by lazy { Crypto.sha256(png).toHexString() }
    }
}

private fun ByteArray.toHexString() = joinToString("") { "%02x".format(it) }

/**
 * Reads, writes and watches the system clipboard. In the daemon this runs as shell, which
 * Android lets read the clipboard from the background.
 *
 * Images written by us are served by [app.syncboard.ipc.ClipProvider] from [clipDir].
 */
class ClipboardBridge(context: Context, private val clipDir: File) {
    private val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    private val main = Handler(Looper.getMainLooper())
    private var lastTimestamp = -1L

    /** [onChange] runs on the main looper; do the reading elsewhere. */
    fun listen(onChange: () -> Unit) {
        main.post { cm.addPrimaryClipChangedListener { onChange() } }
    }

    /**
     * Returns the current clip if it is new and something we sync. Blocking (images are
     * read through the `content` command), so call it off the main thread.
     */
    fun readIfNew(maxImage: Int): Clip? {
        val clip = cm.primaryClip ?: return null
        val desc = clip.description
        val ts = desc.timestamp
        synchronized(this) {
            if (ts != 0L && ts == lastTimestamp) return null
            lastTimestamp = ts
        }
        if (desc.extras?.getBoolean(EXTRA_IS_SENSITIVE) == true) return null
        if (clip.itemCount == 0) return null
        val item = clip.getItemAt(0)
        item.uri?.let { uri ->
            if (uri.authority == PROVIDER_AUTHORITY) return null // our own write
            val mime = (0 until desc.mimeTypeCount).map { desc.getMimeType(it) }.firstOrNull { it.startsWith("image/") }
            if (mime != null) return readImage(uri, maxImage)?.let { Clip.Image(it) }
        }
        val text = item.text?.toString() ?: item.htmlText ?: return null
        return if (text.isEmpty()) null else Clip.Text(text)
    }

    fun setText(text: String) {
        main.post { cm.setPrimaryClip(ClipData.newPlainText(LABEL, text)) }
    }

    fun setImage(png: ByteArray) {
        clipDir.mkdirs()
        clipDir.listFiles()?.forEach { it.delete() }
        val name = "clip-${Crypto.randomId()}.png"
        File(clipDir, name).writeBytes(png)
        val uri = Uri.parse("content://$PROVIDER_AUTHORITY/$name")
        val clip = ClipData(ClipDescription(LABEL, arrayOf("image/png")), ClipData.Item(uri))
        main.post { cm.setPrimaryClip(clip) }
    }

    /** `content read` runs as shell, which the clipboard granted access to the URI. */
    private fun readImage(uri: Uri, max: Int): ByteArray? = runCatching {
        val p = ProcessBuilder("content", "read", "--uri", uri.toString()).redirectErrorStream(false).start()
        val bytes = p.inputStream.use { input ->
            val out = ByteArrayOutputStream()
            val buf = ByteArray(64 * 1024)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                out.write(buf, 0, n)
                if (out.size() > max * 2) return@use null
            }
            out.toByteArray()
        }
        p.waitFor()
        bytes?.let { toPng(it, max) }
    }.getOrNull()

    companion object {
        const val LABEL = "SyncBoard"
        const val PROVIDER_AUTHORITY = "app.syncboard.clip"
        private const val EXTRA_IS_SENSITIVE = "android.content.extra.IS_SENSITIVE"
        private val PNG_MAGIC = byteArrayOf(0x89.toByte(), 0x50, 0x4E, 0x47)

        /** Re-encodes to PNG (downscaling if needed) so the PC always gets PNG ≤ [max]. */
        fun toPng(bytes: ByteArray, max: Int): ByteArray? {
            if (bytes.size <= max && bytes.size > 4 && bytes.copyOf(4).contentEquals(PNG_MAGIC)) return bytes
            var bmp: Bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return null
            repeat(4) {
                val out = ByteArrayOutputStream()
                bmp.compress(Bitmap.CompressFormat.PNG, 100, out)
                if (out.size() <= max) return out.toByteArray()
                bmp = Bitmap.createScaledBitmap(bmp, bmp.width * 7 / 10, bmp.height * 7 / 10, true)
            }
            return null
        }
    }
}
