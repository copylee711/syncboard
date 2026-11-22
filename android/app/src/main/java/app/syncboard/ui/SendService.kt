package app.syncboard.ui

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.util.Log
import android.widget.Toast
import app.syncboard.ipc.DaemonClient
import java.util.concurrent.LinkedBlockingQueue

/**
 * Sends files shared with 发送到电脑 in the background, with the progress in a notification
 * instead of a window (OriginOS opened that window in a small-window frame). A foreground
 * service keeps the process alive once the share activity has finished. The files are
 * opened by [ShareActivity] while it still holds the URI grants.
 */
class SendService : Service() {
    class Item(val name: String, val size: Long, val fd: ParcelFileDescriptor)

    private val main = Handler(Looper.getMainLooper())
    @Volatile private var worker: Thread? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "发送到电脑", NotificationManager.IMPORTANCE_LOW))
        startForeground(NOTE_ID, note("正在发送到电脑…", null, -1), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        if (worker?.isAlive != true) worker = Thread({ run() }, "send-to-pc").apply { start() }
        return START_NOT_STICKY
    }

    private fun note(title: String, text: String?, permille: Int): Notification =
        Notification.Builder(this, CHANNEL)
            .setSmallIcon(app.syncboard.R.drawable.ic_tile)
            .setContentTitle(title)
            .apply { text?.let { setContentText(it) } }
            .setProgress(1000, maxOf(permille, 0), permille < 0)
            .setOnlyAlertOnce(true)
            .setOngoing(true)
            .build()

    private fun run() {
        val nm = getSystemService(NotificationManager::class.java)
        var ok = 0
        var count = 0
        while (true) {
            val batch = generateSequence { queue.poll() }.toList()
            if (batch.isEmpty()) break
            val total = batch.sumOf { it.size }
            var before = 0L
            var shown = 0L
            batch.forEachIndexed { i, it ->
                count++
                val title = if (batch.size > 1) "发送到电脑 ${i + 1}/${batch.size}" else "发送到电脑"
                nm.notify(NOTE_ID, note(title, it.name, if (total > 0) (before * 1000 / total).toInt() else -1))
                val sent = runCatching {
                    ParcelFileDescriptor.AutoCloseInputStream(it.fd).use { input ->
                        DaemonClient.sendFile(applicationContext, it.name, it.size, input) { n ->
                            val now = SystemClock.uptimeMillis()
                            if (now - shown >= 500 && total > 0) {
                                shown = now
                                nm.notify(NOTE_ID, note(title, "${it.name} · ${mb(before + n)} / ${mb(total)}",
                                    ((before + n) * 1000 / total).toInt()))
                            }
                        }
                    }
                }.onFailure { e -> Log.w("SyncBoard", "send failed", e) }.getOrDefault(false)
                before += it.size
                if (sent) ok++
            }
        }
        main.post {
            // More was shared while the last file went out: carry on.
            if (queue.isNotEmpty()) {
                worker = Thread({ run() }, "send-to-pc").apply { start() }
                return@post
            }
            Toast.makeText(applicationContext,
                if (ok == count) "已发送 $ok 个文件到电脑" else "发送完成 $ok/$count，请确认电脑在线", Toast.LENGTH_SHORT).show()
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
    }

    private fun mb(b: Long) = if (b < 1024 * 1024) "${b / 1024} KB" else "%.1f MB".format(b / 1048576.0)

    companion object {
        private const val CHANNEL = "send"
        private const val NOTE_ID = 9
        private val queue = LinkedBlockingQueue<Item>()

        fun start(ctx: Context, items: List<Item>) {
            queue.addAll(items)
            ctx.startForegroundService(Intent(ctx, SendService::class.java))
        }
    }
}
