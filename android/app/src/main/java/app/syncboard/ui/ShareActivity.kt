package app.syncboard.ui

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.widget.Toast

/**
 * "发送到电脑" in the share sheet. Text goes to the PC clipboard; files are opened here
 * (while the URI grants last) and sent by [SendService], with the progress in a
 * notification. No window: OriginOS showed one in a small-window frame.
 */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val i = intent
        @Suppress("DEPRECATION")
        val uris: List<Uri> = when (i.action) {
            Intent.ACTION_SEND -> listOfNotNull(i.getParcelableExtra(Intent.EXTRA_STREAM))
            Intent.ACTION_SEND_MULTIPLE -> i.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
            else -> emptyList()
        }
        val sharedText = i.getStringExtra(Intent.EXTRA_TEXT)
        when {
            uris.isNotEmpty() -> {
                val items = uris.mapNotNull { open(it) }
                if (items.isEmpty()) Toast.makeText(this, "无法读取要发送的文件", Toast.LENGTH_SHORT).show()
                else runCatching { SendService.start(this, items) }
                    .onFailure { Toast.makeText(this, "发送失败：${it.message}", Toast.LENGTH_SHORT).show() }
            }
            sharedText != null -> {
                // The daemon syncs the clipboard, so putting text there sends it.
                getSystemService(android.content.ClipboardManager::class.java)
                    .setPrimaryClip(android.content.ClipData.newPlainText("SyncBoard", sharedText))
                Toast.makeText(this, "已发送到电脑剪贴板", Toast.LENGTH_SHORT).show()
            }
        }
        finish()
    }

    private fun open(uri: Uri): SendService.Item? = runCatching {
        val (name, size) = contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
            ?.use { c -> if (c.moveToFirst()) (c.getString(0) ?: "file") to c.getLong(1) else null } ?: ("file" to -1L)
        val fd = contentResolver.openFileDescriptor(uri, "r") ?: return null
        val len = if (size >= 0) size else fd.statSize
        if (len < 0) { fd.close(); return null }
        SendService.Item(name, len, fd)
    }.onFailure { android.util.Log.w("SyncBoard", "share: ${uri.authority} unreadable", it) }.getOrNull()
}
