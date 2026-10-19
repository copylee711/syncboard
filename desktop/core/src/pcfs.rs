//! Answers file-manager requests from a paired device about this PC's own files
//! (protocol §8, "PC side"). Only active while the user allows it in settings.
//!
//! Everything here blocks; the engine calls [`handle`] on a blocking thread. Platform
//! parts (drive list, Recycle Bin, thumbnails) come from an [`FsHost`] the app installs.

use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Listing pages, like the phone side.
pub const PAGE: usize = 3000;
const WALK_MAX: usize = 50_000;

/// What the desktop app adds on top of plain `std::fs`.
pub trait FsHost: Send + Sync {
    /// `{"home": path, "volumes": [{name, path, total, free, kind}]}`; `kind` is
    /// "drive" or "place" (Desktop, Downloads...).
    fn roots(&self) -> Value {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| "/".into());
        json!({"home": home, "volumes": [{"name": "主目录", "path": home, "total": 0, "free": 0, "kind": "place"}]})
    }

    /// Deletes `paths`; the desktop app sends them to the Recycle Bin instead.
    fn delete(&self, paths: &[PathBuf]) -> Result<(), String> {
        for p in paths {
            let r = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
            r.map_err(|e| io_err(&e))?;
        }
        Ok(())
    }

    /// A JPEG preview no larger than `px` on its long side.
    fn thumb(&self, _path: &Path, _px: u32) -> Option<Vec<u8>> {
        None
    }
}

/// `std::fs` only: permanent delete, no thumbnails. Used in tests and on other platforms.
pub struct PlainHost;
impl FsHost for PlainHost {}

pub fn io_err(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "找不到这个位置".into(),
        std::io::ErrorKind::PermissionDenied => "没有权限".into(),
        std::io::ErrorKind::AlreadyExists => "已存在同名文件或文件夹".into(),
        _ => e.to_string(),
    }
}

/// Accepts absolute local paths only: no `..`, no UNC shares (`\\server\x`) and no device
/// namespaces (`\\?\`, `\\.\`), so a request can't reach beyond this PC's own disks.
pub fn check_path(p: &str) -> Result<PathBuf, String> {
    if p.is_empty() || p.contains('\0') {
        return Err("路径无效".into());
    }
    if p.starts_with(r"\\") || p.starts_with("//") {
        return Err("不支持网络路径".into());
    }
    let path = PathBuf::from(p);
    if !path.is_absolute() {
        return Err("路径必须是绝对路径".into());
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("路径无效".into());
    }
    Ok(path)
}

fn ms(t: std::io::Result<std::time::SystemTime>) -> u64 {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(windows)]
fn hidden(m: &std::fs::Metadata, _name: &str) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM
    m.file_attributes() & 0x6 != 0
}

#[cfg(not(windows))]
fn hidden(_m: &std::fs::Metadata, name: &str) -> bool {
    name.starts_with('.')
}

/// `{n, d, s, m, h}`: name, is folder, size (0 for folders), mtime ms, hidden.
fn entry(name: &str, m: &std::fs::Metadata) -> Value {
    json!({"n": name, "d": m.is_dir(), "s": if m.is_dir() { 0 } else { m.len() }, "m": ms(m.modified()), "h": hidden(m, name)})
}

fn arg<'a>(args: &'a Value, k: &str) -> Result<&'a str, String> {
    args.get(k).and_then(Value::as_str).ok_or_else(|| format!("缺少参数 {k}"))
}

fn paths(args: &Value) -> Result<Vec<PathBuf>, String> {
    args.get("paths")
        .and_then(Value::as_array)
        .ok_or("缺少参数 paths")?
        .iter()
        .map(|v| check_path(v.as_str().unwrap_or_default()))
        .collect()
}

