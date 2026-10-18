//! Getting the phone side running without the user touching a terminal:
//! bundled adb, USB setup, and Android 11+ "pair with QR code" wireless setup.
//! Either way ends in [setup_device]: install the app if needed, start the daemon and
//! pair it with this PC.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PACKAGE: &str = "app.syncboard";

/// Must stay in sync with `Main.LAUNCH_COMMAND` in the Android daemon.
fn launch_command(extra: &str) -> String {
    format!(
        "p=$(pm path {PACKAGE} | head -n1 | cut -d: -f2); \
         [ -n \"$p\" ] && CLASSPATH=$p nohup setsid app_process /system/bin --nice-name=syncboard_daemon \
         app.syncboard.daemon.Main {extra} </dev/null >/dev/null 2>&1 &"
    )
}

// ---------- locating adb ----------

#[cfg(embedded_adb)]
static ADB_PACK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/adb.bin"));

/// Extracts the bundled adb to %LOCALAPPDATA%\SyncBoard\platform-tools (once per build).
#[cfg(embedded_adb)]
fn embedded() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("SyncBoard").join("platform-tools");
    let stamp = format!("{}", ADB_PACK.len());
    let marker = dir.join(".stamp");
    if std::fs::read_to_string(&marker).ok().as_deref() == Some(stamp.as_str()) && dir.join("adb.exe").is_file() {
        return Some(dir.join("adb.exe"));
    }
    let raw = miniz_oxide::inflate::decompress_to_vec(ADB_PACK).ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let mut i = 0;
    while i + 2 <= raw.len() {
        let nl = u16::from_le_bytes([raw[i], raw[i + 1]]) as usize;
        let name = std::str::from_utf8(&raw[i + 2..i + 2 + nl]).ok()?;
        i += 2 + nl;
        let dl = u32::from_le_bytes(raw[i..i + 4].try_into().ok()?) as usize;
        i += 4;
        std::fs::write(dir.join(name), &raw[i..i + dl]).ok()?;
        i += dl;
    }
    std::fs::write(marker, stamp).ok()?;
    Some(dir.join("adb.exe"))
}

#[cfg(not(embedded_adb))]
fn embedded() -> Option<PathBuf> {
    None
}

/// A custom path, then an adb already on the system (to avoid two adb servers of
/// different versions fighting over port 5037), then the bundled one.
pub fn find(custom: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = custom.filter(|p| p.is_file()) {
        return Some(p.to_path_buf());
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("adb.exe")).collect())
        .unwrap_or_default();
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(v) = std::env::var_os(var) {
            candidates.push(PathBuf::from(v).join("platform-tools").join("adb.exe"));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Android").join("Sdk").join("platform-tools").join("adb.exe"));
    }
    candidates.into_iter().find(|p| p.is_file()).or_else(embedded)
}

fn run(adb: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(adb)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("无法运行 adb：{e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.trim().to_string())
    }
}

fn shell(adb: &Path, serial: &str, cmd: &str) -> Result<String, String> {
    run(adb, &["-s", serial, "shell", cmd])
}

// ---------- devices ----------

#[derive(Debug, Clone, PartialEq)]
pub enum DevState {
    Ready,
    Unauthorized,
    Offline,
}

pub fn devices_with_state(adb: &Path) -> Vec<(String, DevState)> {
    run(adb, &["devices"])
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let serial = it.next()?.to_string();
            let st = match it.next()? {
                "device" => DevState::Ready,
                "unauthorized" => DevState::Unauthorized,
                _ => DevState::Offline,
            };
            Some((serial, st))
        })
        .collect()
}

pub fn devices(adb: &Path) -> Vec<String> {
    devices_with_state(adb).into_iter().filter(|(_, s)| *s == DevState::Ready).map(|(d, _)| d).collect()
}

// ---------- device setup ----------

/// Release builds download the phone APK matching this version from GitHub Releases.
/// Local builds may embed it instead (see build.rs) so they can be tested offline.
const RELEASE_BASE: &str = "https://github.com/copylee711/syncboard/releases/download";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(embedded_apk)]
static APK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.apk"));
#[cfg(embedded_apk)]
const APK_VERSION: &str = include_str!(concat!(env!("OUT_DIR"), "/apk_version.txt"));

fn dumpsys_field(adb: &Path, serial: &str, field: &str) -> Option<String> {
    let out = shell(adb, serial, &format!("dumpsys package {PACKAGE} | grep {field}=")).ok()?;
    Some(out.split(&format!("{field}=")).nth(1)?.split_whitespace().next()?.to_string())
}

