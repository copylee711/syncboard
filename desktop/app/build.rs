//! Bundles adb (from the Android SDK) into the exe, so users don't need it installed.
//! Optional: without it the app falls back to a system adb.

use std::path::{Path, PathBuf};

fn platform_tools() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("SYNCBOARD_ADB_DIR") {
        c.push(d.into());
    }
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(d) = std::env::var(var) {
            c.push(Path::new(&d).join("platform-tools"));
        }
    }
    if let Ok(d) = std::env::var("LOCALAPPDATA") {
        c.push(Path::new(&d).join("Android").join("Sdk").join("platform-tools"));
    }
    c.into_iter().find(|d| d.join("adb.exe").is_file())
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(embedded_adb)");
    println!("cargo::rerun-if-env-changed=SYNCBOARD_ADB_DIR");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    if let Some(dir) = platform_tools() {
        // Tiny archive: [u16 name len][name][u32 data len][data]..., then deflate.
        let mut raw = Vec::new();
        for name in ["adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll", "NOTICE.txt"] {
            let p = dir.join(name);
            println!("cargo::rerun-if-changed={}", p.display());
            let Ok(data) = std::fs::read(&p) else { continue };
            raw.extend_from_slice(&(name.len() as u16).to_le_bytes());
            raw.extend_from_slice(name.as_bytes());
            raw.extend_from_slice(&(data.len() as u32).to_le_bytes());
            raw.extend_from_slice(&data);
        }
        let packed = miniz_oxide::deflate::compress_to_vec(&raw, 8);
        std::fs::write(out.join("adb.bin"), packed).unwrap();
        println!("cargo::rustc-cfg=embedded_adb");
    } else {
        println!("cargo::warning=platform-tools not found; adb will not be bundled");
    }

    tauri_build::build()
}
