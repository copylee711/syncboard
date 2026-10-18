//! Windows clipboard: event-driven watcher (AddClipboardFormatListener) and writer.
//!
//! A hidden message-only window lives on its own thread. Clipboard changes arrive as
//! WM_CLIPBOARDUPDATE; writes are posted to the same thread so the window owns the data.

use crate::engine::ClipContent;
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::mpsc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_WRITE: u32 = WM_APP + 1;
const READ_TIMER: usize = 1;
/// Delay before reading after a change, so the writer can finish (and bursts coalesce).
const READ_DELAY_MS: u32 = 80;

/// Callback for local clipboard changes. It runs on the watcher thread, so keep it short.
pub type OnChange = Box<dyn Fn(ClipContent) + Send>;

struct State {
    on_change: OnChange,
    writes: mpsc::Receiver<ClipContent>,
    /// Sequence number produced by our own last write; changes up to it are ignored.
    own_seq: u32,
    fmt_png: u32,
    fmt_exclude: u32,
    fmt_viewer_ignore: u32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub struct ClipboardWriter {
    hwnd: isize,
    tx: mpsc::Sender<ClipContent>,
}

impl ClipboardWriter {
    pub fn write(&self, content: ClipContent) {
        if self.tx.send(content).is_ok() {
            unsafe { PostMessageW(self.hwnd as HWND, WM_WRITE, 0, 0) };
        }
    }
}

pub fn start(on_change: OnChange) -> ClipboardWriter {
    let (hwnd_tx, hwnd_rx) = mpsc::channel::<isize>();
    let (tx, rx) = mpsc::channel::<ClipContent>();
    std::thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || unsafe {
            let class: Vec<u16> = wide("SyncBoardClipboard");
            let hinst = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0,
                HWND_MESSAGE, std::ptr::null_mut(), hinst, std::ptr::null(),
            );
            STATE.with(|s| {
                *s.borrow_mut() = Some(State {
                    on_change,
                    writes: rx,
                    own_seq: 0,
                    fmt_png: register("PNG"),
                    fmt_exclude: register("ExcludeClipboardContentFromMonitorProcessing"),
                    fmt_viewer_ignore: register("Clipboard Viewer Ignore"),
                })
            });
            AddClipboardFormatListener(hwnd);
            let _ = hwnd_tx.send(hwnd as isize);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .expect("spawn clipboard thread");
    let hwnd = hwnd_rx.recv().expect("clipboard window");
    ClipboardWriter { hwnd, tx }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            SetTimer(hwnd, READ_TIMER, READ_DELAY_MS, None);
            0
        }
        WM_TIMER if wp == READ_TIMER => {
            KillTimer(hwnd, READ_TIMER);
            STATE.with(|s| {
                if let Some(st) = s.borrow().as_ref() {
                    let seq = GetClipboardSequenceNumber();
                    if seq != 0 && seq <= st.own_seq {
                        return;
                    }
                    if let Some(c) = read(hwnd, st) {
                        (st.on_change)(c);
                    }
                }
            });
            0
        }
        WM_WRITE => {
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    while let Ok(c) = st.writes.try_recv() {
                        if write(hwnd, st, &c) {
                            st.own_seq = GetClipboardSequenceNumber();
                        }
                    }
                }
            });
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn register(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(wide(name).as_ptr()) }
}

/// Opens the clipboard, retrying while another process holds it (clipboard managers and
/// Windows' own clipboard history can hold it for a few hundred milliseconds).
fn open(hwnd: HWND) -> bool {
    for _ in 0..50 {
        if unsafe { OpenClipboard(hwnd) } != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

unsafe fn global_bytes(h: *mut c_void) -> Option<Vec<u8>> {
    if h.is_null() {
        return None;
    }
    let p = GlobalLock(h) as *const u8;
    if p.is_null() {
        return None;
    }
    let v = std::slice::from_raw_parts(p, GlobalSize(h)).to_vec();
    GlobalUnlock(h);
    Some(v)
}

unsafe fn read(hwnd: HWND, st: &State) -> Option<ClipContent> {
    // Password managers mark secrets with these formats; respect that.
    if IsClipboardFormatAvailable(st.fmt_exclude) != 0 || IsClipboardFormatAvailable(st.fmt_viewer_ignore) != 0 {
        return None;
    }
    if !open(hwnd) {
        return None;
    }
    let result = (|| {
        if IsClipboardFormatAvailable(CF_UNICODETEXT as u32) != 0 {
            let bytes = global_bytes(GetClipboardData(CF_UNICODETEXT as u32))?;
            let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..end]);
            return (!text.is_empty()).then_some(ClipContent::Text(text));
        }
        if IsClipboardFormatAvailable(st.fmt_png) != 0 {
            if let Some(png) = global_bytes(GetClipboardData(st.fmt_png)) {
                return Some(ClipContent::Image(png));
            }
        }
        if IsClipboardFormatAvailable(CF_DIB as u32) != 0 {
            let dib = global_bytes(GetClipboardData(CF_DIB as u32))?;
            let png = dib_to_png(&dib);
            #[cfg(debug_assertions)]
            if png.is_none() {
                eprintln!("[clipboard] could not convert DIB ({} bytes, header {:?})", dib.len(), &dib[..dib.len().min(40)]);
            }
            return png.map(ClipContent::Image);
        }
        None
    })();
    CloseClipboard();
    result
}

