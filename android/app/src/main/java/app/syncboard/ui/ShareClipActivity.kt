package app.syncboard.ui

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.widget.Toast
import app.syncboard.ipc.DaemonClient
import kotlin.concurrent.thread

/**
 * "复制到电脑剪贴板" in the share sheet. Gallery apps and Gboard's screenshot suggestions
 * often keep images out of the system clipboard, so sharing is the reliable way to get
 * one onto the PC clipboard.
 */
class ShareClipActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        @Suppress("DEPRECATION")
        val uri: Uri? = intent.takeIf { it.action == Intent.ACTION_SEND }?.getParcelableExtra(Intent.EXTRA_STREAM)
        if (uri == null) {
            Log.w(TAG, "share: no image in ${intent.action} (type ${intent.type})")
            finish()
            return
        }
        val app = applicationContext
        // Open while this activity still holds the share's URI grant.
        val stream = try {
            contentResolver.openInputStream(uri)
        } catch (e: Exception) {
            Log.w(TAG, "share: cannot open image from ${uri.authority}", e)
            null
        }
        // Finish only when done: some ROMs (OriginOS) kill the process as soon as no
        // activity is left, which would cut the transfer off.
        thread {
            // pushImage returns null on success, so check the stream separately.
            val error = if (stream == null) "无法读取这张图片" else stream.use { DaemonClient.pushImage(it) }
            if (error != null) Log.w(TAG, "share: not copied: $error") else Log.i(TAG, "share: copied image to PC clipboard")
            Handler(Looper.getMainLooper()).post {
                Toast.makeText(app, error ?: "已复制到电脑剪贴板", Toast.LENGTH_SHORT).show()
                finish()
            }
        }
    }

    private companion object {
        const val TAG = "SyncBoard"
    }
}
