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

/// Must stay in sync with `Main.LAUNCH_COMMAND` in the Android daemon. The background
/// job must be a plain command: with `[ ... ] && cmd &`, sh backgrounds a subshell that
/// keeps adb's output pipe open while the daemon runs, so `adb shell` never returns.
fn launch_command(extra: &str) -> String {
    format!(
        "p=$(pm path {PACKAGE} | head -n1 | cut -d: -f2); [ -n \"$p\" ] || exit 1; \
         CLASSPATH=$p nohup setsid app_process /system/bin --nice-name=syncboard_daemon \
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

fn dumpsys_field(adb: &Path, serial: &str, field: &str) -> Option<String> {
    let out = shell(adb, serial, &format!("dumpsys package {PACKAGE} | grep {field}=")).ok()?;
    Some(out.split(&format!("{field}=")).nth(1)?.split_whitespace().next()?.to_string())
}

/// The user installs the app themselves (step 1 of 添加手机); adb only starts it.
fn require_app(adb: &Path, serial: &str) -> Result<(), String> {
    match dumpsys_field(adb, serial, "versionName") {
        Some(_) => Ok(()),
        None => Err("手机上还没有安装 SyncBoard。请回到第 1 步扫码安装，再重试。".into()),
    }
}

/// Starts the daemon on a phone that has the app and asks it to pair with `pair_uri`.
/// Over USB, `adb reverse` lets the phone reach this PC even without a shared network.
pub fn setup_device(adb: &Path, serial: &str, pair_uri: &str, usb: bool, progress: &dyn Fn(&str)) -> Result<(), String> {
    require_app(adb, serial)?;
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
    require_app(adb, serial)?;
    // Re-grant in case the app was reinstalled since setup: it lets the app switch
    // developer options and wireless debugging on by itself.
    let _ = shell(adb, serial, &format!("pm grant {PACKAGE} android.permission.WRITE_SECURE_SETTINGS"));
    shell(adb, serial, &launch_command("")).map(|_| ())
}

// ---------- wireless pairing (Android 11+ "Pair device with QR code") ----------

pub struct WifiPairing {
    pub name: String,
    pub password: String,
}

/// Where the phone said its pairing (and connect) service listens; see `Event::AdbPairHint`.
#[derive(Clone, Debug)]
pub struct PairHint {
    pub pair_addr: String,
    pub connect_addr: Option<String>,
}

impl WifiPairing {
    /// One QR for both ways in (protocol §7). The service name carries this PC's IPv4
    /// addresses (`sb-<hex ip>-...-<random>`) and the password is a SyncBoard pairing
    /// token. Android's wireless-debugging scanner uses them for adb pairing (the phone
    /// then advertises its pairing service under that name, which the SyncBoard app reads
    /// to report the port when mDNS doesn't reach the PC). The SyncBoard app's own
    /// scanner, when the phone service already runs, pairs directly with the IPs + token.
    pub fn new(ips: &[String], token: String) -> Self {
        let r = syncboard_core::crypto::random_bytes::<16>();
        const ABC: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
        let pick = |b: &[u8]| b.iter().map(|x| ABC[*x as usize % ABC.len()] as char).collect::<String>();
        let mut name = String::from("sb");
        for ip in ips.iter().filter_map(|ip| ip.parse::<std::net::Ipv4Addr>().ok()).take(4) {
            name.push('-');
            name.extend(ip.octets().iter().map(|b| format!("{b:02x}")));
        }
        name.push('-');
        name.push_str(&pick(&r[..6]));
        WifiPairing { name, password: token }
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
    hint: &std::sync::Mutex<Option<PairHint>>,
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
        // Whichever comes first: the phone app's report, or our own mDNS discovery.
        if let Some(h) = hint.lock().unwrap().as_ref() {
            break h.pair_addr.clone();
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
        let reported = hint.lock().unwrap().as_ref().and_then(|h| h.connect_addr.clone());
        if let Some(addr) = reported {
            if run(adb, &["connect", &addr]).is_ok_and(|o| o.contains("connected")) {
                return Ok(addr);
            }
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

pub fn disconnect(adb: &Path, serial: &str) {
    let _ = run(adb, &["disconnect", serial]);
}
