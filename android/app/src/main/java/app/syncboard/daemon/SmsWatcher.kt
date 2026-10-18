package app.syncboard.daemon

import android.content.ContentResolver
import android.database.ContentObserver
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * 短信验证码同步 / 新短信提醒: watches the SMS inbox (shell holds READ_SMS) and hands each
 * new message (sender, body) to [onMessage], which decides what, if anything, leaves the
 * phone. Woken by the SMS provider's change notifications; nothing runs between messages.
 */
class SmsWatcher(
    private val resolver: ContentResolver,
    private val onMessage: (address: String, body: String) -> Unit,
) {
    private val worker = Executors.newSingleThreadScheduledExecutor()
    private val main = Handler(Looper.getMainLooper())
    private var observer: ContentObserver? = null
    private var poll: java.util.concurrent.ScheduledFuture<*>? = null
    /** Highest inbox id looked at (only touched on [worker]). */
    private var lastId = -1L

    @Synchronized
    fun setEnabled(on: Boolean) {
        observer?.let { Observers.unregister(it) }
        observer = null
        poll?.cancel(false)
        poll = null
        if (!on) return
        worker.execute { lastId = maxId() }
        val obs = object : ContentObserver(main) {
            override fun onChange(selfChange: Boolean) {
                // The SMS app writes the row, then updates it; one look covers both.
                worker.schedule({ scan() }, 400, TimeUnit.MILLISECONDS)
            }
        }
        val observed = Observers.register(SMS, true, obs)
        if (observed) observer = obs
        // A cheap safety net (the newest id only) in case a ROM doesn't notify; frequent
        // without the observer, since a code is wanted within seconds.
        poll = worker.scheduleWithFixedDelay({ if (maxId() > lastId) scan() }, 3, if (observed) 15 else 3, TimeUnit.SECONDS)
        DaemonLog.i("watching SMS (observer ${if (observed) "on" else "off"})")
    }

    private fun maxId(): Long = runCatching {
        resolver.query(INBOX, arrayOf("_id"), null, null, "_id DESC LIMIT 1")?.use { c -> if (c.moveToFirst()) c.getLong(0) else 0L } ?: 0L
    }.onFailure { DaemonLog.w("reading SMS failed", it) }.getOrDefault(0L)

    private fun scan() {
        if (lastId < 0) return
        val since = System.currentTimeMillis() - 10 * 60_000
        runCatching {
            resolver.query(INBOX, arrayOf("_id", "body", "date", "address"), "_id>?", arrayOf(lastId.toString()), "_id ASC")?.use { c ->
                while (c.moveToNext()) {
                    lastId = maxOf(lastId, c.getLong(0))
                    // Only fresh messages: not old ones restored from a backup.
                    if (c.getLong(2) < since) continue
                    runCatching { onMessage(c.getString(3).orEmpty(), c.getString(1) ?: continue) }
                        .onFailure { Log.w(TAG, "handling an SMS failed", it) }
                }
            }
        }.onFailure { DaemonLog.w("reading SMS failed", it) }
    }

    private companion object {
        const val TAG = "SyncBoard"
        val SMS: Uri = Uri.parse("content://sms")
        val INBOX: Uri = Uri.parse("content://sms/inbox")
    }
}
