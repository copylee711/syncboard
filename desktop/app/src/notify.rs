//! Windows notifications (toasts) for things that arrive from the phone. Windows draws
//! them; SyncBoard keeps nothing in memory for them.
//!
//! Windows shows toasts only for an AppUserModelID it knows. A portable exe has none, so
//! SyncBoard registers one under HKCU\Software\Classes\AppUserModelId (name and icon on
//! toasts) and puts a Start-menu shortcut carrying it: recent Windows 11 builds accept toasts
//! for a registry-only ID but never display them.

use std::sync::OnceLock;
use windows::core::HSTRING;
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const AUMID: &str = "SyncBoard.Desktop";
static ICON: &[u8] = include_bytes!("../icons/128x128.png");

fn register() -> bool {
    static DONE: OnceLock<bool> = OnceLock::new();
    *DONE.get_or_init(|| {
        let icon = std::env::var_os("LOCALAPPDATA")
            .map(|d| std::path::PathBuf::from(d).join("SyncBoard").join("toast-icon.png"));
        if let Some(p) = &icon {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            if std::fs::metadata(p).map(|m| m.len() != ICON.len() as u64).unwrap_or(true) {
                let _ = std::fs::write(p, ICON);
            }
        }
        if let Err(e) = shortcut() {
            eprintln!("[notify] start-menu shortcut: {e}");
        }
        let key = RegKey::predef(HKEY_CURRENT_USER).create_subkey(format!(r"Software\Classes\AppUserModelId\{AUMID}"));
        match key {
            Ok((k, _)) => {
                let _ = k.set_value("DisplayName", &"SyncBoard");
                if let Some(p) = icon {
                    let _ = k.set_value("IconUri", &p.display().to_string());
                }
                true
            }
            Err(e) => {
                eprintln!("[notify] cannot register: {e}");
                false
            }
        }
    })
}

/// %APPDATA%\Microsoft\Windows\Start Menu\Programs\SyncBoard.lnk → this exe, with our AUMID.
fn shortcut() -> windows::core::Result<()> {
    use windows::core::{Interface, HSTRING, PWSTR};
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    let (Some(appdata), Ok(exe)) = (std::env::var_os("APPDATA"), std::env::current_exe()) else { return Ok(()) };
    let lnk = std::path::PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\SyncBoard.lnk");
    unsafe {
        // May already be initialized on this thread; either way COM is usable.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        if let Some(dir) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        let store: IPropertyStore = link.cast()?;
        // A VT_LPWSTR PROPVARIANT; PropVariantClear frees the CoTaskMem string.
        let wide: Vec<u16> = AUMID.encode_utf16().chain(std::iter::once(0)).collect();
        let buf = CoTaskMemAlloc(wide.len() * 2) as *mut u16;
        if buf.is_null() {
            return Err(windows::core::Error::from_win32());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), buf, wide.len());
        let mut pv = PROPVARIANT::default();
        (*pv.Anonymous.Anonymous).vt = VT_LPWSTR;
        (*pv.Anonymous.Anonymous).Anonymous.pwszVal = PWSTR(buf);
        let r = store.SetValue(&PKEY_AppUserModel_ID, &pv).and_then(|_| store.Commit());
        let _ = PropVariantClear(&mut pv);
        r?;
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(lnk.as_os_str()), true)?;
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Shows a toast with a title and up to two lines of text. Never blocks for long, and
/// failures (notifications turned off, focus assist) are silent.
pub fn show(title: &str, lines: &[&str]) {
    if let Err(e) = try_show(title, lines) {
        eprintln!("[notify] {e}");
    }
}

/// [`show`], reporting what went wrong.
pub fn try_show(title: &str, lines: &[&str]) -> Result<(), String> {
    if !register() {
        return Err("无法注册通知（写入注册表失败）".into());
    }
    let text: String = lines.iter().map(|l| format!("<text>{}</text>", xml_escape(l))).collect();
    let xml = format!(
        r#"<toast><visual><binding template="ToastGeneric"><text>{}</text>{text}</binding></visual><audio src="ms-winsoundevent:Notification.SMS"/></toast>"#,
        xml_escape(title)
    );
    let r = (|| -> windows::core::Result<()> {
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?;
        // 0 = Enabled; otherwise turned off for this app, for the user, or by policy.
        // Some Windows builds can't answer for an app without a Start-menu shortcut
        // ("element not found"); showing works anyway.
        let setting = notifier.Setting().map(|s| s.0).unwrap_or(0);
        if setting != 0 {
            return Err(windows::core::Error::new(windows::core::HRESULT(-1), "Windows 已关闭 SyncBoard 的通知，请在「设置 → 系统 → 通知」里打开"));
        }
        notifier.Show(&toast)
    })();
    r.map_err(|e| e.message().to_string())
}
