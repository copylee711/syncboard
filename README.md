# SyncBoard

Windows 与 Android 之间的**双向剪贴板同步**（文字 + 图片）和文件互传。手机端不常驻通知栏、不需要 root，在国产 ROM 上也能后台读取剪贴板。

## 为什么又一个剪贴板同步工具

Android 10 起，普通应用无法在后台读取剪贴板，所以 KDE Connect、ClipCascade 等方案通常只能做到“电脑 → 手机”的单向自动同步。

SyncBoard 的手机端核心是一个以 **shell 身份**（uid 2000）运行的守护进程，原理和 scrcpy、Shizuku 相同：

- shell 可以在后台读写剪贴板，因此两个方向都能自动同步；
- 守护进程不属于 App 进程，不受后台限制和厂商杀后台影响，**不需要前台服务通知**；
- App 被划掉、锁屏后照常同步。

守护进程需要电脑通过 adb 启动一次，这一步由电脑端自动完成。

## 功能

- 文字、图片双向自动同步（电脑端使用 `AddClipboardFormatListener`，事件驱动，不轮询）
- 文件互传：拖到电脑窗口、右键「发送到」、手机系统分享「发送到电脑」
- 手机分享菜单「复制到电脑剪贴板」：相册、截图等不走系统剪贴板的图片也能发
- 可选「截屏后自动复制」：新截图自动进入手机和电脑剪贴板
- 端到端加密：配对时用 P-256 ECDH 协商密钥，传输使用 AES-256-GCM
- 电脑端单个 exe，免安装；关闭窗口进入**轻量模式**（销毁 WebView，常驻内存约 4 MB）
- 手机端：从最近任务隐藏、快捷设置磁贴（暂停/恢复）、深浅色切换
- 尊重密码管理器标记的敏感内容，不同步

## 使用

1. 从 [Releases](https://github.com/copylee711/syncboard/releases) 下载 `SyncBoard-<版本>-windows-x64.exe`，双击运行。
2. 点「添加手机」，任选一种方式：
   - **数据线**：手机打开「开发者选项 → USB 调试」，插上线，在手机弹窗中点「允许」。
   - **无线**（Android 11+）：手机打开「开发者选项 → 无线调试 → 使用二维码配对设备」，扫描电脑上的二维码。两端不在同一网段时，可改用电脑热点。
   - **已安装 App**：手机 App 里点「扫码配对」。
3. 其余步骤自动完成：电脑端会安装手机 App（从本仓库 Release 下载）、启动后台服务并完成配对。

> 没有开发者选项？在「关于手机」里连续点击「版本号」7 次。vivo/iQOO：设置 → 系统管理 → 关于手机 → 软件版本号。

### 手机重启后

没有 root 时，后台服务会随重启停止，可以这样恢复：

- 插上数据线：电脑端会自动重新启动服务；
- 或在手机 App 里点「一键启动」：App 临时打开无线调试，请电脑重新启动服务，完成后关闭无线调试；
- 开启「重启后自动恢复服务」后，开机后会自动执行上面这一步（需要手机连接 Wi-Fi、电脑端在运行）。

### 关闭手机服务

手机 App「设置 → 停止手机服务」，或电脑端设备列表中的停止按钮。卸载 App 后，服务会在 30 秒内自行退出。

## 已知限制

- 部分 ROM 的相册「复制图片」和 Gboard 的截图建议不会写入系统剪贴板，所以无法自动同步。请用分享菜单「复制到电脑剪贴板」，或开启「截屏后自动复制」。
- 手机端守护进程占用约 60 MB（PSS），与 scrcpy 服务端属于同一量级。
- OriginOS 等系统首次通过 adb 安装 App 时，需要在手机上确认。
- 电脑端 exe 没有代码签名，首次运行可能出现 SmartScreen 提示。

## 构建

| 组件 | 目录 | 工具 |
|---|---|---|
| 电脑端 | `desktop/` | Rust 1.9x（MSVC），Tauri 2；`cargo build --release -p syncboard` |
| 手机端 | `android/` | JDK 17+，Android SDK 37；`./gradlew assembleRelease` |

- 电脑端构建时会从 Android SDK 的 `platform-tools` 打包 adb。若 `android/app/build/outputs/apk/release/` 下已有 APK，也会一并打包，便于离线测试；正式发布时设置 `SYNCBOARD_NO_APK=1`，改为运行时从 Release 下载。
- 手机端 release 签名读取 `~/.syncboard/keystore.properties`（或环境变量 `SYNCBOARD_SIGNING` 指定的文件），找不到时使用调试签名。

协议见 [docs/protocol.md](docs/protocol.md)。

## 第三方组件

- `android/.../daemon/ShellContext.kt` 改编自 [scrcpy](https://github.com/Genymobile/scrcpy)（Apache-2.0）。
- 电脑端打包了 Android SDK Platform-Tools 中的 adb（Apache-2.0，NOTICE 随文件一起解压）。
- [Tauri](https://tauri.app)、[ZXing](https://github.com/zxing/zxing)、CameraX、Jetpack Compose 等，均为各自许可证。

## 许可证

[MIT](LICENSE)
