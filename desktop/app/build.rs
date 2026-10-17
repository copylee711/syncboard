//! Bundles adb (from the Android SDK) and the phone APK (from android/) into the exe,
//! so users need neither installed. Both are optional: without them the app falls
//! back to a system adb and skips installing the APK.

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
    println!("cargo::rustc-check-cfg=cfg(embedded_apk)");
    println!("cargo::rerun-if-env-changed=SYNCBOARD_ADB_DIR");
    println!("cargo::rerun-if-env-changed=SYNCBOARD_APK");
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

    // Embedding the APK is for local testing only; release builds set
    // SYNCBOARD_NO_APK=1 and download it from GitHub Releases at runtime.
    println!("cargo::rerun-if-env-changed=SYNCBOARD_NO_APK");
    let apk = std::env::var("SYNCBOARD_APK").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from("../../android/app/build/outputs/apk/release/app-release.apk")
    });
    let apk = if std::env::var_os("SYNCBOARD_NO_APK").is_some() { PathBuf::new() } else { apk };
    println!("cargo::rerun-if-changed={}", apk.display());
    if apk.is_file() {
        std::fs::copy(&apk, out.join("app.apk")).unwrap();
        // versionCode from the Gradle output metadata next to the APK.
        let meta = std::fs::read_to_string(apk.with_file_name("output-metadata.json")).unwrap_or_default();
        let code = meta
            .split("\"versionCode\":")
            .nth(1)
            .and_then(|s| s.trim_start().split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1);
        std::fs::write(out.join("apk_version.txt"), code.to_string()).unwrap();
        println!("cargo::rustc-cfg=embedded_apk");
    } else {
        // Normal for release builds: the APK is downloaded at runtime.
    }

    tauri_build::build()
}
