package app.syncboard.debug

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.os.Bundle
import androidx.core.content.FileProvider
import java.io.File

class CopyImageActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val bmp = Bitmap.createBitmap(160, 100, Bitmap.Config.ARGB_8888)
        Canvas(bmp).apply {
            drawColor(Color.rgb(30, 120, 80))
            drawCircle(80f, 50f, 35f, Paint().apply { color = Color.YELLOW })
        }
        val f = File(cacheDir, "test.png")
        f.outputStream().use { bmp.compress(Bitmap.CompressFormat.PNG, 100, it) }
        val uri = FileProvider.getUriForFile(this, "app.syncboard.debugfiles", f)
        val cm = getSystemService(ClipboardManager::class.java)
        cm.setPrimaryClip(ClipData.newUri(contentResolver, "test image", uri))
        finish()
    }
}
