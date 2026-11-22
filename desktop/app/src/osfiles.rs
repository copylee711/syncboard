//! Files on the Windows clipboard and dragged out to other apps, as Explorer does it: a
//! shell data object (CF_HDROP and friends) for a list of local paths. Phone files are
//! first pulled into a temporary folder ([stage]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use windows::core::HSTRING;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard};
use windows::Win32::System::Ole::{OleFlushClipboard, OleSetClipboard, CF_HDROP, DROPEFFECT_COPY};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{DragQueryFileW, ILFree, SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHParseDisplayName, BHID_DataObject, HDROP, IShellItemArray};

/// Phone files already pulled: (peer, path, size, mtime) → local copy.
#[derive(Default)]
pub struct Staged(Mutex<HashMap<(String, String, u64, u64), PathBuf>>);

fn root() -> PathBuf {
    std::env::temp_dir().join("SyncBoard").join("out")
}

/// Removes copies older than a day (pasted long ago); run at startup.
pub fn clean_old() {
    let Ok(rd) = std::fs::read_dir(root()) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() > 86_400);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[derive(serde::Deserialize)]
pub struct PhoneFile {
    path: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    mtime: u64,
}

/// Local copies of phone files (pulled once, reused while unchanged).
#[tauri::command]
pub async fn stage_phone(app: AppHandle, peer: String, files: Vec<PhoneFile>) -> Result<Vec<String>, String> {
    let engine = app.state::<crate::App>().engine.clone();
    let mut out = Vec::new();
    for f in files {
        let key = (peer.clone(), f.path.clone(), f.size, f.mtime);
        let cached = app.state::<Staged>().0.lock().unwrap().get(&key).cloned();
        if let Some(p) = cached.filter(|p| p.is_file()) {
            out.push(p.to_string_lossy().into_owned());
            continue;
        }
        let name = f.path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("file").to_string();
        let dest = root().join(syncboard_core::crypto::random_id()).join(&name);
        let (_, rx) = engine.pull_file(&peer, &f.path, &dest).await;
        rx.await.map_err(|_| "已取消".to_string())?.map_err(|e| format!("{name}：{e}"))?;
        app.state::<Staged>().0.lock().unwrap().insert(key, dest.clone());
        out.push(dest.to_string_lossy().into_owned());
    }
    Ok(out)
}

/// A shell data object for local paths, like the one Explorer makes for a selection.
unsafe fn data_object(paths: &[String]) -> windows::core::Result<IDataObject> {
    let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
    let r = (|| {
        for p in paths {
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(&HSTRING::from(p.as_str()), None, &mut pidl, 0, None)?;
            pidls.push(pidl);
        }
        let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
        let arr: IShellItemArray = SHCreateShellItemArrayFromIDLists(&consts)?;
        arr.BindToHandler(None, &BHID_DataObject)
    })();
    for p in pidls {
        ILFree(Some(p));
    }
    r
}

/// Runs `f` on the main thread (it has OLE set up and owns the windows) and waits for it.
async fn on_main<T: Send + 'static>(app: &AppHandle, f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())
}

/// Puts files on the Windows clipboard (paste in Explorer, chat apps, browsers...).
/// Returns the clipboard sequence number after, to tell later changes apart.
#[tauri::command]
pub async fn os_copy(app: AppHandle, paths: Vec<String>) -> Result<u32, String> {
    on_main(&app, move || unsafe {
        let obj = data_object(&paths).map_err(|e| e.message())?;
        OleSetClipboard(&obj).map_err(|e| e.message())?;
        // Render it now, so it stays after this window closes.
        let _ = OleFlushClipboard();
        Ok(GetClipboardSequenceNumber())
    })
    .await?
}

/// Drags files out of the window, to the desktop or another app. Only while the left
/// button is still down (preparing phone files can outlast the press); false if it wasn't.
#[tauri::command]
pub async fn os_drag(app: AppHandle, window: tauri::WebviewWindow, paths: Vec<String>) -> Result<bool, String> {
    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
    on_main(&app, move || unsafe {
        if GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000 == 0 {
            return Ok(false);
        }
        let obj = data_object(&paths).map_err(|e| e.message())?;
        let _ = SHDoDragDrop(Some(HWND(hwnd as _)), &obj, None, DROPEFFECT_COPY);
        Ok(true)
    })
    .await?
}

/// Files on the Windows clipboard put there by someone else since `since` (e.g. copied in
/// Explorer), for pasting into a pane; None if there are none.
#[tauri::command]
pub fn os_clip_files(since: u32) -> Option<Vec<String>> {
    unsafe {
        if GetClipboardSequenceNumber() == since || IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_err() {
            return None;
        }
        OpenClipboard(None).ok()?;
        let files = (|| {
            let h: HANDLE = GetClipboardData(CF_HDROP.0 as u32).ok()?;
            let drop = HDROP(h.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            let mut v = Vec::new();
            for i in 0..n {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(drop, i, Some(&mut buf));
                v.push(String::from_utf16_lossy(&buf[..len]));
            }
            (!v.is_empty()).then_some(v)
        })();
        let _ = CloseClipboard();
        files
    }
}
