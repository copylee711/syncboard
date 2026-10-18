package app.syncboard.ui

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.provider.Settings
import android.widget.Toast

/**
 * Developer-option state, and shortcuts into Settings that scroll to (and highlight) the
 * switch the user needs, like Shizuku does. Settings honours `:settings:fragment_args_key`
 * on AOSP and most vendor ROMs; elsewhere the page just opens at the top.
 */
object DevOptions {
    /** Where the user is on the way to developer options. */
    enum class DevState {
        /** Hidden: the build number still has to be tapped 7 times. */
        Hidden,
        /** Unhidden but switched off. vivo shows the entry first; its top switch turns it on. */
        Off,
        On,
    }

    private const val ADB_WIFI = "adb_wifi_enabled"
    /** vivo/OriginOS: the developer options entry is visible (set by the build-number taps). */
    private const val VIVO_DEV_SHOWN = "vivo_development_show"

    private fun global(ctx: Context, key: String) = Settings.Global.getInt(ctx.contentResolver, key, 0) == 1

    fun developerOptionsOn(ctx: Context) = global(ctx, Settings.Global.DEVELOPMENT_SETTINGS_ENABLED)

    fun devState(ctx: Context) = when {
        developerOptionsOn(ctx) -> DevState.On
        global(ctx, VIVO_DEV_SHOWN) -> DevState.Off
        else -> DevState.Hidden
    }

    fun usbDebuggingOn(ctx: Context) = global(ctx, Settings.Global.ADB_ENABLED)

    fun wirelessDebuggingOn(ctx: Context) = global(ctx, ADB_WIFI)

    /** Setup over adb granted WRITE_SECURE_SETTINGS, so switches can be flipped directly. */
    fun canWriteSettings(ctx: Context) =
        ctx.checkSelfPermission("android.permission.WRITE_SECURE_SETTINGS") == android.content.pm.PackageManager.PERMISSION_GRANTED

    private fun put(ctx: Context, key: String) =
        canWriteSettings(ctx) && runCatching { Settings.Global.putInt(ctx.contentResolver, key, 1) }.getOrDefault(false)

    /** Turns developer options on without a trip to Settings, when allowed. */
    fun enableDeveloperOptions(ctx: Context): Boolean {
        if (!put(ctx, Settings.Global.DEVELOPMENT_SETTINGS_ENABLED)) return false
        runCatching { Settings.Global.putInt(ctx.contentResolver, VIVO_DEV_SHOWN, 1) }
        return developerOptionsOn(ctx)
    }

    /** Turns wireless debugging on directly, when allowed. */
    fun enableWirelessDebugging(ctx: Context) = put(ctx, ADB_WIFI) && wirelessDebuggingOn(ctx)

    private fun highlight(action: String, key: String) = Intent(action)
        .putExtra(":settings:fragment_args_key", key)
        .putExtra(":settings:show_fragment_args", Bundle().apply { putString(":settings:fragment_args_key", key) })
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    private fun Context.tryStart(vararg intents: Intent): Boolean =
        intents.any { runCatching { startActivity(it) }.isSuccess }

