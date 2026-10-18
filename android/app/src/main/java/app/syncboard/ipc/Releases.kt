package app.syncboard.ipc

import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/** SyncBoard releases on GitHub; used by the app (checking) and the daemon (installing). */
object Releases {
    private const val REPO = "https://github.com/copylee711/syncboard"

    fun versionTuple(v: String) = v.split(Regex("[^0-9]+")).filter { it.isNotEmpty() }.map { it.toInt() }

    fun isNewer(latest: String, current: String): Boolean {
        val a = versionTuple(latest)
        val b = versionTuple(current)
        for (i in 0 until maxOf(a.size, b.size)) {
            val d = a.getOrElse(i) { 0 } - b.getOrElse(i) { 0 }
            if (d != 0) return d > 0
        }
        return false
    }

    /**
     * The latest release's version, read from where `/releases/latest` redirects (unlike
     * the REST API, this has no rate limit). Blocking.
     */
    fun latestVersion(): String {
        val c = URL("$REPO/releases/latest").openConnection() as HttpURLConnection
        c.instanceFollowRedirects = false
        c.connectTimeout = 15_000
        c.readTimeout = 15_000
        try {
            val location = c.getHeaderField("Location") ?: error("GitHub 没有返回版本信息（${c.responseCode}）")
            return location.substringAfterLast("/tag/v", "").takeIf { it.matches(Regex("[0-9.]+")) }
                ?: error("没有找到发布版本")
        } finally {
            c.disconnect()
        }
    }

    fun apkName(version: String) = "SyncBoard-$version.apk"

    fun apkUrl(version: String) = "$REPO/releases/download/v$version/${apkName(version)}"

    fun pageUrl(version: String) = "$REPO/releases/tag/v$version"

    /**
     * Downloads the APK for [version] into [into] and checks it against the release's
     * SHA256SUMS.txt. Blocking; throws with a message to show the user.
     */
    fun downloadApk(version: String, into: File) {
        require(version.matches(Regex("[0-9.]+"))) { "bad version" }
        val sums = open("$REPO/releases/download/v$version/SHA256SUMS.txt").use { it.readBytes().decodeToString() }
        val want = sums.lineSequence().map { it.trim().split(Regex("[ \t]+")) }
            .firstOrNull { it.size == 2 && it[1] == apkName(version) }?.get(0)?.lowercase()
            ?: error("发布中没有手机端安装包")
        val md = MessageDigest.getInstance("SHA-256")
        open(apkUrl(version)).use { input ->
            into.outputStream().use { out ->
                val buf = ByteArray(256 * 1024)
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    md.update(buf, 0, n)
                    out.write(buf, 0, n)
                }
            }
        }
        if (md.digest().joinToString("") { "%02x".format(it) } != want) {
            into.delete()
            error("下载的安装包校验失败，请重试")
        }
    }

    private fun open(url: String) = (URL(url).openConnection() as HttpURLConnection).apply {
        connectTimeout = 15_000
        readTimeout = 30_000
    }.inputStream
}
