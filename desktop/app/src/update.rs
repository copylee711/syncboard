//! Checking GitHub Releases for a newer SyncBoard and replacing this exe in place.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const REPO: &str = "https://github.com/copylee711/syncboard";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn version_tuple(v: &str) -> Vec<u32> {
    v.split(|c: char| !c.is_ascii_digit()).filter_map(|p| p.parse().ok()).collect()
}

/// The proxy set in Windows' 设置 → 网络 → 代理 (what Clash and the like switch on), as a
/// curl `-x` value. curl.exe only reads proxy environment variables on its own.
fn system_proxy() -> Option<String> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    if key.get_value::<u32, _>("ProxyEnable").ok()? == 0 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    parse_proxy(&server)
}

/// "host:port", or per scheme: "http=host:port;https=host:port;socks=host:port".
fn parse_proxy(server: &str) -> Option<String> {
    let server = server.trim();
    if server.is_empty() {
        return None;
    }
    if !server.contains('=') {
        return Some(if server.contains("://") { server.to_string() } else { format!("http://{server}") });
    }
    let entries: Vec<(&str, &str)> = server.split(';').filter_map(|e| e.split_once('=')).map(|(k, v)| (k.trim(), v.trim())).collect();
    let pick = |scheme: &str| entries.iter().find(|(k, v)| k.eq_ignore_ascii_case(scheme) && !v.is_empty()).map(|(_, v)| *v);
    pick("https")
        .or_else(|| pick("http"))
        .map(|v| format!("http://{v}"))
        .or_else(|| pick("socks").map(|v| format!("socks5h://{v}")))
}

/// How to reach GitHub: through the system proxy when one is on, then directly (the proxy
/// may be off or broken while the setting stays on).
fn routes() -> Vec<Option<String>> {
    match system_proxy() {
        Some(p) => vec![Some(p), None],
        None => vec![None],
    }
}

fn curl_cmd(proxy: &Option<String>) -> Command {
    // curl.exe ships with Windows 10 1803+.
    let mut c = Command::new("curl.exe");
    if let Some(p) = proxy {
        c.args(["-x", p]);
    }
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let mut err = String::new();
    for proxy in routes() {
        let out = curl_cmd(&proxy)
            .args(["-fsS", "--connect-timeout", "15"])
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(out.stdout);
        }
        err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    }
    Err(err)
}

/// The latest release's version, read from where `/releases/latest` redirects
/// (unlike the REST API, this has no rate limit).
pub fn latest_version() -> Result<String, String> {
    let head = curl(&["-I", "-m", "20", &format!("{REPO}/releases/latest")])
        .map_err(|e| format!("无法连接 GitHub：{e}"))?;
    String::from_utf8_lossy(&head)
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("location").then(|| v.trim().rsplit("/tag/v").next().map(str::to_owned))?
        })
        .filter(|v| !version_tuple(v).is_empty())
        .ok_or_else(|| "没有找到发布版本".into())
}

/// The phone APK released with this desktop version.
pub fn apk_name() -> String {
    format!("SyncBoard-{VERSION}.apk")
}

/// This version's release page. The install QR points here rather than at the APK:
/// a direct link opened from a QR scanner lands in a Chrome Custom Tab, which can stall
/// at 100% without finishing the download; tapping the APK on the page works.
pub fn release_page() -> String {
    format!("{REPO}/releases/tag/v{VERSION}")
}

fn old_exe(exe: &Path) -> PathBuf {
    exe.with_extension("exe.old")
}

/// Removes the exe left behind by the previous update.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_exe(&exe));
    }
}

/// Downloads `version`, checks it against the release's SHA256SUMS.txt and swaps it in
/// for the running exe (Windows allows renaming a running exe). The caller restarts.
pub fn install(version: &str, progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
    let name = format!("SyncBoard-{version}-windows-x64.exe");
    let base = format!("{REPO}/releases/download/v{version}");
    let sums = curl(&["-L", "-m", "30", &format!("{base}/SHA256SUMS.txt")]).map_err(|e| format!("下载校验文件失败：{e}"))?;
    let want = String::from_utf8_lossy(&sums)
        .lines()
        .find_map(|l| l.split_once(char::is_whitespace).filter(|(_, f)| f.trim() == name).map(|(h, _)| h.to_lowercase()))
        .ok_or("发布中没有 Windows 版本")?;

    let tmp = std::env::temp_dir().join(format!("{name}.part"));
    let _ = std::fs::remove_file(&tmp);
    download(&format!("{base}/{name}"), &tmp, progress).map_err(|e| format!("下载失败：{e}"))?;
    let bytes = std::fs::read(&tmp).map_err(|e| e.to_string())?;
    if syncboard_core::crypto::sha256_hex(&bytes) != want {
        let _ = std::fs::remove_file(&tmp);
        return Err("下载的文件校验失败，请重试".into());
    }

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = old_exe(&exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("无法替换程序文件：{e}"))?;
    // Copy, not rename: the temp dir may be on another drive.
    if let Err(e) = std::fs::copy(&tmp, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("无法写入新版本：{e}"));
    }
    let _ = std::fs::remove_file(&tmp);
    Ok(exe)
}

/// Size of what `url` finally redirects to (0 when unknown).
fn content_length(url: &str) -> u64 {
    let Ok(head) = curl(&["-I", "-L", "-m", "20", url]) else { return 0 };
    String::from_utf8_lossy(&head)
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<u64>().ok())?
        })
        .last()
        .unwrap_or(0)
}

/// Downloads `url` into `to`, reporting (bytes so far, total) while curl runs.
fn download(url: &str, to: &Path, progress: impl Fn(u64, u64)) -> Result<(), String> {
    let total = content_length(url);
    let mut err = String::new();
    for proxy in routes() {
        let _ = std::fs::remove_file(to);
        match download_via(&proxy, url, to, total, &progress) {
            Ok(()) => return Ok(()),
            Err(e) => err = e,
        }
    }
    Err(err)
}

fn download_via(proxy: &Option<String>, url: &str, to: &Path, total: u64, progress: &impl Fn(u64, u64)) -> Result<(), String> {
    let mut child = curl_cmd(proxy)
        .args(["-fsSL", "--connect-timeout", "15", "--retry", "2", "-o"])
        .arg(to)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    loop {
        let done = std::fs::metadata(to).map(|m| m.len()).unwrap_or(0);
        progress(done, total);
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if status.success() {
                return Ok(());
            }
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut e, &mut err);
            }
            return Err(err.trim().to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Starts `exe` once this process has exited (the single-instance lock would otherwise
/// hand the launch back to us).
pub fn relaunch_after_exit(exe: &Path) {
    let ps = format!(
        "Wait-Process -Id {} -Timeout 20 -ErrorAction SilentlyContinue; Start-Process -FilePath '{}'",
        std::process::id(),
        exe.display().to_string().replace('\'', "''"),
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &ps])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_server_formats() {
        assert_eq!(parse_proxy("127.0.0.1:7890").as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(parse_proxy("http=a:1;https=b:2").as_deref(), Some("http://b:2"));
        assert_eq!(parse_proxy("socks=c:3").as_deref(), Some("socks5h://c:3"));
        assert_eq!(parse_proxy(""), None);
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_tuple("0.10.0") > version_tuple("0.9.3"));
        assert_eq!(version_tuple("v0.2.1"), vec![0, 2, 1]);
    }
}
