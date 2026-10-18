#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adb;
mod hotspot;
mod system;
mod update;

use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use syncboard_core::sync::ClipSync;
use syncboard_core::{Engine, Event, PORT};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent, Wry};
use tauri_plugin_dialog::DialogExt;

const MAIN: &str = "main";
/// The 添加手机 QR stays valid this long. The UI only replaces it well before that, because
/// replacing it abandons a scan in progress (the phone keeps waiting on the old code).
const WIFI_QR_TTL: Duration = Duration::from_secs(30 * 60);

struct App {
    engine: Arc<Engine>,
    sync: Arc<ClipSync>,
    /// Recent transfers and pending pair requests, kept so a recreated window can show them.
    transfers: Mutex<Vec<Value>>,
    pair_requests: Mutex<Vec<Value>>,
    pause_item: Mutex<Option<CheckMenuItem<Wry>>>,
    /// Cancels the running phone-setup flow (USB watch or wireless pairing).
    setup_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// Whether SyncBoard turned the hotspot on (so it turns it off again).
    hotspot_ours: AtomicBool,
    /// The wireless pairing in progress: its QR service name, and where the phone app
    /// reported the pairing port (see `Event::AdbPairHint`).
    wifi_pairing: Mutex<Option<(String, Arc<Mutex<Option<adb::PairHint>>>)>>,
}

type St<'a> = tauri::State<'a, App>;

// ---------- window (light mode) ----------

/// Creates the window on demand. Closing it destroys the webview, leaving only the Rust
/// core and the tray icon in memory ("light mode").
fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    if let Err(e) = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("SyncBoard")
        .inner_size(420.0, 660.0)
        .min_inner_size(360.0, 480.0)
        .decorations(false)
        .shadow(true)
        .center()
        .build()
    {
        eprintln!("[app] cannot create window: {e}");
    }
}

fn notify_ui(app: &AppHandle, name: &str, payload: Value) {
    if app.get_webview_window(MAIN).is_some() {
        let _ = app.emit(name, payload);
    }
}

// ---------- commands ----------

fn state_json(st: &App) -> Value {
    let connected = st.engine.connected();
    let peers: Vec<Value> = st
        .engine
        .peers()
        .into_iter()
        .map(|p| {
            let c = connected.iter().find(|(id, ..)| *id == p.id);
            json!({"id": p.id, "name": p.name, "platform": p.platform,
                   "connected": c.is_some(), "addr": c.map(|c| c.2.clone()).or(p.last_addr),
                   "keyMismatch": st.engine.key_mismatch(&p.id)})
        })
        .collect();
    let settings = st.engine.with_store(|s| s.config.settings.clone());
    let download_dir = st.engine.with_store(|s| s.download_dir());
    let adb = adb::find(settings.adb_path.as_deref());
    json!({
        "name": st.engine.device_name(),
        "version": update::VERSION,
        "port": st.engine.port(),
        "ips": syncboard_core::engine::local_ipv4s(),
        "peers": peers,
        "settings": settings,
        "downloadDir": download_dir,
        "paused": st.sync.paused(),
        "autostart": system::autostart_enabled(),
        "sendto": system::sendto_enabled(),
        "adb": adb.map(|p| p.display().to_string()),
        "transfers": *st.transfers.lock().unwrap(),
        "pairRequests": *st.pair_requests.lock().unwrap(),
    })
}

#[tauri::command]
fn get_state(st: St) -> Value {
    state_json(&st)
}

fn qr_svg(text: &str) -> String {
    qrcode::QrCode::new(text.as_bytes())
        .map(|q| {
            q.render::<qrcode::render::svg::Color>()
                .min_dimensions(220, 220)
                .quiet_zone(false)
                .dark_color(qrcode::render::svg::Color("#111827"))
                .light_color(qrcode::render::svg::Color("#ffffff"))
                .build()
        })
        .unwrap_or_default()
}

#[tauri::command]
fn pairing_qr(st: St) -> Value {
    let uri = st.engine.pairing_uri();
    json!({"uri": uri, "svg": qr_svg(&uri)})
}

#[tauri::command]
fn apk_qr() -> Value {
    let url = update::release_page();
    json!({"url": url, "apk": update::apk_name(), "svg": qr_svg(&url)})
}

#[tauri::command]
fn open_releases() {
    system::open_url(&format!("{}/releases", update::REPO));
}

// ---------- phone setup (USB / wireless / hotspot) ----------

fn adb_path(st: &App) -> Result<PathBuf, String> {
    let custom = st.engine.with_store(|s| s.config.settings.adb_path.clone());
    adb::find(custom.as_deref()).ok_or_else(|| "找不到 adb".to_string())
}

