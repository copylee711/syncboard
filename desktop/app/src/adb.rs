//! Starting the phone daemon over ADB (USB or wireless debugging).

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Must stay in sync with `Main.LAUNCH_COMMAND` in the Android daemon.
const LAUNCH_COMMAND: &str = "p=$(pm path app.syncboard | head -n1 | cut -d: -f2); \
    [ -n \"$p\" ] && CLASSPATH=$p nohup setsid app_process /system/bin --nice-name=syncboard_daemon \
    app.syncboard.daemon.Main </dev/null >/dev/null 2>&1 &";

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
    candidates.into_iter().find(|p| p.is_file())
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

/// Serials of devices that are online and authorized.
pub fn devices(adb: &Path) -> Vec<String> {
    run(adb, &["devices"])
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            match (it.next(), it.next()) {
                (Some(serial), Some("device")) => Some(serial.to_string()),
                _ => None,
            }
        })
        .collect()
}

/// Starts the daemon on one device. Harmless if it is already running.
pub fn start_daemon(adb: &Path, serial: &str) -> Result<(), String> {
    let installed = run(adb, &["-s", serial, "shell", "pm path app.syncboard"])?;
    if !installed.contains("package:") {
        return Err("手机上还没有安装 SyncBoard".into());
    }
    run(adb, &["-s", serial, "shell", LAUNCH_COMMAND])?;
    Ok(())
}

/// Starts the daemon on every connected device; returns a human-readable summary.
pub fn start_all(adb: &Path) -> Result<String, String> {
    let devs = devices(adb);
    if devs.is_empty() {
        return Err("没有检测到 adb 设备。请用 USB 连接手机并允许 USB 调试。".into());
    }
    let mut ok = 0;
    let mut errors = Vec::new();
    for d in &devs {
        match start_daemon(adb, d) {
            Ok(()) => ok += 1,
            Err(e) => errors.push(format!("{d}: {e}")),
        }
    }
    if errors.is_empty() {
        Ok(format!("已在 {ok} 台设备上启动手机服务"))
    } else {
        Err(errors.join("\n"))
    }
}
