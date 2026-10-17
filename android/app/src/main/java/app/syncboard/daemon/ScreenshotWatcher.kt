package app.syncboard.daemon

import android.os.FileObserver
import android.util.Log
import java.io.File
import java.util.concurrent.Executors

/**
 * Optional "截屏自动复制": when a new screenshot appears, hand it to [onScreenshot]
 * (which puts it on the phone and PC clipboards). Screenshot tools and Gboard's
 * suggestions don't touch the system clipboard, so this watches the folders instead.
 */
class ScreenshotWatcher(
    private val resolver: android.content.ContentResolver,
    private val onScreenshot: (File) -> Unit,
) {
    private val worker = Executors.newSingleThreadExecutor()
    private val observers = mutableListOf<FileObserver>()
    @Volatile private var lastPath = ""

    @Synchronized
    fun setEnabled(on: Boolean) {
        observers.forEach { it.stopWatching() }
        observers.clear()
        if (!on) return
        for (dir in folders()) {
            val obs = object : FileObserver(dir, CLOSE_WRITE or MOVED_TO) {
                override fun onEvent(event: Int, path: String?) {
                    if (path == null || path.startsWith(".") || !IMAGE.matches(path)) return
                    val f = File(dir, path)
                    worker.execute { handle(f) }
                }
            }
            obs.startWatching()
            observers.add(obs)
            Log.i(TAG, "watching ${dir.path} for screenshots")
        }
        if (observers.isEmpty()) Log.w(TAG, "no screenshot folder found")
    }

    /**
     * Screenshot folders differ by vendor (DCIM/Screenshots, Pictures/Screenshots, localized
     * names...), so ask MediaStore where screenshot-looking images live, plus the defaults.
     */
    private fun folders(): List<File> {
        val found = runCatching {
            val col = android.provider.MediaStore.MediaColumns.RELATIVE_PATH
            resolver.query(
                android.provider.MediaStore.Images.Media.EXTERNAL_CONTENT_URI, arrayOf(col),
                "$col LIKE ? OR $col LIKE ? OR $col LIKE ? OR $col LIKE ?",
                arrayOf("%screenshot%", "%screen_shot%", "%截屏%", "%截图%"), null,
            )?.use { c -> buildSet { while (c.moveToNext()) c.getString(0)?.let { add(it.trimEnd('/')) } } }.orEmpty()
        }.onFailure { Log.w(TAG, "MediaStore lookup for screenshot folders failed", it) }.getOrDefault(emptySet())
        return (found.map { "$ROOT/$it" } + DEFAULT_DIRS).distinct().map(::File).filter { it.isDirectory }
    }

    private fun handle(f: File) {
        // CLOSE_WRITE and MOVED_TO can both fire for one screenshot.
        if (f.path == lastPath) return
        lastPath = f.path
        // Let the screenshot app finish writing (and renaming from .pending).
        Thread.sleep(400)
        if (!f.isFile || f.length() == 0L) return
        runCatching { onScreenshot(f) }.onFailure { Log.w(TAG, "screenshot ${f.name.substringAfterLast('.')} failed", it) }
    }

    companion object {
        private const val TAG = "SyncBoard"
        private val IMAGE = Regex(".*\\.(png|jpe?g|webp)$", RegexOption.IGNORE_CASE)
        private const val ROOT = "/storage/emulated/0"
        private val DEFAULT_DIRS = listOf("$ROOT/Pictures/Screenshots", "$ROOT/DCIM/Screenshots")
    }
}
