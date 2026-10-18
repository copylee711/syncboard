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
import android.util.Log
import app.syncboard.proto.Crypto
import java.io.ByteArrayOutputStream

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
 * Images we put on the clipboard stay in memory here; [app.syncboard.ipc.ClipProvider]
 * fetches them over IPC when another app pastes.
 */
class ClipboardBridge(context: Context) {
    private val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    val resolver: android.content.ContentResolver = context.contentResolver
    private val main = Handler(Looper.getMainLooper())
    private var lastTimestamp = -1L
    @Volatile private var image: Pair<String, ByteArray>? = null

    fun image(name: String): ByteArray? = image?.takeIf { it.first == name }?.second

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
        val mimes = (0 until desc.mimeTypeCount).map { desc.getMimeType(it) }
        // Log types and sizes only, never clipboard content.
        Log.d(TAG, "clip changed: $mimes, items=${clip.itemCount}")
        if (desc.extras?.getBoolean(EXTRA_IS_SENSITIVE) == true) {
            Log.d(TAG, "skipped: marked sensitive")
            return null
        }
        if (clip.itemCount == 0) return null
        val item = clip.getItemAt(0)
        item.uri?.let { uri ->
            if (uri.authority == PROVIDER_AUTHORITY || uri == ownUri) return null // our own write
            if (mimes.any { it.startsWith("image/") }) {
                val png = readImage(uri, maxImage) ?: return null
                return Clip.Image(png)
            }
        }
        val text = item.text?.toString() ?: item.htmlText
        if (text.isNullOrEmpty()) {
            Log.d(TAG, "skipped: no text and no image (uri authority=${item.uri?.authority})")
            return null
        }
        return Clip.Text(text)
    }

    fun setText(text: String) {
        main.post { cm.setPrimaryClip(ClipData.newPlainText(LABEL, text)) }
    }

    @Volatile private var ownUri: Uri? = null

    /**
     * Puts an image that already lives in MediaStore (a screenshot) on the clipboard by its
     * own URI. Pasting then works even when the SyncBoard app isn't running: ROMs such as
     * OriginOS won't start it for other apps, and [setImage]'s URIs need its provider.
     */
    fun setMediaImage(uri: Uri, mime: String) {
        ownUri = uri
        val clip = ClipData(ClipDescription(LABEL, arrayOf(mime)), ClipData.Item(uri))
        main.post { cm.setPrimaryClip(clip) }
    }

    /** The MediaStore URI and type of a file under shared storage, if it is indexed yet. */
    fun mediaUriOf(path: String): Pair<Uri, String>? = runCatching {
        val images = android.provider.MediaStore.Images.Media.EXTERNAL_CONTENT_URI
        @Suppress("DEPRECATION")
        val data = android.provider.MediaStore.MediaColumns.DATA
        resolver.query(images, arrayOf(android.provider.BaseColumns._ID, android.provider.MediaStore.MediaColumns.MIME_TYPE),
            "$data=?", arrayOf(path), null)?.use { c ->
            if (c.moveToFirst()) android.content.ContentUris.withAppendedId(images, c.getLong(0)) to (c.getString(1) ?: "image/png") else null
        }
    }.onFailure { Log.w(TAG, "MediaStore lookup failed", it) }.getOrNull()

    fun setImage(png: ByteArray) {
        val name = "clip-${Crypto.randomId()}.png"
        image = name to png
        val uri = Uri.parse("content://$PROVIDER_AUTHORITY/$name")
        val clip = ClipData(ClipDescription(LABEL, arrayOf("image/png")), ClipData.Item(uri))
        main.post { cm.setPrimaryClip(clip) }
    }

    /** `content read` runs as shell, which the clipboard granted access to the URI. */
    private fun readImage(uri: Uri, max: Int): ByteArray? = try {
        val p = ProcessBuilder("content", "read", "--uri", uri.toString()).start()
        val bytes = p.inputStream.use { input ->
            val out = ByteArrayOutputStream()
            val buf = ByteArray(64 * 1024)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                out.write(buf, 0, n)
                if (out.size() > MAX_RAW_IMAGE) return@use null
            }
            out.toByteArray()
        }
        val err = p.errorStream.bufferedReader().readText().trim()
        val code = p.waitFor()
        when {
            bytes == null -> { Log.w(TAG, "image from ${uri.authority} is over ${MAX_RAW_IMAGE / 1048576} MB, skipped"); null }
            bytes.isEmpty() -> { Log.w(TAG, "content read ${uri.authority} failed (exit $code): ${err.take(200)}"); null }
            else -> { Log.d(TAG, "read ${bytes.size} bytes from ${uri.authority}"); toPng(bytes, max) }
        }
    } catch (e: Exception) {
        Log.w(TAG, "reading image from ${uri.authority} failed", e)
        null
    }

    companion object {
        const val LABEL = "SyncBoard"
        const val PROVIDER_AUTHORITY = "app.syncboard.clip"
        private const val EXTRA_IS_SENSITIVE = "android.content.extra.IS_SENSITIVE"
        private const val TAG = "SyncBoard"
        private val PNG_MAGIC = byteArrayOf(0x89.toByte(), 0x50, 0x4E, 0x47)

        /** Largest encoded image we accept before decoding (photos can be big JPEGs). */
        const val MAX_RAW_IMAGE = 64 * 1024 * 1024

        /** Longest side after decoding; keeps a 50 MP photo from taking ~200 MB as a bitmap. */
        private const val MAX_SIDE = 4096

        /** Re-encodes to PNG (downscaling if needed) so the PC always gets PNG ≤ [max]. */
        fun toPng(bytes: ByteArray, max: Int): ByteArray? {
            if (bytes.size <= max && bytes.size > 4 && bytes.copyOf(4).contentEquals(PNG_MAGIC)) return bytes
            return try {
                val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
                if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
                    Log.w(TAG, "not a decodable image (${bytes.size} bytes, type ${bounds.outMimeType})")
                    return null
                }
                var sample = 1
                while (maxOf(bounds.outWidth, bounds.outHeight) / sample > MAX_SIDE) sample *= 2
                var bmp: Bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size,
                    BitmapFactory.Options().apply { inSampleSize = sample }) ?: run {
                    Log.w(TAG, "decode failed (${bounds.outWidth}x${bounds.outHeight} ${bounds.outMimeType})")
                    return null
                }
                repeat(4) {
                    val out = ByteArrayOutputStream()
                    bmp.compress(Bitmap.CompressFormat.PNG, 100, out)
                    if (out.size() <= max) {
                        Log.d(TAG, "png ${bmp.width}x${bmp.height}, ${out.size()} bytes (from ${bounds.outMimeType})")
                        return out.toByteArray()
                    }
                    bmp = Bitmap.createScaledBitmap(bmp, bmp.width * 7 / 10, bmp.height * 7 / 10, true)
                }
                Log.w(TAG, "image still over ${max / 1048576} MB after downscaling, skipped")
                null
            } catch (e: OutOfMemoryError) {
                Log.w(TAG, "out of memory converting a ${bytes.size}-byte image", e)
                null
            }
        }
    }
}
