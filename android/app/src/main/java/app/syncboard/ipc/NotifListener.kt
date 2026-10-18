package app.syncboard.ipc

import android.app.Notification
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import org.json.JSONObject
import java.io.BufferedOutputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.Executors

/**
 * 手机通知同步: the system hands new notifications to this service (the phone service
 * grants it notification access when the setting is on) and it passes their text to the
 * daemon, which decides what goes to the PC. Event-driven: nothing runs between
 * notifications. The daemon can't receive notifications itself: Android only delivers them
 * to an app's listener service.
 */
class NotifListener : NotificationListenerService() {
    private val worker = Executors.newSingleThreadExecutor()
    /** Last text per notification key: updates that change nothing (progress, re-posts) are skipped. */
    private val seen = object : LinkedHashMap<String, String>(64, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, String>?) = size > 200
    }

    override fun onListenerConnected() = send(JSONObject().put("cmd", "listener").put("bound", true))

    override fun onListenerDisconnected() = send(JSONObject().put("cmd", "listener").put("bound", false))

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        val n = sbn.notification ?: return
        if (sbn.packageName == packageName) return
        if (n.flags and (Notification.FLAG_ONGOING_EVENT or Notification.FLAG_FOREGROUND_SERVICE or Notification.FLAG_GROUP_SUMMARY) != 0) return
        val x = n.extras ?: return
        val title = x.getCharSequence(Notification.EXTRA_TITLE)?.toString().orEmpty()
        // Chat apps put the newest message last in EXTRA_MESSAGES; others use (big) text.
        val messages = x.getParcelableArray(Notification.EXTRA_MESSAGES)
        val last = messages?.lastOrNull()?.let { (it as? android.os.Bundle)?.getCharSequence("text")?.toString() }
        val text = (last ?: x.getCharSequence(Notification.EXTRA_BIG_TEXT) ?: x.getCharSequence(Notification.EXTRA_TEXT))?.toString().orEmpty()
        if (title.isBlank() && text.isBlank()) return
        val sig = "$title\n$text"
        synchronized(seen) {
            if (seen[sbn.key] == sig) return
            seen[sbn.key] = sig
        }
        send(JSONObject().put("cmd", "notif").put("pkg", sbn.packageName).put("title", title.take(200))
            .put("text", text.take(2000)).put("ts", sbn.postTime))
    }

    /** One frame to the daemon, then hang up. */
    private fun send(o: JSONObject) {
        worker.execute {
            runCatching {
                Socket().use { s ->
                    s.connect(InetSocketAddress("127.0.0.1", DaemonClient.PORT), 1000)
                    app.syncboard.proto.writePlain(DataOutputStream(BufferedOutputStream(s.getOutputStream())), o)
                }
            }
        }
    }
}
