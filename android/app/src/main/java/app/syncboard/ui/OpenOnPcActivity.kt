package app.syncboard.ui

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.Toast
import app.syncboard.ipc.DaemonClient
import kotlin.concurrent.thread

/** "在电脑上打开" in the share sheet: opens a shared link in the PC's browser. */
class OpenOnPcActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent.getStringExtra(Intent.EXTRA_TEXT).orEmpty()
        val url = URL.find(text)?.value
        val app = applicationContext
        if (url == null) {
            Toast.makeText(app, "没有找到网页链接", Toast.LENGTH_SHORT).show()
            finish()
            return
        }
        thread {
            val error = DaemonClient.openOnPc(url)
            Handler(Looper.getMainLooper()).post {
                Toast.makeText(app, error ?: "已在电脑上打开", Toast.LENGTH_SHORT).show()
                finish()
            }
        }
    }

    private companion object {
        val URL = Regex("""https?://[^\s<>"]+""")
    }
}