/// Returns a local APK path to install, or None if the phone is already up to date.
fn apk_to_install(adb: &Path, serial: &str, progress: &dyn Fn(&str)) -> Result<Option<PathBuf>, String> {
    #[cfg(embedded_apk)]
    {
        let _ = progress;
        let bundled: u32 = APK_VERSION.trim().parse().unwrap_or(1);
        let installed = dumpsys_field(adb, serial, "versionCode").and_then(|v| v.parse::<u32>().ok());
        if installed.is_some_and(|v| v >= bundled) {
            return Ok(None);
        }
        let tmp = std::env::temp_dir().join("syncboard-phone.apk");
        std::fs::write(&tmp, APK).map_err(|e| e.to_string())?;
        Ok(Some(tmp))
    }
    #[cfg(not(embedded_apk))]
    {
        let installed = dumpsys_field(adb, serial, "versionName");
        if installed.as_deref().is_some_and(|v| crate::update::version_tuple(v) >= crate::update::version_tuple(VERSION)) {
            return Ok(None);
        }
        let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("no LOCALAPPDATA")?).join("SyncBoard");
        let apk = dir.join(format!("SyncBoard-{VERSION}.apk"));
        if !apk.is_file() {
            progress("正在下载手机端 SyncBoard…");
            let _ = std::fs::create_dir_all(&dir);
            let url = format!("{RELEASE_BASE}/v{VERSION}/SyncBoard-{VERSION}.apk");
            let part = apk.with_extension("part");
            // curl.exe ships with Windows 10 1803+.
            let ok = Command::new("curl.exe")
                .args(["-fL", "--retry", "2", "--connect-timeout", "15", "-o"])
                .arg(&part)
                .arg(&url)
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .is_ok_and(|s| s.success());
            if !ok || std::fs::rename(&part, &apk).is_err() {
                let _ = std::fs::remove_file(&part);
                return Err(if installed.is_some() {
                    "无法下载新版手机端，已跳过更新".into()
                } else {
                    format!("无法下载手机端 App（{url}）。请先在手机上安装 SyncBoard，再重试。")
                });
            }
        }
        Ok(Some(apk))
    }
}

