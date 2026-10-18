package app.syncboard.daemon

import android.database.ContentObserver
import android.net.Uri
import android.os.Build
import android.os.IBinder
import android.util.Log

/**
 * Registers a [ContentObserver] straight with the system's content service, and says
 * whether that worked. Through our shell Context, ContentResolver.registerContentObserver
 * can return normally without registering anything. Android 16 refuses observers from
 * processes ActivityManager doesn't know ("Failed to find PID"), which includes this
 * app_process daemon, so callers keep a polling fallback.
 */
object Observers {
    private val service: Any? by lazy {
        runCatching {
            val binder = Class.forName("android.os.ServiceManager").getMethod("getService", String::class.java)
                .invoke(null, "content") as IBinder
            Class.forName("android.content.IContentService\$Stub").getMethod("asInterface", IBinder::class.java)
                .invoke(null, binder)
        }.onFailure { Log.w("SyncBoard", "content service unavailable", it) }.getOrNull()
    }

    private fun transport(o: ContentObserver): Any =
        ContentObserver::class.java.getMethod("getContentObserver").invoke(o)!!

    /** True when the service accepted the observer. */
    fun register(uri: Uri, descendants: Boolean, o: ContentObserver): Boolean = runCatching {
        val svc = service ?: error("no content service")
        val m = svc.javaClass.methods.first { it.name == "registerContentObserver" && it.parameterTypes.size == 5 }
        m.invoke(svc, uri, descendants, transport(o), 0, Build.VERSION.SDK_INT)
        true
    }.onFailure {
        val e = (it as? java.lang.reflect.InvocationTargetException)?.targetException ?: it
        Log.w("SyncBoard", "observing $uri failed: ${e.message ?: e}")
    }
        .getOrDefault(false)

    fun unregister(o: ContentObserver) {
        runCatching {
            val svc = service ?: return
            svc.javaClass.methods.first { it.name == "unregisterContentObserver" }.invoke(svc, transport(o))
        }
    }
}
