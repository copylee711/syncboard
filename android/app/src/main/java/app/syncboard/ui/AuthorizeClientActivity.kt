package app.syncboard.ui

import android.app.Activity
import android.app.AlertDialog
import android.os.Bundle
import android.widget.Toast
import app.syncboard.ipc.DaemonClient
import app.syncboard.proto.readPlain
import app.syncboard.proto.writePlain
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import kotlin.concurrent.thread

/**
 * Another app (TwinFile) asks to use the phone service: browse the connected PCs' files
 * through it and reach Android/data with its rights. Started with startActivityForResult,
 * so the caller's package is known for certain; the user decides here, and the daemon then
 * accepts connections from that app's uid. Revoked in 设置 → 已授权的应用.
 */
class AuthorizeClientActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val pkg = callingPackage
        if (pkg == null || pkg == packageName) {
            setResult(RESULT_CANCELED)
            return finish()
        }
        val label = runCatching { packageManager.getApplicationInfo(pkg, 0).loadLabel(packageManager).toString() }.getOrDefault(pkg)
        AlertDialog.Builder(this)
            .setTitle("允许「$label」使用 SyncBoard？")
            .setMessage(
                "它将可以：\n" +
                    "· 通过 SyncBoard 已连接的电脑浏览、管理电脑上的文件（电脑需开启「允许已配对设备浏览文件」）\n" +
                    "· 以 SyncBoard 手机服务的权限访问 Android/data 等受保护的文件夹\n\n" +
                    "可以随时在 SyncBoard 设置 → 已授权的应用 里撤销。"
            )
            .setPositiveButton("允许") { _, _ -> approve(pkg) }
            .setNegativeButton("拒绝") { _, _ -> setResult(RESULT_CANCELED); finish() }
            .setOnCancelListener { setResult(RESULT_CANCELED); finish() }
            .show()
    }

    private fun approve(pkg: String) {
        thread {
            val error = try {
                val uid = packageManager.getPackageUid(pkg, 0)
                Socket().use { s ->
                    s.connect(InetSocketAddress("127.0.0.1", DaemonClient.PORT), 1000)
                    s.soTimeout = 5000
                    writePlain(DataOutputStream(BufferedOutputStream(s.getOutputStream())),
                        JSONObject().put("cmd", "authorizeClient").put("pkg", pkg).put("uid", uid))
                    val r = readPlain(DataInputStream(BufferedInputStream(s.getInputStream())))
                    if (r.optBoolean("ok")) null else "授权失败"
                }
            } catch (e: java.net.ConnectException) {
                "手机服务未运行，请先打开 SyncBoard 启动服务"
            } catch (e: Exception) {
                e.message ?: "授权失败"
            }
            runOnUiThread {
                if (error != null) Toast.makeText(this, error, Toast.LENGTH_LONG).show()
                setResult(if (error == null) RESULT_OK else RESULT_CANCELED)
                finish()
            }
        }
    }
}
