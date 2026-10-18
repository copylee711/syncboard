package app.syncboard.daemon

import android.content.ContentResolver
import android.database.ContentObserver
import android.net.Uri
import android.os.Bundle
import android.os.FileObserver
import android.os.Handler
import android.os.Looper
import android.provider.BaseColumns
import android.provider.MediaStore
import android.util.Log
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

/**
 * Optional "截屏后自动复制" and "新照片自动复制": when a new image appears in a watched
 * place (screenshots, the camera folder, folders the user picked), hand it to [onImage]
 * (which puts it on the phone and PC clipboards). Screenshot tools and the camera don't
 * touch the system clipboard, so this watches storage instead.
 *
 * Two triggers feed one queue: a MediaStore observer, which sees every image however it
 * was written (some writers, such as vivo's home-screen screenshots, bypass the FUSE view,
 * so inotify on /storage/emulated/0 never fires for them), and inotify on the screenshot
 * folders, which fires a little sooner. Each image is handled once.
 */
class MediaWatcher(
    private val resolver: ContentResolver,
    /** The file, and its MediaStore URI and type when known. */
    private val onImage: (File, Uri?, String?) -> Unit,
) {
    /** What to watch. Folders are MediaStore relative paths, like "Pictures/WeiXin". */
    data class Targets(val screenshots: Boolean, val camera: Boolean, val folders: List<String>) {
        val any get() = screenshots || camera || folders.isNotEmpty()
    }

    private val worker = Executors.newSingleThreadScheduledExecutor()
    private val main = Handler(Looper.getMainLooper())
    private val observers = mutableListOf<FileObserver>()
    private var mediaObserver: ContentObserver? = null
    private var poll: ScheduledFuture<*>? = null
    @Volatile private var targets = Targets(false, false, emptyList())
    /** Highest MediaStore id looked at (only touched on [worker]); newer rows are candidates. */
    private var lastId = -1L
    private val handled = ArrayDeque<String>()

    @Synchronized
    fun configure(t: Targets) {
        if (t == targets) return
        targets = t
        observers.forEach { it.stopWatching() }
        observers.clear()
        mediaObserver?.let { runCatching { resolver.unregisterContentObserver(it) } }
        mediaObserver = null
        poll?.cancel(false)
        poll = null
        if (!t.any) {
            worker.execute { lastId = -1 }
            return
        }
        // Only images added from now on.
        worker.execute { if (lastId < 0) lastId = maxId() }

        if (t.screenshots) {
            for (dir in screenshotFolders()) {
                val obs = object : FileObserver(dir, CLOSE_WRITE or MOVED_TO) {
                    override fun onEvent(event: Int, path: String?) {
                        if (path == null || path.startsWith(".") || !IMAGE.matches(path)) return
                        // Let the screenshot app finish writing (and renaming from .pending).
                        worker.schedule({ handle(File(dir, path), null, null) }, 400, TimeUnit.MILLISECONDS)
                    }
                }
                obs.startWatching()
                observers.add(obs)
                Log.i(TAG, "watching ${dir.path} for screenshots")
            }
        }

        val obs = object : ContentObserver(main) {
            override fun onChange(selfChange: Boolean) {
                // Several notifications arrive per image (insert, then publish); a scan covers them all.
                worker.schedule({ scan() }, 300, TimeUnit.MILLISECONDS)
            }
        }
        val registered = runCatching {
            resolver.registerContentObserver(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, true, obs)
        }.onFailure { Log.w(TAG, "MediaStore observer failed, polling instead", it) }.isSuccess
        if (registered) mediaObserver = obs
        // A slow safety net either way, in case a writer doesn't notify.
        poll = worker.scheduleWithFixedDelay({ scan() }, 3, if (registered) 10 else 2, TimeUnit.SECONDS)
        Log.i(TAG, "watching new images: $t (observer ${if (registered) "on" else "off"})")
    }

    private fun maxId(): Long = runCatching {
        val args = Bundle().apply {
            putStringArray(ContentResolver.QUERY_ARG_SORT_COLUMNS, arrayOf(BaseColumns._ID))
            putInt(ContentResolver.QUERY_ARG_SORT_DIRECTION, ContentResolver.QUERY_SORT_DIRECTION_DESCENDING)
            putInt(ContentResolver.QUERY_ARG_LIMIT, 1)
        }
        resolver.query(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, arrayOf(BaseColumns._ID), args, null)
            ?.use { c -> if (c.moveToFirst()) c.getLong(0) else 0L } ?: 0L
    }.onFailure { Log.w(TAG, "MediaStore max id failed", it) }.getOrDefault(0L)

    /** Handles images added since the last scan. Runs on [worker]. */
    private fun scan() {
        val t = targets
        if (!t.any || lastId < 0) return
        val uri = MediaStore.Images.Media.EXTERNAL_CONTENT_URI
        @Suppress("DEPRECATION")
        val cols = arrayOf(BaseColumns._ID, MediaStore.MediaColumns.DATA, MediaStore.MediaColumns.RELATIVE_PATH,
            MediaStore.MediaColumns.IS_PENDING, MediaStore.MediaColumns.MIME_TYPE)
        val rows = runCatching {
            resolver.query(uri, cols, "${BaseColumns._ID}>?", arrayOf(lastId.toString()), "${BaseColumns._ID} ASC")?.use { c ->
                buildList { while (c.moveToNext()) add(Row(c.getLong(0), c.getString(1), c.getString(2), c.getInt(3) == 1, c.getString(4))) }
            }.orEmpty()
        }.onFailure { Log.w(TAG, "MediaStore scan failed", it) }.getOrDefault(emptyList())
        // Stop short of an image still being written; it is looked at again once published.
        val firstPending = rows.firstOrNull { it.pending }?.id
        for (r in rows) {
            if (r.pending) continue
            if (firstPending == null || r.id < firstPending) lastId = r.id
            if (r.data == null || !wanted(t, r.relPath)) continue
            handle(File(r.data), android.content.ContentUris.withAppendedId(uri, r.id), r.mime)
        }
    }

    private class Row(val id: Long, val data: String?, val relPath: String?, val pending: Boolean, val mime: String?)

    private fun wanted(t: Targets, relPath: String?): Boolean {
        val rel = relPath?.trimEnd('/') ?: return false
        val lower = rel.lowercase()
        if (t.screenshots && SCREENSHOT_WORDS.any { it in lower }) return true
        if (t.camera && (lower == "dcim/camera" || lower.startsWith("dcim/camera/"))) return true
        return t.folders.any { it.trimEnd('/').equals(rel, ignoreCase = true) }
    }

    /**
     * Screenshot folders differ by vendor (DCIM/Screenshots, Pictures/Screenshots, localized
     * names...), so ask MediaStore where screenshot-looking images live, plus the defaults.
     */
    private fun screenshotFolders(): List<File> {
        val found = runCatching {
            val col = MediaStore.MediaColumns.RELATIVE_PATH
            resolver.query(
                MediaStore.Images.Media.EXTERNAL_CONTENT_URI, arrayOf(col),
                SCREENSHOT_WORDS.joinToString(" OR ") { "$col LIKE ?" },
                SCREENSHOT_WORDS.map { "%$it%" }.toTypedArray(), null,
            )?.use { c -> buildSet { while (c.moveToNext()) c.getString(0)?.let { add(it.trimEnd('/')) } } }.orEmpty()
        }.onFailure { Log.w(TAG, "MediaStore lookup for screenshot folders failed", it) }.getOrDefault(emptySet())
        return (found.map { "$ROOT/$it" } + DEFAULT_DIRS).distinct().map(::File).filter { it.isDirectory }
    }

    /** Runs on [worker]. */
    private fun handle(f: File, uri: Uri?, mime: String?) {
        // inotify and MediaStore both report the same image (and inotify can fire twice).
        if (f.path in handled) return
        if (!f.isFile || f.length() == 0L) return
        handled.addLast(f.path)
        while (handled.size > 50) handled.removeFirst()
        runCatching { onImage(f, uri, mime) }.onFailure { Log.w(TAG, "new image ${f.name.substringAfterLast('.')} failed", it) }
    }

    companion object {
        private const val TAG = "SyncBoard"
        private val IMAGE = Regex(".*\\.(png|jpe?g|webp|heic)$", RegexOption.IGNORE_CASE)
        private val SCREENSHOT_WORDS = listOf("screenshot", "screen_shot", "截屏", "截图")
        private const val ROOT = "/storage/emulated/0"
        private val DEFAULT_DIRS = listOf("$ROOT/Pictures/Screenshots", "$ROOT/DCIM/Screenshots")

        /** Folders that hold images, with how many, most first (for the folder picker). */
        fun imageFolders(resolver: ContentResolver): List<Pair<String, Int>> = runCatching {
            val col = MediaStore.MediaColumns.RELATIVE_PATH
            resolver.query(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, arrayOf(col), null, null, null)?.use { c ->
                val counts = HashMap<String, Int>()
                while (c.moveToNext()) c.getString(0)?.trimEnd('/')?.takeIf { it.isNotEmpty() }?.let { counts[it] = (counts[it] ?: 0) + 1 }
                counts.entries.sortedByDescending { it.value }.map { it.key to it.value }
            }.orEmpty()
        }.onFailure { Log.w(TAG, "listing image folders failed", it) }.getOrDefault(emptyList())
    }
}
