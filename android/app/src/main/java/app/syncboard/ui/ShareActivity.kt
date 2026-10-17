package app.syncboard.ui

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.view.Gravity
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.TextView
import android.widget.Toast

/**
 * "发送到电脑" in the share sheet. Text goes to the PC clipboard, files to Downloads.
 * Stays open (as a small dialog) until the transfer ends: OriginOS kills the process as
 * soon as no activity is left.
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
                setFinishOnTouchOutside(false)
                val label = TextView(this).apply { text = "正在发送到电脑…"; textSize = 15f }
                val pad = (24 * resources.displayMetrics.density).toInt()
                setContentView(LinearLayout(this).apply {
                    orientation = LinearLayout.HORIZONTAL
                    gravity = Gravity.CENTER_VERTICAL
                    setPadding(pad, pad, pad, pad)
                    addView(ProgressBar(this@ShareActivity).apply { isIndeterminate = true })
                    addView(label, LinearLayout.LayoutParams(0, -2, 1f).apply { marginStart = pad / 2 })
                })
                sendUris(this, uris,
                    onProgress = { n, total, name -> label.text = if (total > 1) "正在发送 ${n + 1}/$total：$name" else "正在发送：$name" },
                    onDone = { finish() })
            }
            sharedText != null -> {
                // The daemon syncs the clipboard, so putting text there sends it.
                getSystemService(android.content.ClipboardManager::class.java)
                    .setPrimaryClip(android.content.ClipData.newPlainText("SyncBoard", sharedText))
                Toast.makeText(this, "已发送到电脑剪贴板", Toast.LENGTH_SHORT).show()
                finish()
            }
            else -> finish()
        }
    }
}