/// Runs one request. Returns the extra header fields and the body.
pub fn handle(host: &dyn FsHost, op: &str, args: &Value) -> Result<(Value, Vec<u8>), String> {
    let none = || Ok((json!({}), Vec::new()));
    match op {
        "roots" => Ok((host.roots(), Vec::new())),
        "list" => {
            let dir = check_path(arg(args, "path")?)?;
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let mut all: Vec<Value> = std::fs::read_dir(&dir)
                .map_err(|e| io_err(&e))?
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    // Follow links so linked folders behave like folders.
                    let m = std::fs::metadata(e.path()).or_else(|_| e.metadata()).ok()?;
                    Some(entry(&name, &m))
                })
                .collect();
            // A stable order makes paging consistent between requests.
            all.sort_by(|a, b| a["n"].as_str().cmp(&b["n"].as_str()));
            let page: Vec<Value> = all.iter().skip(offset).take(PAGE).cloned().collect();
            let mut h = json!({});
            if offset + page.len() < all.len() {
                h["next"] = json!(offset + page.len());
            }
            Ok((h, serde_json::to_vec(&page).unwrap()))
        }
        "walk" => {
            let root = check_path(arg(args, "path")?)?;
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let mut all = Vec::new();
            walk(&root, &root, &mut all)?;
            let page: Vec<Value> = all.iter().skip(offset).take(PAGE).cloned().collect();
            let mut h = json!({});
            if offset + page.len() < all.len() {
                h["next"] = json!(offset + page.len());
            }
            Ok((h, serde_json::to_vec(&page).unwrap()))
        }
        "stat" => {
            let p = check_path(arg(args, "path")?)?;
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned());
            match std::fs::metadata(&p) {
                Ok(m) => Ok((json!({"e": entry(&name, &m)}), Vec::new())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((json!({"e": null}), Vec::new())),
                Err(e) => Err(io_err(&e)),
            }
        }
        "mkdir" => {
            let p = check_path(arg(args, "path")?)?;
            if p.exists() {
                return Err("已存在同名文件或文件夹".into());
            }
            std::fs::create_dir_all(&p).map_err(|e| io_err(&e))?;
            none()
        }
        "create" => {
            let p = check_path(arg(args, "path")?)?;
            std::fs::OpenOptions::new().write(true).create_new(true).open(&p).map_err(|e| io_err(&e))?;
            none()
        }
        "rename" => {
            let from = check_path(arg(args, "path")?)?;
            let to = check_path(arg(args, "to")?)?;
            if to.exists() && !same_file_name_change(&from, &to) {
                return Err("已存在同名文件或文件夹".into());
            }
            std::fs::rename(&from, &to).map_err(|e| io_err(&e))?;
            none()
        }
        "delete" => {
            let ps = paths(args)?;
            for p in &ps {
                if p.parent().is_none() {
                    return Err("不能删除整个磁盘".into());
                }
            }
            host.delete(&ps)?;
            none()
        }
        "copy" | "move" => {
            let dir = check_path(arg(args, "dir")?)?;
            for src in paths(args)? {
                paste(&src, &dir, op == "move")?;
            }
            none()
        }
        "setmtime" => {
            let p = check_path(arg(args, "path")?)?;
            let m = args.get("m").and_then(Value::as_u64).ok_or("缺少参数 m")?;
            let f = std::fs::File::options().write(true).open(&p).map_err(|e| io_err(&e))?;
            f.set_modified(UNIX_EPOCH + std::time::Duration::from_millis(m)).map_err(|e| io_err(&e))?;
            none()
        }
        "thumb" => {
            let p = check_path(arg(args, "path")?)?;
            let px = args.get("px").and_then(Value::as_u64).unwrap_or(128).clamp(32, 512) as u32;
            match host.thumb(&p, px) {
                Some(jpeg) => Ok((json!({"mime": "image/jpeg"}), jpeg)),
                None => Err("没有预览".into()),
            }
        }
        _ => Err(format!("不支持的操作：{op}")),
    }
}

/// Windows renames "a.txt" to "A.txt" in place; that is not a name clash.
fn same_file_name_change(from: &Path, to: &Path) -> bool {
    from.parent() == to.parent()
        && from.file_name().map(|n| n.to_string_lossy().to_lowercase()) == to.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Value>) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| io_err(&e))?.flatten() {
        if out.len() >= WALK_MAX {
            return Err("文件太多（超过 5 万个）".into());
        }
        let Ok(m) = e.metadata() else { continue };
        let rel = e.path().strip_prefix(root).unwrap_or(&e.path()).to_string_lossy().replace('\\', "/");
        out.push(json!({"p": rel, "d": m.is_dir(), "s": if m.is_dir() { 0 } else { m.len() }}));
        if m.is_dir() {
            walk(root, &e.path(), out)?;
        }
    }
    Ok(())
}

/// Copies or moves `src` into `dir`, appending " (n)" to taken names.
fn paste(src: &Path, dir: &Path, mv: bool) -> Result<(), String> {
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("路径无效")?;
    if dir.starts_with(src) {
        return Err("不能放进它自己里面".into());
    }
    if mv && src.parent() == Some(dir) {
        return Ok(());
    }
    let dst = unique_path(dir, &name);
    if mv && std::fs::rename(src, &dst).is_ok() {
        return Ok(());
    }
    copy_rec(src, &dst).map_err(|e| format!("{name}：{}", io_err(&e)))?;
    if mv {
        let r = if src.is_dir() { std::fs::remove_dir_all(src) } else { std::fs::remove_file(src) };
        r.map_err(|e| io_err(&e))?;
    }
    Ok(())
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

pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_checked() {
        assert!(check_path(r"\\server\share\x").is_err());
        assert!(check_path(r"\\?\C:\x").is_err());
        assert!(check_path(r"\\.\PhysicalDrive0").is_err());
        assert!(check_path("relative\\x").is_err());
        #[cfg(windows)]
        {
            assert!(check_path(r"C:\Users\a").is_ok());
            assert!(check_path(r"C:\Users\..\Windows").is_err());
        }
    }
}
