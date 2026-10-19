//! Windows parts of letting a paired device browse this PC (protocol §8, PC side):
//! drives with their free space, the Recycle Bin and Explorer's thumbnails.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use syncboard_core::pcfs::FsHost;

pub struct WinFsHost;

fn disk_space(root: &str) -> (u64, u64) {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
    let (mut free, mut total) = (0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    if ok == 0 { (0, 0) } else { (total, free) }
}

impl FsHost for WinFsHost {
    fn roots(&self) -> Value {
        let mut volumes = Vec::new();
        for (name, id) in [
            ("桌面", &windows_sys::Win32::UI::Shell::FOLDERID_Desktop),
            ("下载", &windows_sys::Win32::UI::Shell::FOLDERID_Downloads),
            ("文档", &windows_sys::Win32::UI::Shell::FOLDERID_Documents),
            ("图片", &windows_sys::Win32::UI::Shell::FOLDERID_Pictures),
            ("视频", &windows_sys::Win32::UI::Shell::FOLDERID_Videos),
        ] {
            if let Some(p) = crate::files::known_folder(id) {
                volumes.push(json!({"name": name, "path": p, "total": 0, "free": 0, "kind": "place"}));
            }
        }
        for c in b'A'..=b'Z' {
            let root = format!("{}:\\", c as char);
            if !Path::new(&root).exists() {
                continue;
            }
            let (total, free) = disk_space(&root);
            volumes.push(json!({"name": format!("{}:", c as char), "path": root, "total": total, "free": free, "kind": "drive"}));
        }
        let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
        json!({"home": home, "volumes": volumes})
    }

    fn delete(&self, paths: &[PathBuf]) -> Result<(), String> {
        let list: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        crate::files::recycle(&list)
    }

    fn thumb(&self, path: &Path, px: u32) -> Option<Vec<u8>> {
        let rgb = shell_thumbnail(path, px)?;
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode_image(&rgb).ok()?;
        Some(out)
    }
}

/// Explorer's own thumbnail (photos, videos, PDFs... whatever has a thumbnail handler);
/// None for files that would only get an icon.
fn shell_thumbnail(path: &Path, px: u32) -> Option<image::RgbImage> {
    use std::ffi::c_void;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_THUMBNAILONLY};

    unsafe {
        // Runs on a blocking-pool thread; joining the MTA is idempotent.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
        let size = SIZE { cx: px as i32, cy: px as i32 };
        let hbmp = factory.GetImage(size, SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK).ok()?;
        let mut bm = BITMAP::default();
        let got = GetObjectW(hbmp.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void));
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        let mut pixels = vec![0u8; (w.max(0) * h.max(0) * 4) as usize];
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = CreateCompatibleDC(None);
        let rows = if got > 0 && w > 0 && h > 0 {
            GetDIBits(dc, hbmp, 0, h as u32, Some(pixels.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS)
        } else {
            0
        };
        let _ = DeleteDC(dc);
        let _ = DeleteObject(hbmp.into());
        if rows <= 0 {
            return None;
        }
        // BGRA -> RGB
        let rgb: Vec<u8> = pixels.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
        let img = image::RgbImage::from_raw(w as u32, h as u32, rgb)?;
        let (w, h) = (w as u32, h as u32);
        if w <= px && h <= px {
            return Some(img);
        }
        let scale = px as f32 / w.max(h) as f32;
        Some(image::imageops::thumbnail(&img, ((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1)))
    }
}
