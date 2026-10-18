//! Small Windows integrations: autostart, "Send to" menu, working-set trimming.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "SyncBoard";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn exe() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

pub fn autostart_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_READ)
        .and_then(|k| k.get_value::<String, _>(VALUE))
        .map(|v| v.contains(&*exe().to_string_lossy()))
        .unwrap_or(false)
}

pub fn set_autostart(on: bool) -> std::io::Result<()> {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_WRITE)?;
    if on {
        key.set_value(VALUE, &format!("\"{}\" --hidden", exe().display()))
    } else {
        match key.delete_value(VALUE) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

fn sendto_link() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join(r"Microsoft\Windows\SendTo\SyncBoard 手机.lnk"))
}

pub fn sendto_enabled() -> bool {
    sendto_link().is_some_and(|p| p.exists())
}

/// Adds or removes an Explorer "Send to" entry that runs `SyncBoard.exe --send <files>`.
pub fn set_sendto(on: bool) -> Result<(), String> {
    let link = sendto_link().ok_or("找不到 SendTo 文件夹")?;
    if !on {
        let _ = std::fs::remove_file(&link);
        return Ok(());
    }
    let ps = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Arguments='--send';$s.IconLocation='{},0';$s.Save()",
        link.display().to_string().replace('\'', "''"),
        exe().display().to_string().replace('\'', "''"),
        exe().display().to_string().replace('\'', "''"),
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err("创建快捷方式失败".into()) }
}

/// Returns freed pages to the OS; used after the window is destroyed (light mode).
pub fn trim_working_set() {
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        EmptyWorkingSet(GetCurrentProcess());
    }
}

pub fn open_path(p: &std::path::Path) {
    let _ = std::fs::create_dir_all(p);
    let _ = Command::new("explorer").arg(p).spawn();
}

/// Opens an http(s) URL in the default browser.
pub fn open_url(url: &str) {
    if url.starts_with("https://") || url.starts_with("http://") {
        let _ = Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    }
}

/// Image names (lowercase) of the running processes.
pub fn process_names() -> std::collections::HashSet<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut names = std::collections::HashSet::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return names;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            names.insert(String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase());
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    names
}

/// Terminates direct child processes of this process with the given image name.
pub fn kill_child_processes(image: &str) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return;
        }
        let me = GetCurrentProcessId();
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            if e.th32ParentProcessID == me && name.eq_ignore_ascii_case(image) {
                let h = OpenProcess(PROCESS_TERMINATE, 0, e.th32ProcessID);
                if !h.is_null() {
                    TerminateProcess(h, 0);
                    CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
}