unsafe fn alloc(data: &[u8]) -> *mut c_void {
    let h = GlobalAlloc(GMEM_MOVEABLE, data.len());
    if h.is_null() {
        return h;
    }
    let p = GlobalLock(h) as *mut u8;
    std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
    GlobalUnlock(h);
    h
}

unsafe fn write(hwnd: HWND, st: &State, c: &ClipContent) -> bool {
    let formats: Vec<(u32, Vec<u8>)> = match c {
        ClipContent::Text(t) => {
            let bytes: Vec<u8> = wide(t).iter().flat_map(|u| u.to_le_bytes()).collect();
            vec![(CF_UNICODETEXT as u32, bytes)]
        }
        ClipContent::Image(png) => {
            let Some(dib) = png_to_dib(png) else { return false };
            vec![(st.fmt_png, png.clone()), (CF_DIB as u32, dib)]
        }
    };
    if !open(hwnd) {
        return false;
    }
    EmptyClipboard();
    let mut ok = true;
    for (fmt, data) in formats {
        let h = alloc(&data);
        if h.is_null() || SetClipboardData(fmt, h).is_null() {
            ok = false;
        }
    }
    CloseClipboard();
    ok
}

/// CF_DIB is a BMP file without its 14-byte file header; add one and decode.
pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes([dib[o], dib[o + 1], dib[o + 2], dib[o + 3]]);
    let hdr = u32_at(0) as usize;
    let bitcount = u16::from_le_bytes([dib[14], dib[15]]);
    let compression = u32_at(16);
    let clr_used = u32_at(32) as usize;
    let colors = if bitcount <= 8 { if clr_used == 0 { 1usize << bitcount } else { clr_used } } else { clr_used };
    let masks = if compression == 3 && hdr == 40 { 12 } else { 0 };
    let off = 14 + hdr + masks + colors * 4;
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&(off as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    let img = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Largest image Windows clipboard history (Win+V) keeps: measured on Windows 11 26300, it
/// drops images over about 8 MiB as 32-bit pixels (945×2100 kept, 1008×2240 dropped),
/// whatever formats they are offered in.
pub const HISTORY_MAX_PIXELS: u64 = 2_000_000;

/// Scales a PNG down to at most `max_pixels`, keeping its aspect; None if it already fits
/// (or can't be decoded).
pub fn fit_pixels(png: &[u8], max_pixels: u64) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    let (w, h) = (img.width() as u64, img.height() as u64);
    if w * h <= max_pixels {
        return None;
    }
    let k = (max_pixels as f64 / (w * h) as f64).sqrt();
    let (nw, nh) = (((w as f64 * k) as u32).max(1), ((h as f64 * k) as u32).max(1));
    let small = img.resize_exact(nw, nh, image::imageops::FilterType::Triangle);
    let mut out = std::io::Cursor::new(Vec::new());
    small.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// PNG → 32bpp bottom-up BI_RGB DIB.
pub fn png_to_dib(png: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut out = Vec::with_capacity(40 + (w * h * 4) as usize);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(w * h * 4).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = img.get_pixel(x, y).0;
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_pixels_shrinks_only_large_images() {
        let png = |w, h| {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgba8(w, h).write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        assert!(fit_pixels(&png(100, 100), 20_000).is_none());
        let small = fit_pixels(&png(1260, 2800), HISTORY_MAX_PIXELS).unwrap();
        let img = image::load_from_memory(&small).unwrap();
        assert!(img.width() as u64 * img.height() as u64 <= HISTORY_MAX_PIXELS);
        assert_eq!((img.width() as f64 / img.height() as f64 * 100.0).round(), 45.0);
    }

    #[test]
    fn dib_roundtrip() {
        let mut img = image::RgbaImage::new(3, 2);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([x as u8 * 80, y as u8 * 100, 7, 255]);
        }
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let dib = png_to_dib(png.get_ref()).unwrap();
        let back = dib_to_png(&dib).unwrap();
        let decoded = image::load_from_memory(&back).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (3, 2));
        assert_eq!(decoded.get_pixel(2, 1).0[..3], img.get_pixel(2, 1).0[..3]);
    }
}
