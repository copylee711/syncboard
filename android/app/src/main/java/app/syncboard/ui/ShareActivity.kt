package app.syncboard.ui

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.Toast

/** "Send to PC" in the system share sheet. Text goes to the PC clipboard, files to Downloads. */
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
        val text = i.getStringExtra(Intent.EXTRA_TEXT)
        when {
            uris.isNotEmpty() -> {
                Toast.makeText(this, "正在发送到电脑…", Toast.LENGTH_SHORT).show()
                sendUris(this, uris)
            }
            text != null -> {
                // The daemon syncs the clipboard, so putting text there sends it.
                getSystemService(android.content.ClipboardManager::class.java)
                    .setPrimaryClip(android.content.ClipData.newPlainText("SyncBoard", text))
                Toast.makeText(this, "已发送到电脑剪贴板", Toast.LENGTH_SHORT).show()
            }
        }
        finish()
    }
}
