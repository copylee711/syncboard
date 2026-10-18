//! The file manager window: this PC's files, the phone's files (protocol §8) and a
//! transfer queue between them. Everything here runs only while the window is open,
//! apart from the queue, which finishes what was started.

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use syncboard_core::Engine;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const FILES: &str = "files";
const FS_TIMEOUT: Duration = Duration::from_secs(20);
/// Copying or moving big folders on the phone takes a while.
const FS_LONG_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(FILES) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    if let Err(e) = WebviewWindowBuilder::new(app, FILES, WebviewUrl::App("files.html".into()))
        .title("SyncBoard 文件管理")
        .inner_size(1120.0, 720.0)
        .min_inner_size(620.0, 420.0)
        .decorations(false)
        .shadow(true)
        .center()
        .build()
    {
        eprintln!("[files] cannot create window: {e}");
    }
}

fn engine(app: &AppHandle) -> Arc<Engine> {
    app.state::<crate::App>().engine.clone()
}

fn ms(t: std::io::Result<std::time::SystemTime>) -> u64 {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ---------- this PC ----------

/// Async on purpose: creating a window from a synchronous command deadlocks WebView2 on
/// Windows (the new window stays blank).
#[tauri::command]
pub async fn open_files(app: AppHandle) {
    show(&app);
}

/// Quick-access folders and drives.
#[tauri::command]
pub fn local_places() -> Value {
    let mut places = Vec::new();
    for (name, id) in [
        ("桌面", &windows_sys::Win32::UI::Shell::FOLDERID_Desktop),
        ("下载", &windows_sys::Win32::UI::Shell::FOLDERID_Downloads),
        ("文档", &windows_sys::Win32::UI::Shell::FOLDERID_Documents),
        ("图片", &windows_sys::Win32::UI::Shell::FOLDERID_Pictures),
        ("视频", &windows_sys::Win32::UI::Shell::FOLDERID_Videos),
    ] {
        if let Some(p) = known_folder(id) {
            places.push(json!({"name": name, "path": p}));
        }
    }
    let drives: Vec<Value> = (b'A'..=b'Z')
        .map(|c| format!("{}:\\", c as char))
        .filter(|d| Path::new(d).exists())
        .map(|d| json!({"name": d.trim_end_matches('\\'), "path": d}))
        .collect();
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
    json!({"home": home, "places": places, "drives": drives})
}

fn known_folder(id: &windows_sys::core::GUID) -> Option<String> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
    unsafe {
        let mut p: windows_sys::core::PWSTR = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut p);
        if hr != 0 || p.is_null() {
            return None;
        }
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        CoTaskMemFree(p as *const _);
        Some(s)
    }
}

#[tauri::command]
pub async fn local_list(path: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let rd = std::fs::read_dir(&path).map_err(|e| io_err(&e))?;
        let mut out = Vec::new();
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            let attrs = m.file_attributes();
            // FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM
            let hidden = attrs & 0x2 != 0 || attrs & 0x4 != 0;
            out.push(json!({"n": e.file_name().to_string_lossy(), "d": m.is_dir(),
                            "s": if m.is_dir() { 0 } else { m.len() }, "m": ms(m.modified()), "h": hidden}));
        }
        Ok(json!({"path": path, "entries": out}))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn io_err(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "找不到这个位置".into(),
        std::io::ErrorKind::PermissionDenied => "没有权限".into(),
        std::io::ErrorKind::AlreadyExists => "已存在同名文件或文件夹".into(),
        _ => e.to_string(),
    }
}

#[tauri::command]
pub fn local_mkdir(path: String) -> Result<(), String> {
    if Path::new(&path).exists() {
        return Err("已存在同名文件或文件夹".into());
    }
    std::fs::create_dir_all(&path).map_err(|e| io_err(&e))
}

#[tauri::command]
pub fn local_rename(path: String, to: String) -> Result<(), String> {
    if Path::new(&to).exists() {
        return Err("已存在同名文件或文件夹".into());
    }
    std::fs::rename(&path, &to).map_err(|e| io_err(&e))
}

/// Moves to the Recycle Bin.
#[tauri::command]
pub async fn local_delete(paths: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || recycle(&paths)).await.map_err(|e| e.to_string())?
}