/// Installs or upgrades the phone app when needed.
fn ensure_app(adb: &Path, serial: &str, progress: &dyn Fn(&str)) -> Result<(), String> {
    let apk = match apk_to_install(adb, serial, progress) {
        Ok(Some(apk)) => apk,
        Ok(None) => return Ok(()),
        // An older app that is already installed still works; don't block on updates.
        Err(e) if dumpsys_field(adb, serial, "versionName").is_some() => {
            progress(&e);
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    progress("正在安装手机端 SyncBoard…（如手机弹出确认，请点「允许」）");
    match run(adb, &["-s", serial, "install", "-r", "-g", &apk.to_string_lossy()]) {
        Ok(out) if out.contains("Success") => Ok(()),
        Ok(out) | Err(out) => Err(install_error(&out)),
    }
}

fn install_error(out: &str) -> String {
    if out.contains("INSTALL_FAILED_USER_RESTRICTED") || out.contains("INSTALL_FAILED_ABORTED") {
        "手机拒绝了安装。请在开发者选项里打开「USB 安装」，或在手机弹窗中点「允许」后重试。".into()
    } else if out.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE") {
        "手机上已有签名不同的 SyncBoard，请先卸载后重试。".into()
    } else {
        format!("安装失败：{}", out.lines().last().unwrap_or(out).trim())
    }
}

/// Installs the app if needed, starts the daemon and asks it to pair with `pair_uri`.
/// Over USB, `adb reverse` lets the phone reach this PC even without a shared network.
pub fn setup_device(adb: &Path, serial: &str, pair_uri: &str, usb: bool, progress: &dyn Fn(&str)) -> Result<(), String> {
    ensure_app(adb, serial, progress)?;
    progress("正在启动手机服务…");
    let mut uri = pair_uri.to_string();
    if usb && run(adb, &["-s", serial, "reverse", "tcp:53317", "tcp:53317"]).is_ok() {
        // Loopback through the cable goes last so a shared network is preferred.
        uri = uri.replacen("&p=", ",127.0.0.1&p=", 1);
    }
    // Lets the app re-enable wireless debugging after a reboot (see BootReceiver).
    let _ = shell(adb, serial, &format!("pm grant {PACKAGE} android.permission.WRITE_SECURE_SETTINGS"));
    shell(adb, serial, &launch_command(&format!("--pair '{uri}'")))?;
    Ok(())
}

/// Just (re)starts the daemon on every connected device; used from the tray menu.
pub fn start_all(adb: &Path) -> Result<String, String> {
    let devs = devices(adb);
    if devs.is_empty() {
        return Err("没有检测到手机。请用 USB 连接并允许 USB 调试，或使用无线配对。".into());
    }
    let mut errors = Vec::new();
    for d in &devs {
        if let Err(e) = shell(adb, d, &launch_command("")) {
            errors.push(format!("{d}: {e}"));
        }
    }
    if errors.is_empty() { Ok(format!("已在 {} 台设备上启动手机服务", devs.len())) } else { Err(errors.join("\n")) }
}

pub fn start_daemon(adb: &Path, serial: &str) -> Result<(), String> {
    if dumpsys_field(adb, serial, "versionName").is_none() {
        return Err("手机上还没有安装 SyncBoard".into());
    }
    shell(adb, serial, &launch_command("")).map(|_| ())
}

// ---------- wireless pairing (Android 11+ "Pair device with QR code") ----------

pub struct WifiPairing {
    pub name: String,
    pub password: String,
}

impl WifiPairing {
    pub fn new() -> Self {
        let r = syncboard_core::crypto::random_bytes::<16>();
        const ABC: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
        let pick = |b: &[u8]| b.iter().map(|x| ABC[*x as usize % ABC.len()] as char).collect::<String>();
        WifiPairing { name: format!("syncboard-{}", pick(&r[..6])), password: pick(&r[6..16]) }
    }

    /// The text Android's wireless-debugging QR scanner expects.
    pub fn qr_text(&self) -> String {
        format!("WIFI:T:ADB;S:{};P:{};;", self.name, self.password)
    }
}

/// `adb mdns services` lines: (instance name, service type, ip:port).
fn mdns_services(adb: &Path) -> Vec<(String, String, String)> {
    run(adb, &["mdns", "services"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (n, t, a) = (it.next()?, it.next()?, it.next()?);
            a.contains(':').then(|| (n.to_string(), t.to_string(), a.to_string()))
        })
        .collect()
}

/// Waits for the phone to scan the QR code, pairs, connects, and returns the serial.
pub fn wifi_pair_and_connect(
    adb: &Path,
    p: &WifiPairing,
    cancel: &Arc<AtomicBool>,
    progress: &dyn Fn(&str),
) -> Result<String, String> {
    let _ = run(adb, &["start-server"]);
    let deadline = Instant::now() + Duration::from_secs(300);
    let pair_addr = loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        if Instant::now() > deadline {
            return Err("等待超时：没有发现扫码的手机。请确认手机和电脑在同一网络。".into());
        }
        if let Some((_, _, addr)) =
            mdns_services(adb).into_iter().find(|(n, t, _)| n == &p.name && t.contains("adb-tls-pairing"))
        {
            break addr;
        }
        std::thread::sleep(Duration::from_millis(800));
    };
    progress("已发现手机，正在配对…");
    let out = run(adb, &["pair", &pair_addr, &p.password])?;
    if !out.contains("Successfully paired") {
        return Err(format!("配对失败：{}", out.trim()));
    }
    let ip = pair_addr.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or_default();

    progress("配对成功，正在连接…");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        // adb usually auto-connects to a freshly paired device found over mDNS.
        let ready = devices(adb);
        if let Some(d) = ready.iter().find(|d| d.starts_with(&format!("{ip}:")) || d.contains("adb-tls-connect")) {
            return Ok(d.clone());
        }
        if let Some((_, _, addr)) = mdns_services(adb)
            .into_iter()
            .find(|(_, t, a)| t.contains("adb-tls-connect") && a.starts_with(&format!("{ip}:")))
        {
            if run(adb, &["connect", &addr]).is_ok_and(|o| o.contains("connected")) {
                return Ok(addr);
            }
        }
        if Instant::now() > deadline {
            return Err("配对成功但无法连接手机，请确认「无线调试」仍处于开启状态。".into());
        }
        std::thread::sleep(Duration::from_millis(800));
    }
}

#[cfg(all(test, embedded_adb))]
mod tests {
    use super::*;

    /// On a PC without the Android SDK, `find` must fall back to the bundled adb.
    #[test]
    fn bundled_adb_extracts_and_runs() {
        let tmp = std::env::temp_dir().join(format!("sb-adb-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: single-threaded test; restores nothing because the process exits after.
        unsafe {
            std::env::set_var("LOCALAPPDATA", &tmp);
            std::env::set_var("PATH", "");
            std::env::remove_var("ANDROID_HOME");
            std::env::remove_var("ANDROID_SDK_ROOT");
        }
        let adb = find(None).expect("bundled adb");
        assert!(adb.starts_with(&tmp), "expected the bundled copy, got {}", adb.display());
        let out = run(&adb, &["version"]).expect("adb version");
        assert!(out.contains("Android Debug Bridge"), "{out}");
        // A second call reuses the extracted copy.
        assert_eq!(find(None).unwrap(), adb);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// Connects to a phone that announced its wireless-debugging port (after a reboot).
pub fn connect_wireless(adb: &Path, addr: &str) -> Result<String, String> {
    let out = run(adb, &["connect", addr])?;
    if out.contains("connected") { Ok(addr.to_string()) } else { Err(out.trim().to_string()) }
}