/// Starts a new setup flow, cancelling any previous one.
fn new_setup(st: &App) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(old) = st.setup_cancel.lock().unwrap().replace(cancel.clone()) {
        old.store(true, Ordering::Relaxed);
    }
    cancel
}

fn setup_event(app: &AppHandle, stage: &str, text: &str) {
    notify_ui(app, "setup", json!({"stage": stage, "text": text}));
}

#[tauri::command]
fn setup_cancel(st: St) {
    if let Some(c) = st.setup_cancel.lock().unwrap().take() {
        c.store(true, Ordering::Relaxed);
    }
}

/// Watches `adb devices` until a phone is plugged in and authorized, then sets it up.
#[tauri::command]
fn setup_usb(app: AppHandle, st: St) -> Result<(), String> {
    let adb = adb_path(&st)?;
    let cancel = new_setup(&st);
    std::thread::spawn(move || {
        let mut last = String::new();
        while !cancel.load(Ordering::Relaxed) {
            let devs = adb::devices_with_state(&adb);
            let mut usb: Vec<_> = devs.iter().filter(|(s, _)| !s.contains(':') && !s.contains("._adb")).collect();
            // A real phone wins over an emulator that happens to be running.
            usb.sort_by_key(|(s, _)| s.starts_with("emulator-"));
            let text = if let Some((serial, _)) = usb.iter().find(|(_, s)| *s == adb::DevState::Ready) {
                let uri = app.state::<App>().engine.pairing_uri();
                let progress = |t: &str| setup_event(&app, "progress", t);
                match adb::setup_device(&adb, serial, &uri, true, &progress) {
                    Ok(()) => setup_event(&app, "waiting", "手机服务已启动，正在配对…"),
                    Err(e) => setup_event(&app, "error", &e),
                }
                return;
            } else if usb.iter().any(|(_, s)| *s == adb::DevState::Unauthorized) {
                "请在手机上点「允许 USB 调试」"
            } else {
                "等待手机通过 USB 连接…"
            };
            if last != text {
                last = text.to_string();
                setup_event(&app, "progress", text);
            }
            std::thread::sleep(Duration::from_millis(1200));
        }
    });
    Ok(())
}

/// Shows an Android "pair with QR code" code and sets the phone up once it is scanned.
#[tauri::command]
fn setup_wifi(app: AppHandle, st: St) -> Result<Value, String> {
    let adb = adb_path(&st)?;
    let cancel = new_setup(&st);
    let pairing = adb::WifiPairing::new(&syncboard_core::engine::local_ipv4s(), st.engine.new_pairing_token(WIFI_QR_TTL));
    let svg = qr_svg(&pairing.qr_text());
    let hint = Arc::new(Mutex::new(None));
    *st.wifi_pairing.lock().unwrap() = Some((pairing.name.clone(), hint.clone()));
    std::thread::spawn(move || {
        let progress = |t: &str| setup_event(&app, "progress", t);
        let r = adb::wifi_pair_and_connect(&adb, &pairing, &hint, &cancel, &progress).and_then(|serial| {
            let uri = app.state::<App>().engine.pairing_uri();
            adb::setup_device(&adb, &serial, &uri, false, &progress)
        });
        match r {
            Ok(()) => setup_event(&app, "waiting", "手机服务已启动，正在配对…"),
            Err(_) if cancel.load(Ordering::Relaxed) => {}
            Err(e) => setup_event(&app, "error", &e),
        }
    });
    Ok(json!({"svg": svg}))
}

#[tauri::command]
async fn hotspot_start(st: St<'_>) -> Result<Value, String> {
    let was_on = tauri::async_runtime::spawn_blocking(|| hotspot::info().map(|h| h.on))
        .await
        .map_err(|e| e.to_string())??;
    let h = tauri::async_runtime::spawn_blocking(hotspot::start).await.map_err(|e| e.to_string())??;
    if !was_on {
        st.hotspot_ours.store(true, Ordering::Relaxed);
    }
    Ok(json!({"ssid": h.ssid, "svg": qr_svg(&h.qr_text())}))
}

