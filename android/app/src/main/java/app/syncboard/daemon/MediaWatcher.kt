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
        mediaObserver?.let { Observers.unregister(it) }
        mediaObserver = null
        poll?.cancel(false)
        poll = null
        if (!t.any) {
            worker.execute { lastId = -1 }
            return
        }
        // Only images added from now on. Before the first unlock after a reboot MediaStore
        // isn't usable yet; [scan] retries until it is (issue #2).
        worker.execute { initLastId() }

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
        // Android 16 refuses observers from this daemon; then a cheap query (newer ids only)
        // every 1.5 s stands in. Otherwise it is a slow safety net for writers that don't notify.
        val registered = Observers.register(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, true, obs)
        if (registered) mediaObserver = obs
        poll = worker.scheduleWithFixedDelay({ scan() }, 1500, if (registered) 10_000 else 1500, TimeUnit.MILLISECONDS)
        Log.i(TAG, "watching new images: $t (observer ${if (registered) "on" else "off"})")
    }

    /** Sets [lastId] to MediaStore's newest id once the user is unlocked and MediaStore answers. */
    private fun initLastId() {
        if (lastId >= 0) return
        if (!userUnlocked()) return
        val id = maxId() ?: return
        lastId = id
        Log.i(TAG, "new images from MediaStore id $id on")
    }

    private fun userUnlocked(): Boolean = runCatching {
        ShellContext.init().getSystemService(android.os.UserManager::class.java)?.isUserUnlocked
    }.getOrNull() ?: true

    /**
     * MediaStore's newest image id, 0 when it has none; null when it can't be read (locked
     * after a reboot, MediaStore still starting), so the caller tries again later rather
     * than treating every existing image as new.
     */
    private fun maxId(): Long? = runCatching {
        val args = Bundle().apply {
            putString(ContentResolver.QUERY_ARG_SQL_SORT_ORDER, "${BaseColumns._ID} DESC")
            putInt(ContentResolver.QUERY_ARG_LIMIT, 1)
        }
        resolver.query(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, arrayOf(BaseColumns._ID), args, null)?.use { c ->
            // Some Android 10 ROMs ignore the sort and limit arguments: take the largest anyway.
            var max = 0L
            while (c.moveToNext()) max = maxOf(max, c.getLong(0))
            max
        }
    }.onFailure { Log.w(TAG, "MediaStore max id failed", it) }.getOrNull()

    private val power by lazy { runCatching { ShellContext.init().getSystemService(android.os.PowerManager::class.java) }.getOrNull() }
    /** Images MediaStore lists but whose file isn't readable yet: id → first seen (ms). */
    private val notReady = HashMap<Long, Long>()

    /** Handles images added since the last scan. Runs on [worker]. */
    private fun scan() {
        val t = targets
        if (!t.any) return
        if (lastId < 0) { initLastId(); return }
        // No screenshots or photos are taken with the screen off; they are picked up once it is on.
        if (runCatching { power?.isInteractive }.getOrNull() == false) return
        val uri = MediaStore.Images.Media.EXTERNAL_CONTENT_URI
        @Suppress("DEPRECATION")
        val cols = arrayOf(BaseColumns._ID, MediaStore.MediaColumns.DATA, MediaStore.MediaColumns.RELATIVE_PATH,
            MediaStore.MediaColumns.IS_PENDING, MediaStore.MediaColumns.MIME_TYPE, MediaStore.MediaColumns.SIZE,
            MediaStore.MediaColumns.DATE_ADDED, MediaStore.MediaColumns.DATE_MODIFIED)
        val rows = runCatching {
            resolver.query(uri, cols, "${BaseColumns._ID}>?", arrayOf(lastId.toString()), "${BaseColumns._ID} ASC")?.use { c ->
                buildList {
                    while (c.moveToNext()) add(Row(c.getLong(0), c.getString(1), c.getString(2), c.getInt(3) == 1,
                        c.getString(4), c.getLong(5), c.getLong(6), c.getLong(7)))
                }
            }.orEmpty()
        }.onFailure { Log.w(TAG, "MediaStore scan failed", it) }.getOrDefault(emptyList())
        // Stop short of an image still being written (pending, or published before its file
        // is complete, as some cameras do); it is looked at again on the next scans.
        val now = System.currentTimeMillis()
        // Only images that are really new: not old ones MediaStore re-indexed, a cloud album
        // restored, or anything left over when [lastId] couldn't be trusted (issue #2).
        val oldest = now / 1000 - FRESH_SECS
        var stop = false
        var newest: Pair<Row, File>? = null
        for (r in rows) {
            // Switched off or changed meanwhile: leave the rest to the new configuration.
            if (targets !== t) return
            val fresh = r.added >= oldest && (r.modified == 0L || r.modified >= oldest)
            val want = fresh && !r.pending && r.data != null && wanted(t, r.relPath)
            val f = r.data?.let(::File)
            val waiting = (r.pending || (want && !ready(f!!, r.size))) && now - notReady.getOrPut(r.id) { now } < GIVE_UP_MS
            if (waiting) { stop = true; continue }
            notReady.remove(r.id)
            if (!stop) lastId = r.id
            if (want) newest = r to f!!
        }
        // Only one image fits on a clipboard: a burst of several hands over the last one.
        newest?.let { (r, f) -> handle(f, android.content.ContentUris.withAppendedId(uri, r.id), r.mime) }
        // Forget images given up on or already past; keep the ones still being waited for.
        notReady.entries.removeAll { (id, seen) -> id <= lastId || now - seen >= GIVE_UP_MS * 4 }
    }

    /** The file is there and, when MediaStore knows its size, fully written. */
    private fun ready(f: File, size: Long) = f.isFile && f.length().let { it > 0 && it >= size }

    private class Row(val id: Long, val data: String?, val relPath: String?, val pending: Boolean, val mime: String?,
                      val size: Long, val added: Long, val modified: Long)

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
        private const val GIVE_UP_MS = 30_000L
        /** How old (seconds) an image may be and still count as new. */
        private const val FRESH_SECS = 120L
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
