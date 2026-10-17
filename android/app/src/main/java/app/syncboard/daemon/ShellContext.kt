package app.syncboard.daemon

import android.annotation.SuppressLint
import android.app.Application
import android.app.Instrumentation
import android.content.AttributionSource
import android.content.ContentResolver
import android.content.Context
import android.content.ContextWrapper
import android.content.IContentProvider
import android.content.pm.ApplicationInfo
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.os.Process
import android.util.Log

/**
 * A Context usable from an app_process running as shell (uid 2000), so that system
 * services attribute our calls to `com.android.shell`.
 * Adapted from scrcpy's FakeContext/Workarounds (Apache-2.0).
 */
@SuppressLint("PrivateApi", "DiscouragedPrivateApi", "SoonBlockedPrivateApi", "NewApi")
class ShellContext private constructor(base: Context) : ContextWrapper(base) {

    override fun getPackageName() = PACKAGE
    override fun getOpPackageName() = PACKAGE

    override fun getAttributionSource(): AttributionSource =
        AttributionSource.Builder(Process.SHELL_UID).setPackageName(PACKAGE).build()

    override fun getDeviceId() = 0

    override fun getApplicationContext(): Context = this

    override fun createPackageContext(packageName: String?, flags: Int): Context = this

    /**
     * The system context's resolver acquires providers as package "android" through an
     * app thread AMS doesn't know, which fails for us. Some ROMs (OriginOS) read Settings
     * inside ClipboardManager.getPrimaryClip, so route provider lookups through
     * getContentProviderExternal, which is meant for shell callers.
     */
    private val resolver = ExternalResolver(this)

    override fun getContentResolver(): ContentResolver = resolver

    private class ExternalResolver(ctx: Context) : ContentResolver(ctx) {
        // Hidden in the SDK; at runtime these override ContentResolver's abstract methods.
        @Suppress("unused")
        protected fun acquireProvider(c: Context?, name: String): IContentProvider? = externalProvider(name)

        @Suppress("unused")
        fun releaseProvider(p: IContentProvider?): Boolean = false

        @Suppress("unused")
        protected fun acquireUnstableProvider(c: Context?, name: String): IContentProvider? = externalProvider(name)

        @Suppress("unused")
        fun releaseUnstableProvider(p: IContentProvider?): Boolean = false

        @Suppress("unused")
        fun unstableProviderDied(p: IContentProvider?) {}
    }

    override fun getSystemService(name: String): Any? {
        val service = super.getSystemService(name) ?: return null
        if (name == CLIPBOARD_SERVICE || name == "semclipboard" || name == ACTIVITY_SERVICE) {
            runCatching {
                val f = service.javaClass.getDeclaredField("mContext")
                f.isAccessible = true
                f.set(service, this)
            }
        }
        return service
    }

    companion object {
        const val PACKAGE = "com.android.shell"

        @Volatile private var instance: ShellContext? = null

        /** Must be called on the thread that called Looper.prepareMainLooper(). */
        fun init(): ShellContext {
            instance?.let { return it }
            val at = Class.forName("android.app.ActivityThread")
            val thread = at.getDeclaredConstructor().apply { isAccessible = true }.newInstance()
            at.getDeclaredField("sCurrentActivityThread").apply { isAccessible = true }.set(null, thread)
            at.getDeclaredField("mSystemThread").apply { isAccessible = true }.setBoolean(thread, true)
            runCatching {
                val abd = Class.forName("android.app.ActivityThread\$AppBindData")
                val bind = abd.getDeclaredConstructor().apply { isAccessible = true }.newInstance()
                val ai = ApplicationInfo().apply { packageName = PACKAGE }
                abd.getDeclaredField("appInfo").apply { isAccessible = true }.set(bind, ai)
                at.getDeclaredField("mBoundApplication").apply { isAccessible = true }.set(thread, bind)
            }
            val sys = at.getDeclaredMethod("getSystemContext").invoke(thread) as Context
            val ctx = ShellContext(sys)
            runCatching {
                val app = Instrumentation.newApplication(Application::class.java, ctx)
                at.getDeclaredField("mInitialApplication").apply { isAccessible = true }.set(thread, app)
            }
            instance = ctx
            return ctx
        }

        /** ActivityManager.getService().getContentProviderExternal(name, user 0, token, tag).provider */
        fun externalProvider(name: String): IContentProvider? = runCatching {
            val am = Class.forName("android.app.ActivityManager").getMethod("getService").invoke(null)!!
            val get = am.javaClass.getMethod(
                "getContentProviderExternal",
                String::class.java, Int::class.javaPrimitiveType, IBinder::class.java, String::class.java,
            )
            val holder = get.invoke(am, name, 0, Binder(), "syncboard") ?: return null
            holder.javaClass.getDeclaredField("provider").apply { isAccessible = true }.get(holder) as IContentProvider?
        }.onFailure { Log.w("SyncBoard", "external provider $name: $it") }.getOrNull()

        val isShell: Boolean get() = Process.myUid() == Process.SHELL_UID || Process.myUid() == 0
        val sdk: Int get() = Build.VERSION.SDK_INT
    }
}