#[tauri::command]
async fn hotspot_stop(st: St<'_>) -> Result<(), String> {
    if st.hotspot_ours.swap(false, Ordering::Relaxed) {
        tauri::async_runtime::spawn_blocking(hotspot::stop).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn stop_phone_service(st: St<'_>, id: String) -> Result<(), String> {
    st.engine.stop_peer_service(&id).await.map_err(|e| e.to_string())
}


#[tauri::command]
fn answer_pair(st: St, req: u64, accept: bool) {
    st.pair_requests.lock().unwrap().retain(|r| r["req"] != req);
    st.engine.answer_pair(req, accept);
}

#[tauri::command]
async fn check_update() -> Result<Value, String> {
    let latest = tauri::async_runtime::spawn_blocking(update::latest_version).await.map_err(|e| e.to_string())??;
    let newer = update::version_tuple(&latest) > update::version_tuple(update::VERSION);
    Ok(json!({"current": update::VERSION, "latest": latest, "newer": newer}))
}

/// Downloads `version`, swaps it in for this exe and restarts into it.
#[tauri::command]
async fn install_update(app: AppHandle, version: String) -> Result<(), String> {
    let ui = app.clone();
    let exe = tauri::async_runtime::spawn_blocking(move || {
        update::install(&version, |done, total| notify_ui(&ui, "updateProgress", json!({"done": done, "total": total})))
    })
    .await
    .map_err(|e| e.to_string())??;
    update::relaunch_after_exit(&exe);
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn unpair(st: St, id: String) -> Result<(), String> {
    st.engine.unpair(&id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_paths(st: St<'_>, peer: Option<String>, paths: Vec<String>) -> Result<(), String> {
    send_files(&st.engine, peer, paths.into_iter().map(PathBuf::from).collect()).await
}

#[tauri::command]
fn pick_and_send(app: AppHandle, peer: Option<String>) {
    pick_files(app, peer);
}

#[tauri::command]
fn set_settings(app: AppHandle, st: St, patch: Value) -> Result<(), String> {
    st.engine.with_store(|s| {
        let mut cur = serde_json::to_value(&s.config.settings).unwrap();
        if let (Some(c), Some(p)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                c.insert(k.clone(), v.clone());
            }
        }
        s.config.settings = serde_json::from_value(cur).map_err(|e| e.to_string())?;
        s.save().map_err(|e| e.to_string())
    })?;
    notify_ui(&app, "state", state_json(&st));
    Ok(())
}

#[tauri::command]
fn set_paused(app: AppHandle, st: St, minutes: Option<u64>, paused: bool) {
    apply_pause(&app, &st, paused, minutes);
}

#[tauri::command]
fn set_autostart(on: bool) -> Result<(), String> {
    system::set_autostart(on).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_sendto(on: bool) -> Result<(), String> {
    system::set_sendto(on)
}

#[tauri::command]
fn open_downloads(st: St) {
    system::open_path(&st.engine.with_store(|s| s.download_dir()));
}

#[tauri::command]
async fn adb_start(st: St<'_>) -> Result<String, String> {
    let custom = st.engine.with_store(|s| s.config.settings.adb_path.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let adb = adb::find(custom.as_deref()).ok_or("没有找到 adb。请安装 Android Platform Tools，或在设置里指定 adb.exe。")?;
        adb::start_all(&adb)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------- helpers ----------

async fn send_files(engine: &Arc<Engine>, peer: Option<String>, paths: Vec<PathBuf>) -> Result<(), String> {
    let targets: Vec<String> = match peer {
        Some(p) => vec![p],
        None => engine.connected().into_iter().map(|(id, ..)| id).collect(),
    };
    if targets.is_empty() {
        return Err("手机未连接".into());
    }
    for t in &targets {
        for p in &paths {
            engine.send_file(t, p).await.map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn pick_files(app: AppHandle, peer: Option<String>) {
    let a = app.clone();
    app.dialog().file().set_title("选择要发送到手机的文件").pick_files(move |files| {
        let Some(files) = files else { return };
        let paths: Vec<PathBuf> = files.into_iter().filter_map(|f| f.into_path().ok()).collect();
        let engine = a.state::<App>().engine.clone();
        let a2 = a.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = send_files(&engine, peer, paths).await {
                notify_ui(&a2, "toast", json!({"kind": "error", "text": e}));
            }
        });
    });
}

fn apply_pause(app: &AppHandle, st: &App, paused: bool, minutes: Option<u64>) {
    if paused {
        st.sync.pause(minutes.map(|m| Duration::from_secs(m * 60)));
    } else {
        st.sync.resume();
    }
    if let Some(item) = st.pause_item.lock().unwrap().as_ref() {
        let _ = item.set_checked(st.sync.paused());
    }
    update_tray(app);
    notify_ui(app, "state", state_json(st));
    // Re-check the tray state when a timed pause runs out.
    if let (true, Some(m)) = (paused, minutes) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(m * 60 + 1)).await;
            let st = app.state::<App>();
            if let Some(item) = st.pause_item.lock().unwrap().as_ref() {
                let _ = item.set_checked(st.sync.paused());
            }
            update_tray(&app);
            notify_ui(&app, "state", state_json(&st));
        });
    }
}

fn update_tray(app: &AppHandle) {
    let st = app.state::<App>();
    let n = st.engine.connected().len();
    let tip = if st.sync.paused() {
        "SyncBoard · 已暂停同步".to_string()
    } else if n > 0 {
        format!("SyncBoard · 已连接 {n} 台设备")
    } else {
        "SyncBoard · 等待手机连接".to_string()
    };
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tip));
    }
}

fn on_engine_event(app: &AppHandle, ev: Event) {
    let st = app.state::<App>();
    let v = serde_json::to_value(&ev).unwrap_or(Value::Null);
    match &ev {
        Event::AdbPairHint { name, ip, pair_port, connect_port } => {
            eprintln!("[pair] {name} at {ip}:{pair_port} (connect {connect_port:?})");
            if let Some((current, hint)) = st.wifi_pairing.lock().unwrap().as_ref() {
                if current == name {
                    *hint.lock().unwrap() = Some(adb::PairHint {
                        pair_addr: format!("{ip}:{pair_port}"),
                        connect_addr: connect_port.map(|p| format!("{ip}:{p}")),
                    });
                }
            }
        }
        Event::AdbWake { addr, .. } => {
            // A paired phone rebooted: reconnect over wireless debugging and restart it.
            let (addr, custom) = (addr.clone(), st.engine.with_store(|s| s.config.settings.adb_path.clone()));
            std::thread::spawn(move || {
                eprintln!("[wake] {addr}");
                let Some(adb) = adb::find(custom.as_deref()) else {
                    eprintln!("[wake] no adb");
                    return;
                };
                match adb::connect_wireless(&adb, &addr) {
                    Ok(serial) => {
                        let r = adb::start_daemon(&adb, &serial);
                        eprintln!("[wake] start daemon on {serial}: {r:?}");
                        adb::disconnect(&adb, &serial);
                    }
                    Err(e) => eprintln!("[wake] adb connect {addr} failed: {e}"),
                }
            });
        }
        Event::Paired { name, .. } => {
            if let Some(c) = st.setup_cancel.lock().unwrap().take() {
                c.store(true, Ordering::Relaxed);
            }
            setup_event(app, "done", &format!("已与「{name}」配对"));
        }
        Event::PairRequest { .. } => {
            st.pair_requests.lock().unwrap().push(v.clone());
            show_window(app);
        }
        Event::FileProgress { id, .. } | Event::FileDone { id, .. } | Event::FileFailed { id, .. } => {
            let mut t = st.transfers.lock().unwrap();
            t.retain(|x| x["id"] != *id);
            t.insert(0, v.clone());
            t.truncate(20);
        }
        _ => {}
    }
    if matches!(ev, Event::PeerConnected { .. } | Event::PeerDisconnected { .. } | Event::Paired { .. }) {
        update_tray(app);
    }
    notify_ui(app, "engine", v);
}

/// `--send <files>` from the Explorer "Send to" menu.
fn handle_args(app: &AppHandle, args: &[String], first_launch: bool) {
    eprintln!("[app] args {args:?} first={first_launch}");
    if let Some(i) = args.iter().position(|a| a == "--send") {
        let paths: Vec<PathBuf> = args[i + 1..].iter().map(PathBuf::from).collect();
        if paths.is_empty() {
            return;
        }
        let engine = app.state::<App>().engine.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            // On a cold start, give the phone a moment to reconnect.
            for _ in 0..if first_launch { 30 } else { 1 } {
                if !engine.connected().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            if let Err(e) = send_files(&engine, None, paths).await {
                show_window(&app);
                notify_ui(&app, "toast", json!({"kind": "error", "text": e}));
            }
        });
    } else if !args.iter().any(|a| a == "--hidden") {
        show_window(app);
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let h = app.handle();
    let open = MenuItem::with_id(h, "open", "打开面板", true, None::<&str>)?;
    let paused = CheckMenuItem::with_id(h, "pause", "暂停同步", true, false, None::<&str>)?;
    let p5 = MenuItem::with_id(h, "pause5", "暂停 5 分钟", true, None::<&str>)?;
    let p30 = MenuItem::with_id(h, "pause30", "暂停 30 分钟", true, None::<&str>)?;
    let pause_menu = Submenu::with_items(h, "暂停", true, &[&paused, &p5, &p30])?;
    let send = MenuItem::with_id(h, "send", "发送文件到手机…", true, None::<&str>)?;
    let adb = MenuItem::with_id(h, "adb", "启动手机服务 (ADB)", true, None::<&str>)?;
    let folder = MenuItem::with_id(h, "folder", "打开接收文件夹", true, None::<&str>)?;
    let quit = MenuItem::with_id(h, "quit", "退出", true, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(h);
    let menu = Menu::with_items(h, &[&open, &sep()?, &pause_menu, &send, &folder, &adb, &sep()?, &quit])?;
    *app.state::<App>().pause_item.lock().unwrap() = Some(paused);

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("icon"))
        .tooltip("SyncBoard")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| {
            let st = app.state::<App>();
            match e.id().as_ref() {
                "open" => show_window(app),
                "pause" => {
                    let now = !st.sync.paused();
                    apply_pause(app, &st, now, None);
                }
                "pause5" => apply_pause(app, &st, true, Some(5)),
                "pause30" => apply_pause(app, &st, true, Some(30)),
                "send" => pick_files(app.clone(), None),
                "folder" => system::open_path(&st.engine.with_store(|s| s.download_dir())),
                "adb" => {
                    let custom = st.engine.with_store(|s| s.config.settings.adb_path.clone());
                    let app = app.clone();
                    std::thread::spawn(move || {
                        let r = adb::find(custom.as_deref())
                            .ok_or_else(|| "没有找到 adb".to_string())
                            .and_then(|a| adb::start_all(&a));
                        let (kind, text) = match r {
                            Ok(t) => ("ok", t),
                            Err(t) => ("error", t),
                        };
                        app.dialog().message(text).title(if kind == "ok" { "SyncBoard" } else { "启动失败" }).show(|_| {});
                    });
                }
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// While a paired phone is offline, polls `adb devices` and restarts the daemon on
/// phones that get plugged in. Idle (no adb at all) when every phone is connected.
fn spawn_auto_adb(app: AppHandle) {
    std::thread::spawn(move || {
        let mut started: Vec<String> = Vec::new();
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let st = app.state::<App>();
            let s = st.engine.with_store(|s| s.config.settings.clone());
            let peers = st.engine.peers();
            let all_online = st.engine.connected().len() >= peers.len();
            if !s.auto_adb || peers.is_empty() || all_online || st.setup_cancel.lock().unwrap().is_some() {
                started.clear();
                continue;
            }
            let Some(adb) = adb::find(s.adb_path.as_deref()) else { continue };
            let devs = adb::devices(&adb);
            started.retain(|d| devs.contains(d));
            for d in devs {
                if !started.contains(&d) && adb::start_daemon(&adb, &d).is_ok() {
                    started.push(d);
                }
            }
        }
    });
}

fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into()).join("SyncBoard")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    update::cleanup();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| handle_args(app, &argv, false)))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_state, pairing_qr, answer_pair, unpair, send_paths, pick_and_send, set_settings,
            set_paused, set_autostart, set_sendto, open_downloads, adb_start, setup_cancel, setup_usb,
            setup_wifi, hotspot_start, hotspot_stop, stop_phone_service, check_update, install_update, apk_qr, open_releases
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let (engine, sync) = tauri::async_runtime::block_on(async {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let engine = Engine::start(&config_dir(), PORT, tx).await?;
                let h = handle.clone();
                let sync = syncboard_core::sync::start(engine.clone(), rx, move |ev| on_engine_event(&h, ev));
                Ok::<_, anyhow_like::Error>((engine, sync))
            })
            .map_err(|e| format!("无法启动：{e}（端口 {PORT} 是否被占用？）"))?;
            app.manage(App {
                engine,
                sync,
                transfers: Mutex::new(Vec::new()),
                pair_requests: Mutex::new(Vec::new()),
                pause_item: Mutex::new(None),
                setup_cancel: Mutex::new(None),
                hotspot_ours: AtomicBool::new(false),
                wifi_pairing: Mutex::new(None),
            });
            build_tray(app)?;
            update_tray(app.handle());
            spawn_auto_adb(app.handle().clone());
            handle_args(app.handle(), &args, true);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build app");

    app.run(|app, event| match event {
        // Keep running in the tray after the last window closes.
        RunEvent::ExitRequested { api, code: None, .. } => api.prevent_exit(),
        RunEvent::WindowEvent { label, event: WindowEvent::Destroyed, .. } if label == MAIN => {
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(2));
                // Light mode: WebView2 keeps its browser process alive after the last
                // webview closes; end it (it is recreated with the next window).
                if app.get_webview_window(MAIN).is_none() {
                    system::kill_child_processes("msedgewebview2.exe");
                }
                system::trim_working_set();
            });
        }
        _ => {}
    });
}

mod anyhow_like {
    pub type Error = Box<dyn std::error::Error + Send + Sync>;
}