fn recycle(paths: &[String]) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
    };
    if paths.is_empty() {
        return Ok(());
    }
    // Double-NUL-terminated list of NUL-separated paths.
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(p.encode_utf16());
        from.push(0);
    }
    from.push(0);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16;
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 || op.fAnyOperationsAborted != 0 {
        return Err(format!("删除失败（{r:#x}）"));
    }
    Ok(())
}

#[tauri::command]
pub fn local_open(path: String) {
    let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", &path]).spawn();
}

#[tauri::command]
pub fn local_reveal(path: String) {
    let _ = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
}

/// Copies or moves files and folders into `dir` on this PC (names made unique).
#[tauri::command]
pub async fn local_paste(paths: Vec<String>, dir: String, mv: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = PathBuf::from(dir);
        for p in paths {
            let src = PathBuf::from(&p);
            let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if dir.starts_with(&src) {
                return Err("不能放进它自己里面".to_string());
            }
            if mv && src.parent() == Some(dir.as_path()) {
                continue;
            }
            let dst = unique_path(&dir, &name);
            if mv && std::fs::rename(&src, &dst).is_ok() {
                continue;
            }
            copy_rec(&src, &dst).map_err(|e| format!("{name}：{}", io_err(&e)))?;
            if mv {
                let r = if src.is_dir() { std::fs::remove_dir_all(&src) } else { std::fs::remove_file(&src) };
                r.map_err(|e| io_err(&e))?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn copy_rec(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_rec(&e.path(), &dst.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap()
}

// ---------- the phone ----------

/// A phone file op (protocol §8) other than listing and transfers.
#[tauri::command]
pub async fn phone_fs(app: AppHandle, peer: String, op: String, args: Value) -> Result<Value, String> {
    let timeout = match op.as_str() {
        "copy" | "move" | "delete" => FS_LONG_TIMEOUT,
        // The first request: an older phone app never answers, so don't keep the user waiting.
        "roots" | "status" => Duration::from_secs(6),
        _ => FS_TIMEOUT,
    };
    let r = engine(&app).fs(&peer, &op, args, timeout).await.map_err(|e| e.to_string())?;
    Ok(r.header)
}

/// All pages of a phone listing (`list` or `walk`).
async fn phone_pages(engine: &Engine, peer: &str, op: &str, path: &str) -> Result<Vec<Value>, String> {
    let mut all = Vec::new();
    let mut offset = 0u64;
    loop {
        let r = engine.fs(peer, op, json!({"path": path, "offset": offset}), FS_TIMEOUT).await.map_err(|e| e.to_string())?;
        let page: Vec<Value> = serde_json::from_slice(&r.body).map_err(|e| e.to_string())?;
        all.extend(page);
        match r.header.get("next").and_then(Value::as_u64) {
            Some(n) => offset = n,
            None => return Ok(all),
        }
    }
}

#[tauri::command]
pub async fn phone_list(app: AppHandle, peer: String, path: String) -> Result<Value, String> {
    let entries = phone_pages(&engine(&app), &peer, "list", &path).await?;
    Ok(json!({"path": path, "entries": entries}))
}

/// A small preview as a data: URL.
#[tauri::command]
pub async fn phone_thumb(app: AppHandle, peer: String, path: String, px: u32) -> Result<String, String> {
    let r = engine(&app).fs(&peer, "thumb", json!({"path": path, "px": px}), FS_TIMEOUT).await.map_err(|e| e.to_string())?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(&r.body)))
}

/// Opens a phone file with its Windows app, from a temporary copy.
#[tauri::command]
pub async fn phone_open(app: AppHandle, peer: String, path: String) -> Result<(), String> {
    let name = path.rsplit('/').next().unwrap_or("file").to_string();
    let dir = std::env::temp_dir().join("SyncBoard").join(syncboard_core::crypto::random_id());
    let dest = dir.join(&name);
    let (_, rx) = engine(&app).pull_file(&peer, &path, &dest).await;
    rx.await.map_err(|_| "已取消".to_string())??;
    local_open(dest.to_string_lossy().into_owned());
    Ok(())
}

// ---------- transfer queue ----------

#[derive(Deserialize, Clone)]
pub struct Job {
    /// "up" (PC → phone) or "down" (phone → PC).
    dir: String,
    peer: String,
    /// Source paths (PC paths for up, phone paths for down).
    paths: Vec<String>,
    /// Destination folder on the other side.
    dest: String,
    /// Delete the sources once everything arrived (cut and paste).
    #[serde(default)]
    mv: bool,
}

struct Item {
    qid: u64,
    job: Job,
    /// The source being transferred (one per item).
    src: String,
    name: String,
    state: &'static str,
    error: Option<String>,
    done: u64,
    total: u64,
    files_done: u64,
    files: u64,
    /// The engine transfer in flight, for cancelling.
    current: Option<String>,
    current_done: u64,
}

impl Item {
    fn json(&self) -> Value {
        json!({"qid": self.qid, "name": self.name, "dir": self.job.dir, "dest": self.job.dest, "mv": self.job.mv, "state": self.state,
               "error": self.error, "done": self.done + self.current_done, "total": self.total,
               "filesDone": self.files_done, "files": self.files})
    }
}

#[derive(Default)]
pub struct Queue {
    items: Mutex<VecDeque<Item>>,
    running: Mutex<bool>,
    next: AtomicU64,
    /// Engine transfer ids started here (their events arrive after the transfer ends).
    owned: Mutex<std::collections::HashSet<String>>,
}

impl Queue {
    pub fn owns(&self, id: &str) -> bool {
        self.owned.lock().unwrap().contains(id)
    }
}

fn emit_queue(app: &AppHandle) {
    let q = &app.state::<crate::App>().queue;
    let items: Vec<Value> = q.items.lock().unwrap().iter().map(Item::json).collect();
    if app.get_webview_window(FILES).is_some() {
        let _ = app.emit_to(FILES, "queue", items);
    }
}

#[tauri::command]
pub fn queue_state(app: AppHandle) -> Value {
    let q = &app.state::<crate::App>().queue;
    let items: Vec<Value> = q.items.lock().unwrap().iter().map(Item::json).collect();
    json!(items)
}

#[tauri::command]
pub fn queue_add(app: AppHandle, job: Job) {
    {
        let q = &app.state::<crate::App>().queue;
        let mut items = q.items.lock().unwrap();
        for src in &job.paths {
            let name = src.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(src).to_string();
            items.push_back(Item {
                qid: q.next.fetch_add(1, Ordering::Relaxed) + 1,
                job: job.clone(),
                src: src.clone(),
                name,
                state: "queued",
                error: None,
                done: 0,
                total: 0,
                files_done: 0,
                files: 0,
                current: None,
                current_done: 0,
            });
        }
    }
    emit_queue(&app);
    let q = &app.state::<crate::App>().queue;
    let mut running = q.running.lock().unwrap();
    if !*running {
        *running = true;
        let app = app.clone();
        tauri::async_runtime::spawn(async move { run_queue(app).await });
    }
}

#[tauri::command]
pub fn queue_cancel(app: AppHandle, qid: u64) {
    let current = {
        let q = &app.state::<crate::App>().queue;
        let mut items = q.items.lock().unwrap();
        let Some(it) = items.iter_mut().find(|i| i.qid == qid) else { return };
        if it.state == "queued" || it.state == "running" {
            it.state = "cancelled";
        }
        it.current.take()
    };
    if let Some(id) = current {
        engine(&app).cancel_transfer(&id);
    }
    emit_queue(&app);
}

/// Removes finished items (or `qid` alone).
#[tauri::command]
pub fn queue_clear(app: AppHandle, qid: Option<u64>) {
    {
        let q = &app.state::<crate::App>().queue;
        let mut items = q.items.lock().unwrap();
        items.retain(|i| match qid {
            Some(id) => i.qid != id || matches!(i.state, "queued" | "running"),
            None => matches!(i.state, "queued" | "running"),
        });
    }
    emit_queue(&app);
}

/// Progress of the engine transfer `id`, from `Event::FileProgress`.
pub fn on_progress(app: &AppHandle, id: &str, done: u64) {
    let hit = {
        let q = &app.state::<crate::App>().queue;
        let mut items = q.items.lock().unwrap();
        match items.iter_mut().find(|i| i.current.as_deref() == Some(id)) {
            Some(it) => {
                it.current_done = done;
                true
            }
            None => false,
        }
    };
    if hit {
        emit_queue(app);
    }
}

fn with_item<T>(app: &AppHandle, qid: u64, f: impl FnOnce(&mut Item) -> T) -> Option<T> {
    let q = &app.state::<crate::App>().queue;
    let mut items = q.items.lock().unwrap();
    items.iter_mut().find(|i| i.qid == qid).map(f)
}

fn cancelled(app: &AppHandle, qid: u64) -> bool {
    with_item(app, qid, |i| i.state == "cancelled").unwrap_or(true)
}

async fn run_queue(app: AppHandle) {
    loop {
        let next = {
            let q = &app.state::<crate::App>().queue;
            let mut items = q.items.lock().unwrap();
            let it = items.iter_mut().find(|i| i.state == "queued");
            match it {
                Some(it) => {
                    it.state = "running";
                    Some((it.qid, it.job.clone(), it.src.clone()))
                }
                None => {
                    *q.running.lock().unwrap() = false;
                    None
                }
            }
        };
        let Some((qid, job, src)) = next else { break };
        emit_queue(&app);
        let r = if job.dir == "up" { upload(&app, qid, &job, &src).await } else { download(&app, qid, &job, &src).await };
        let r = match r {
            Ok(()) if job.mv && !cancelled(&app, qid) => delete_source(&app, &job, &src).await,
            r => r,
        };
        with_item(&app, qid, |it| {
            it.current = None;
            it.current_done = 0;
            if it.state != "cancelled" {
                match r {
                    Ok(()) => it.state = "done",
                    Err(e) => {
                        it.state = "failed";
                        it.error = Some(e);
                    }
                }
            }
        });
        emit_queue(&app);
    }
}

async fn delete_source(app: &AppHandle, job: &Job, src: &str) -> Result<(), String> {
    if job.dir == "up" {
        let p = PathBuf::from(src);
        let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        r.map_err(|e| format!("已复制，但删除原文件失败：{}", io_err(&e)))
    } else {
        engine(app)
            .fs(&job.peer, "delete", json!({"paths": [src]}), FS_LONG_TIMEOUT)
            .await
            .map(|_| ())
            .map_err(|e| format!("已复制，但删除原文件失败：{e}"))
    }
}

fn join_phone(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

/// A phone name in `dir` not taken yet, like `name (1).ext`.
fn unique_phone_name(taken: &[String], name: &str) -> String {
    if !taken.iter().any(|t| t == name) {
        return name.into();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..).map(|n| format!("{stem} ({n}){ext}")).find(|c| !taken.iter().any(|t| t == c)).unwrap()
}

/// Waits for one engine transfer, recording it as the item's current one.
async fn track(app: &AppHandle, qid: u64, id: String, rx: tokio::sync::oneshot::Receiver<syncboard_core::engine::TransferResult>, size: u64) -> Result<(), String> {
    if !id.is_empty() {
        app.state::<crate::App>().queue.owned.lock().unwrap().insert(id.clone());
        with_item(app, qid, |it| {
            it.current = Some(id.clone());
            it.current_done = 0;
        });
    }
    let r = rx.await.map_err(|_| "已取消".to_string())?;
    with_item(app, qid, |it| {
        it.current = None;
        it.current_done = 0;
        if r.is_ok() {
            it.done += size;
            it.files_done += 1;
        }
    });
    emit_queue(app);
    r.map(|_| ())
}

async fn upload(app: &AppHandle, qid: u64, job: &Job, src: &str) -> Result<(), String> {
    let e = engine(app);
    let src = PathBuf::from(src);
    let meta = std::fs::metadata(&src).map_err(|e| io_err(&e))?;
    if !meta.is_dir() {
        with_item(app, qid, |it| {
            it.total = meta.len();
            it.files = 1;
        });
        let (id, rx) = e.push_file(&job.peer, &src, &job.dest).await;
        return track(app, qid, id, rx, meta.len()).await;
    }
    // A folder: recreate it on the phone (with a unique name), then send its files.
    let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    fn walk(base: &Path, rel: &str, files: &mut Vec<(PathBuf, String, u64)>, dirs: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(base.join(rel.replace('/', "\\"))) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            match e.metadata() {
                Ok(m) if m.is_dir() => {
                    dirs.push(r.clone());
                    walk(base, &r, files, dirs);
                }
                Ok(m) => files.push((e.path(), r, m.len())),
                Err(_) => {}
            }
        }
    }
    walk(&src, "", &mut files, &mut dirs);
    with_item(app, qid, |it| {
        it.total = files.iter().map(|f| f.2).sum();
        it.files = files.len() as u64;
    });
    emit_queue(app);
    let taken: Vec<String> = phone_pages(&e, &job.peer, "list", &job.dest)
        .await?
        .iter()
        .filter_map(|v| v.get("n").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "folder".into());
    let root = join_phone(&job.dest, &unique_phone_name(&taken, &name));
    e.fs(&job.peer, "mkdir", json!({"path": root}), FS_TIMEOUT).await.map_err(|e| e.to_string())?;
    for d in &dirs {
        if cancelled(app, qid) {
            return Ok(());
        }
        e.fs(&job.peer, "mkdir", json!({"path": join_phone(&root, d)}), FS_TIMEOUT).await.map_err(|e| e.to_string())?;
    }
    for (path, rel, size) in files {
        if cancelled(app, qid) {
            return Ok(());
        }
        let dir = match rel.rfind('/') {
            Some(i) => join_phone(&root, &rel[..i]),
            None => root.clone(),
        };
        let (id, rx) = e.push_file(&job.peer, &path, &dir).await;
        track(app, qid, id, rx, size).await.map_err(|err| format!("{rel}：{err}"))?;
    }
    Ok(())
}

async fn download(app: &AppHandle, qid: u64, job: &Job, src: &str) -> Result<(), String> {
    let e = engine(app);
    let name = src.rsplit('/').find(|s| !s.is_empty()).unwrap_or("file").to_string();
    let dest_dir = PathBuf::from(&job.dest);
    // Is it a folder? Ask for the listing of its parent entry.
    let parent = src.rsplit_once('/').map(|(p, _)| if p.is_empty() { "/" } else { p }).unwrap_or("/");
    let siblings = phone_pages(&e, &job.peer, "list", parent).await?;
    let me = siblings.iter().find(|v| v.get("n").and_then(Value::as_str) == Some(name.as_str()));
    let is_dir = me.and_then(|v| v.get("d")).and_then(Value::as_bool).unwrap_or(false);
    if !is_dir {
        let size = me.and_then(|v| v.get("s")).and_then(Value::as_u64).unwrap_or(0);
        with_item(app, qid, |it| {
            it.total = size;
            it.files = 1;
        });
        let dest = unique_path(&dest_dir, &name);
        let (id, rx) = e.pull_file(&job.peer, src, &dest).await;
        return track(app, qid, id, rx, size).await;
    }
    let all = phone_pages(&e, &job.peer, "walk", src).await?;
    let files: Vec<(String, u64)> = all
        .iter()
        .filter(|v| v.get("d").and_then(Value::as_bool) == Some(false))
        .filter_map(|v| Some((v.get("p")?.as_str()?.to_string(), v.get("s").and_then(Value::as_u64).unwrap_or(0))))
        .collect();
    with_item(app, qid, |it| {
        it.total = files.iter().map(|f| f.1).sum();
        it.files = files.len() as u64;
    });
    emit_queue(app);
    let root = unique_path(&dest_dir, &name);
    std::fs::create_dir_all(&root).map_err(|e| io_err(&e))?;
    for v in &all {
        if v.get("d").and_then(Value::as_bool) == Some(true) {
            if let Some(p) = v.get("p").and_then(Value::as_str) {
                let _ = std::fs::create_dir_all(root.join(p.replace('/', "\\")));
            }
        }
    }
    for (rel, size) in files {
        if cancelled(app, qid) {
            return Ok(());
        }
        let dest = root.join(rel.replace('/', "\\"));
        let (id, rx) = e.pull_file(&job.peer, &join_phone(src, &rel), &dest).await;
        track(app, qid, id, rx, size).await.map_err(|err| format!("{rel}：{err}"))?;
    }
    Ok(())
}