    /**
     * "About phone", for the build number that unlocks developer options. vivo/OriginOS
     * keeps it one level down on 版本信息, a page other apps can't open directly
     * (VivoSubSettings closes itself when another app names the fragment), so the hint
     * says where to tap.
     */
    fun openBuildNumber(ctx: Context) {
        ctx.tryStart(
            highlight(Settings.ACTION_DEVICE_INFO_SETTINGS, "build_number"),
            Intent(Settings.ACTION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        if (android.os.Build.MANUFACTURER.equals("vivo", ignoreCase = true)) {
            guide(ctx, "解锁开发者选项", "滑到底点「版本信息」，连续点击「软件版本号」7 次",
                "在「关于手机」滑到底，点「版本信息」，再连续点击「软件版本号」7 次。看到「已进入开发者模式」后返回 SyncBoard，再点「去打开」。")
        } else {
            guide(ctx, "解锁开发者选项", "连续点击「版本号」7 次，然后返回 SyncBoard",
                "连续点击「版本号」7 次，看到「您已处于开发者模式」后返回 SyncBoard。找不到的话，版本号可能在「关于手机」的「版本信息」或「软件信息」里。")
        }
    }

    /** Developer options, for its top on/off switch (vivo shows the page but leaves it off). */
    fun openDeveloperOptionsSwitch(ctx: Context) {
        ctx.tryStart(Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        guide(ctx, "打开开发者选项", "打开最上面的「开发者选项」开关，然后返回 SyncBoard",
            "打开页面最上面的「开发者选项」开关，然后返回 SyncBoard 继续下一步。")
    }

    /** Developer options, scrolled to the wireless debugging switch. */
    fun openWirelessDebuggingSwitch(ctx: Context) {
        ctx.tryStart(highlight(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS, "toggle_adb_wireless"))
        guide(ctx, "打开无线调试", "打开「无线调试」，然后返回 SyncBoard", "在「调试」分组里打开「无线调试」，在弹窗中点「允许」，然后返回 SyncBoard。")
    }

    /** Developer options, scrolled to the USB debugging switch. */
    fun openUsbDebuggingSwitch(ctx: Context) {
        ctx.tryStart(highlight(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS, "enable_adb"))
        guide(ctx, "打开 USB 调试", "打开「USB 调试」，然后插上数据线", "在「调试」分组里打开「USB 调试」，插上数据线，在手机弹窗中点「允许」。")
    }

    private const val GUIDE_CHANNEL = "guide"
    private const val GUIDE_ID = 7

    /**
     * Shows what to do on the Settings page we just opened: a toast right away, and the
     * same steps as a notification that stays in the shade (a heads-up banner where the
     * ROM allows it; OriginOS doesn't by default). Settings hides app overlays, so a
     * floating card isn't an option.
     */
    private fun guide(ctx: Context, title: String, short: String, text: String) {
        // Toasts get cut at two lines on some ROMs; the notification has the full steps.
        Toast.makeText(ctx, short, Toast.LENGTH_LONG).show()
        val nm = ctx.getSystemService(NotificationManager::class.java)
        if (!nm.areNotificationsEnabled()) return
        nm.createNotificationChannel(NotificationChannel(GUIDE_CHANNEL, "设置引导", NotificationManager.IMPORTANCE_HIGH).apply {
            description = "跳转到系统设置时提示下一步怎么做"
            setSound(null, null)
        })
        val back = PendingIntent.getActivity(ctx, 0, Intent(ctx, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP), PendingIntent.FLAG_IMMUTABLE)
        nm.notify(GUIDE_ID, Notification.Builder(ctx, GUIDE_CHANNEL)
            .setSmallIcon(app.syncboard.R.drawable.ic_tile)
            .setContentTitle(title)
            .setContentText(text)
            .setStyle(Notification.BigTextStyle().bigText(text))
            .setContentIntent(back)
            .setAutoCancel(true)
            .setTimeoutAfter(60_000)
            .build())
    }

    /** Clears the banner once the user is back in the app. */
    fun dismissGuide(ctx: Context) {
        ctx.getSystemService(NotificationManager::class.java).cancel(GUIDE_ID)
    }

    /** Whether to ask for the notification permission before showing the first banner. */
    fun needsNotificationPermission(ctx: Context) = android.os.Build.VERSION.SDK_INT >= 33 &&
        ctx.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED

    /**
     * The wireless debugging page itself (where "pair with QR code" is). Settings exposes
     * it as the preferences screen of its Quick Settings tile; falls back to the switch.
     */
    fun openWirelessDebuggingPage(ctx: Context) {
        val tile = Intent("android.service.quicksettings.action.QS_TILE_PREFERENCES")
            .putExtra(Intent.EXTRA_COMPONENT_NAME, ComponentName(
                "com.android.settings", "com.android.settings.development.qstile.DevelopmentTiles\$WirelessDebugging"))
            .setPackage("com.android.settings")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (ctx.tryStart(tile)) {
            guide(ctx, "扫码配对电脑", "点「使用二维码配对设备」，扫电脑上的码", "在电脑 SyncBoard 点「添加手机」→「无线」，然后在这里点「使用二维码配对设备」扫描电脑上的二维码。")
        } else {
            openWirelessDebuggingSwitch(ctx)
        }
    }
}
