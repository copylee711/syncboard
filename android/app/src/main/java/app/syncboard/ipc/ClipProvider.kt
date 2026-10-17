package app.syncboard.ipc

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.File
import java.io.FileNotFoundException

/**
 * Serves images the daemon put on the clipboard. The daemon keeps them in memory; on
 * first access they are copied into our private cache. Exported read-only so any app can
 * paste them.
 */
class ClipProvider : ContentProvider() {
    private fun dir() = File(requireNotNull(context).cacheDir, "clip")

    private fun fileFor(uri: Uri): File {
        val name = uri.lastPathSegment ?: throw FileNotFoundException()
        if (!name.matches(Regex("clip-[0-9a-f]{16}\\.png"))) throw FileNotFoundException(name)
        val f = File(dir(), name)
        if (!f.isFile) {
            dir().listFiles()?.forEach { it.delete() } // keep only the current image
            if (!DaemonClient.fetchClip(name, f)) throw FileNotFoundException(name)
        }
        return f
    }

    override fun onCreate() = true

    override fun getType(uri: Uri) = "image/png"

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw SecurityException("read-only")
        return ParcelFileDescriptor.open(fileFor(uri), ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun query(uri: Uri, projection: Array<out String>?, s: String?, a: Array<out String>?, o: String?): Cursor {
        val f = fileFor(uri)
        return MatrixCursor(arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)).apply {
            addRow(arrayOf<Any>(f.name, f.length()))
        }
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, s: String?, a: Array<out String>?) = 0
    override fun update(uri: Uri, v: ContentValues?, s: String?, a: Array<out String>?) = 0
}
