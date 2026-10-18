package app.syncboard.daemon

import android.graphics.Bitmap
import android.media.ThumbnailUtils
import android.os.StatFs
import android.util.Size
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException

/**
 * The phone side of the PC's file manager (protocol §8): file operations on shared
 * storage, run as shell, which can read and write all of /storage/emulated/0. Shell goes
 * through the FUSE view there, so MediaStore (gallery apps) sees every change by itself.
 * Each op takes the request header and returns the reply header plus an optional body.
 */
object FileOps {
    const val HOME = "/storage/emulated/0"
    private const val PAGE = 3000
    private const val WALK_MAX = 50_000

    class OpError(msg: String) : IOException(msg)

    class Reply(val header: JSONObject, val body: ByteArray = ByteArray(0))

    /** Only shared storage: nothing under /data or the system partitions. */
    fun checked(path: String): File {
        val f = File(path).canonicalFile
        val p = f.path
        if (!(p == "/storage" || p.startsWith("/storage/") || p == "/sdcard" || p.startsWith("/sdcard/"))) {
            throw OpError("不允许访问这个位置")
        }
        return f
    }

    fun run(req: JSONObject): Reply = when (val op = req.optString("op")) {
        "roots" -> roots()
        "list" -> list(checked(req.getString("path")), req.optInt("offset"))
        "walk" -> walk(checked(req.getString("path")), req.optInt("offset"))
        "mkdir" -> mkdir(checked(req.getString("path")))
        "rename" -> rename(checked(req.getString("path")), checked(req.getString("to")))
        "delete" -> delete(req.getJSONArray("paths"))
        "copy", "move" -> copyOrMove(req.getJSONArray("paths"), checked(req.getString("dir")), op == "move")
        "thumb" -> thumb(checked(req.getString("path")), req.optInt("px", 96))
        else -> throw OpError("手机端不支持 $op，请更新手机 App")
    }

    private fun ok() = JSONObject().put("ok", true)

    private fun roots(): Reply {
        val vols = JSONArray()
        fun add(name: String, dir: File) {
            val st = runCatching { StatFs(dir.path) }.getOrNull()
            vols.put(JSONObject().put("name", name).put("path", dir.path)
                .put("total", st?.totalBytes ?: 0).put("free", st?.availableBytes ?: 0))
        }
        add("内部存储", File(HOME))
        // SD cards and USB drives show up as /storage/XXXX-XXXX.
        File("/storage").listFiles()?.filter { it.name.matches(Regex("[0-9A-F]{4}-[0-9A-F]{4}")) && it.canRead() }
            ?.forEach { add("存储卡 ${it.name}", it) }
        return Reply(ok().put("home", HOME).put("volumes", vols))
    }

    private fun entry(f: File) = JSONObject().put("n", f.name).put("d", f.isDirectory)
        .put("s", if (f.isDirectory) (f.list()?.size ?: 0).toLong() else f.length()).put("m", f.lastModified())

    /** One page of a directory listing, as a JSON array body; `next` is set while more remain. */
    private fun list(dir: File, offset: Int): Reply {
        if (!dir.isDirectory) throw OpError(if (dir.exists()) "不是文件夹" else "文件夹不存在")
        val names = dir.list() ?: throw OpError("无法读取这个文件夹")
        names.sort()
        val page = names.drop(offset).take(PAGE)
        val arr = JSONArray()
        page.forEach { arr.put(entry(File(dir, it))) }
        val h = ok().put("path", dir.path)
        if (offset + page.size < names.size) h.put("next", offset + page.size)
        return Reply(h, arr.toString().toByteArray())
    }

    @Volatile private var walkCache: Pair<String, List<JSONObject>>? = null

    /** Every file and folder under [dir] (relative paths), paged like [list]. */
    private fun walk(dir: File, offset: Int): Reply {
        if (!dir.isDirectory) throw OpError("文件夹不存在")
        val all = walkCache?.takeIf { offset > 0 && it.first == dir.path }?.second ?: buildList {
            fun go(d: File, rel: String) {
                val kids = d.listFiles() ?: return
                kids.sortBy { it.name }
                for (k in kids) {
                    if (size >= WALK_MAX) throw OpError("文件夹里的文件太多（超过 $WALK_MAX 个）")
                    val r = if (rel.isEmpty()) k.name else "$rel/${k.name}"
                    add(JSONObject().put("p", r).put("d", k.isDirectory).put("s", if (k.isDirectory) 0 else k.length()))
                    if (k.isDirectory) go(k, r)
                }
            }
            go(dir, "")
        }.also { walkCache = dir.path to it }
        val page = all.drop(offset).take(PAGE)
        val h = ok().put("path", dir.path)
        if (offset + page.size < all.size) h.put("next", offset + page.size)
        return Reply(h, JSONArray(page).toString().toByteArray())
    }

    private fun mkdir(dir: File): Reply {
        if (dir.exists()) throw OpError("已存在同名文件或文件夹")
        if (!dir.mkdirs()) throw OpError("无法新建文件夹")
        return Reply(ok().put("path", dir.path))
    }

    private fun rename(from: File, to: File): Reply {
        if (!from.exists()) throw OpError("文件不存在")
        if (to.exists()) throw OpError("已存在同名文件或文件夹")
        if (!from.renameTo(to)) throw OpError("无法重命名")
        return Reply(ok().put("path", to.path))
    }

    private fun delete(paths: JSONArray): Reply {
        val files = List(paths.length()) { checked(paths.getString(it)) }
        if (files.any { it.path == HOME || it.parent == "/storage" }) throw OpError("不能删除存储根目录")
        val failed = files.filterNot { it.deleteRecursively() }
        if (failed.isNotEmpty()) throw OpError("有 ${failed.size} 项无法删除")
        return Reply(ok())
    }

    private fun copyOrMove(paths: JSONArray, dir: File, move: Boolean): Reply {
        if (!dir.isDirectory) throw OpError("目标文件夹不存在")
        for (i in 0 until paths.length()) {
            val src = checked(paths.getString(i))
            if (!src.exists()) continue
            if (dir.path == src.path || dir.path.startsWith(src.path + "/")) throw OpError("不能放进它自己里面")
            if (move && src.parentFile?.path == dir.path) continue
            val dst = Engine.uniqueFile(dir, src.name)
            if (move && src.renameTo(dst)) {
                continue
            }
            if (!src.copyRecursively(dst)) throw OpError("复制 ${src.name} 失败")
            if (move) src.deleteRecursively()
        }
        return Reply(ok())
    }

    /** A small JPEG preview of an image or video, or an error for other files. */
    private fun thumb(f: File, px: Int): Reply {
        val size = Size(px.coerceIn(32, 512), px.coerceIn(32, 512))
        val ext = f.extension.lowercase()
        val bmp: Bitmap = try {
            when (ext) {
                in VIDEO -> ThumbnailUtils.createVideoThumbnail(f, size, null)
                else -> ThumbnailUtils.createImageThumbnail(f, size, null)
            }
        } catch (e: Exception) {
            throw OpError("无法预览")
        } catch (e: OutOfMemoryError) {
            throw OpError("无法预览")
        }
        val out = ByteArrayOutputStream()
        bmp.compress(Bitmap.CompressFormat.JPEG, 80, out)
        bmp.recycle()
        return Reply(ok().put("mime", "image/jpeg"), out.toByteArray())
    }

    private val VIDEO = setOf("mp4", "mkv", "mov", "3gp", "webm", "avi", "m4v")
}
