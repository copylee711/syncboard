//! The PC side of the protocol: listens for phones, handles pairing, sessions,
//! clipboard messages and file transfers. UI and OS clipboard live outside; they
//! talk to the engine through `Event`s and the public methods.

use crate::crypto::{self, Ecdh, RecordCipher};
use crate::frame::{read_plain, write_plain, Message, SecureReader, SecureWriter, CHUNK_SIZE};
use crate::pcfs::{self, FsHost};
use crate::store::{Peer, Store};
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, oneshot};

pub const PORT: u16 = 53317;
pub const MAX_TEXT: usize = 1024 * 1024;
pub const MAX_IMAGE: u64 = 20 * 1024 * 1024;
const TOKEN_TTL: Duration = Duration::from_secs(300);
const PAIR_TIMEOUT: Duration = Duration::from_secs(120);
const PING_EVERY: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq)]
pub enum ClipContent {
    Text(String),
    /// PNG bytes.
    Image(Vec<u8>),
}

impl ClipContent {
    pub fn hash(&self) -> String {
        match self {
            ClipContent::Text(t) => crypto::sha256_hex(t.as_bytes()),
            ClipContent::Image(b) => crypto::sha256_hex(b),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    PeerConnected { id: String, name: String, addr: String },
    PeerDisconnected { id: String },
    PairRequest { req: u64, name: String, platform: String, code: String },
    Paired { id: String, name: String },
    PairFailed { reason: String },
    #[serde(skip)]
    Clip { from: String, content: ClipContent },
    FileProgress { id: String, name: String, done: u64, total: u64, incoming: bool },
    FileDone { id: String, name: String, path: Option<String>, incoming: bool },
    FileFailed { id: String, name: String, reason: String },
    /// A paired phone rebooted and turned on wireless debugging; the app should
    /// `adb connect` to `addr` and start the daemon.
    AdbWake { id: String, name: String, addr: String },
    /// A paired phone connected but its session key doesn't match ours (one side
    /// was re-paired or reset); the user has to pair it again.
    KeyMismatch { id: String, name: String },
    /// A phone that scanned our wireless-debugging QR reports its pairing (and connect)
    /// port, for networks where mDNS doesn't reach the PC (protocol §7). `name` is the
    /// pairing service name from the QR, so the app can tell whether it is the one it shows.
    AdbPairHint { name: String, ip: String, pair_port: u16, connect_port: Option<u16> },
    /// The phone shared a link with 在电脑上打开 (protocol §9).
    OpenUrl { from: String, url: String },
    /// A text message arrived on the phone (protocol §10): only its verification code
    /// (`body` None), or the whole message when the phone's 新短信提醒 is on.
    Sms { peer: String, from: Option<String>, body: Option<String>, code: Option<String>, source: Option<String> },
    /// A notification from an app on the phone (protocol §10, 手机通知同步).
    PhoneNotification { peer: String, pkg: String, app: String, title: String, text: String },
}

struct Session {
    gen: u64,
    name: String,
    addr: SocketAddr,
    tx: mpsc::Sender<Message>,
    /// A file-manager app (TwinFile), not the SyncBoard phone service: no clipboard.
    files_only: bool,
}

enum Inbound {
    Image { hasher: Sha256, buf: Vec<u8>, size: u64 },
    File { hasher: Sha256, file: tokio::fs::File, path: PathBuf, name: String, size: u64, done: u64, push: Option<Push> },
}

/// A device's upload into a folder it picked on this PC (`file.offer` with `dir`).
struct Push {
    /// Modification time to give the file, ms.
    mtime: Option<u64>,
}

struct Outbound {
    peer: String,
    path: PathBuf,
    name: String,
    size: u64,
}

/// A phone file the file manager asked for (protocol §8 `pull`), saved to `dest`.
struct Pull {
    peer: String,
    dest: PathBuf,
    name: String,
}

/// Result of a transfer started by the file manager: where a pulled file landed.
pub type TransferResult = std::result::Result<Option<PathBuf>, String>;

/// A reply to a file-manager request: its header and body.
pub struct FsReply {
    pub header: Value,
    pub body: Vec<u8>,
}

pub struct Engine {
    store: Mutex<Store>,
    events: mpsc::UnboundedSender<Event>,
    sessions: Mutex<HashMap<String, Session>>,
    /// Pairing tokens and when each expires.
    tokens: Mutex<Vec<([u8; 32], Instant)>>,
    pending_pairs: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    outbound: Mutex<HashMap<String, Outbound>>,
    /// Peers whose last handshake failed with a key mismatch.
    key_mismatch: Mutex<HashSet<String>>,
    /// File-manager requests waiting for `fs.res`, by request id.
    fs_waiting: Mutex<HashMap<String, oneshot::Sender<FsReply>>>,
    pulls: Mutex<HashMap<String, Pull>>,
    /// Transfers the file manager's queue awaits, by transfer id.
    done_waiting: Mutex<HashMap<String, oneshot::Sender<TransferResult>>>,
    /// Transfers started through [`Self::push_file`] / [`Self::pull_file`] (the file manager's).
    managed: Mutex<std::collections::HashSet<String>>,
    /// Transfers the user cancelled; checked while streaming either way.
    cancelled: Mutex<HashSet<String>>,
    /// Files a device is pulling from this PC (protocol §8, PC side), by request id.
    serving: Mutex<HashSet<String>>,
    fs_host: Mutex<Arc<dyn FsHost>>,
    next: AtomicU64,
    port: u16,
}

impl Engine {
    /// Binds TCP + UDP on `port` (0 = any, for tests) and starts serving.
    pub async fn start(
        config_dir: &Path,
        port: u16,
        events: mpsc::UnboundedSender<Event>,
    ) -> Result<Arc<Engine>> {
        let store = Store::open(config_dir)?;
        let listener = TcpListener::bind(("0.0.0.0", port)).await.context("bind tcp")?;
        let port = listener.local_addr()?.port();
        let engine = Arc::new(Engine {
            store: Mutex::new(store),
            events,
            sessions: Mutex::new(HashMap::new()),
            tokens: Mutex::new(Vec::new()),
            pending_pairs: Mutex::new(HashMap::new()),
            outbound: Mutex::new(HashMap::new()),
            key_mismatch: Mutex::new(HashSet::new()),
            fs_waiting: Mutex::new(HashMap::new()),
            pulls: Mutex::new(HashMap::new()),
            done_waiting: Mutex::new(HashMap::new()),
            managed: Mutex::new(Default::default()),
            cancelled: Mutex::new(HashSet::new()),
            serving: Mutex::new(HashSet::new()),
            fs_host: Mutex::new(Arc::new(pcfs::PlainHost)),
            next: AtomicU64::new(1),
            port,
        });

        let e = engine.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        let e = e.clone();
                        tokio::spawn(async move {
                            let _ = stream.set_nodelay(true);
                            if let Err(err) = e.handle_conn(stream, addr).await {
                                eprintln!("[engine] {addr}: {err:#}");
                            }
                        });
                    }
                    Err(err) => eprintln!("[engine] accept: {err}"),
                }
            }
        });

        match UdpSocket::bind(("0.0.0.0", port)).await {
            Ok(sock) => {
                let e = engine.clone();
                tokio::spawn(async move { e.discovery_loop(sock).await });
            }
            Err(err) => eprintln!("[engine] discovery disabled: {err}"),
        }
        Ok(engine)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn device_id(&self) -> String {
        self.store.lock().unwrap().config.device_id.clone()
    }

    pub fn device_name(&self) -> String {
        self.store.lock().unwrap().config.name.clone()
    }

    /// Installs the platform parts (drives, Recycle Bin, thumbnails) used when a paired
    /// device browses this PC.
    pub fn set_fs_host(&self, host: Arc<dyn FsHost>) {
        *self.fs_host.lock().unwrap() = host;
    }

    fn remote_browse_allowed(&self) -> bool {
        self.store.lock().unwrap().config.settings.allow_remote_browse
    }

    pub fn with_store<T>(&self, f: impl FnOnce(&mut Store) -> T) -> T {
        f(&mut self.store.lock().unwrap())
    }

    pub fn peers(&self) -> Vec<Peer> {
        self.store.lock().unwrap().config.peers.clone()
    }

    pub fn connected(&self) -> Vec<(String, String, String)> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(id, s)| (id.clone(), s.name.clone(), s.addr.ip().to_string()))
            .collect()
    }

    /// True when `id`'s last connection attempt failed because the pairing keys differ.
    pub fn key_mismatch(&self, id: &str) -> bool {
        self.key_mismatch.lock().unwrap().contains(id)
    }

    /// Creates a fresh single-use pairing token (base64url), valid for `ttl`.
    pub fn new_pairing_token(&self, ttl: Duration) -> String {
        let token = crypto::random_bytes::<32>();
        let mut t = self.tokens.lock().unwrap();
        t.retain(|(_, until)| Instant::now() < *until);
        t.push((token, Instant::now() + ttl));
        B64URL.encode(token)
    }

    /// Creates a fresh single-use pairing token and returns the QR payload.
    pub fn pairing_uri(&self) -> String {
        let token = self.new_pairing_token(TOKEN_TTL);
        let hosts = local_ipv4s().join(",");
        let (id, name) = {
            let s = self.store.lock().unwrap();
            (s.config.device_id.clone(), s.config.name.clone())
        };
        format!(
            "syncboard://pair?v=1&h={}&p={}&id={}&n={}&t={}",
            hosts,
            self.port,
            id,
            url_encode(&name),
            token
        )
    }

    pub fn answer_pair(&self, req: u64, accept: bool) {
        if let Some(tx) = self.pending_pairs.lock().unwrap().remove(&req) {
            let _ = tx.send(accept);
        }
    }

    /// Sets the user's 备注 for a paired device (empty clears it).
    pub fn set_alias(&self, id: &str, alias: &str) -> Result<()> {
        let alias = alias.trim();
        let mut s = self.store.lock().unwrap();
        let Some(p) = s.config.peers.iter_mut().find(|p| p.id == id) else { bail!("没有这台设备") };
        p.alias = (!alias.is_empty()).then(|| alias.to_string());
        let name = p.display_name().to_string();
        s.save()?;
        drop(s);
        if let Some(sess) = self.sessions.lock().unwrap().get_mut(id) {
            sess.name = name;
        }
        Ok(())
    }

    pub fn unpair(&self, id: &str) -> Result<()> {
        if let Some(s) = self.sessions.lock().unwrap().remove(id) {
            // Lets the phone forget this PC too; it then closes the connection.
            let _ = s.tx.try_send(Message::new(json!({"t":"unpaired"})));
        }
        self.key_mismatch.lock().unwrap().remove(id);
        let mut s = self.store.lock().unwrap();
        s.config.peers.retain(|p| p.id != id);
        s.save()
    }

    /// Sends clipboard content to every connected peer.
    pub fn send_clip(self: &Arc<Self>, content: ClipContent) {
        let targets: Vec<mpsc::Sender<Message>> =
            self.sessions.lock().unwrap().values().filter(|s| !s.files_only).map(|s| s.tx.clone()).collect();
        for tx in targets {
            let content = content.clone();
            tokio::spawn(async move {
                let id = crypto::random_id();
                let hash = content.hash();
                match content {
                    ClipContent::Text(text) => {
                        let m = Message::with_body(
                            json!({"t":"clip.text","id":id,"hash":hash}),
                            text.into_bytes(),
                        );
                        let _ = tx.send(m).await;
                    }
                    ClipContent::Image(png) => {
                        let head = json!({"t":"clip.image","id":id,"hash":hash,"mime":"image/png","size":png.len()});
                        if tx.send(Message::new(head)).await.is_err() {
                            return;
                        }
                        for (seq, c) in png.chunks(CHUNK_SIZE).enumerate() {
                            let m = Message::with_body(json!({"t":"chunk","id":id,"seq":seq}), c.to_vec());
                            if tx.send(m).await.is_err() {
                                return;
                            }
                        }
                        let _ = tx.send(Message::new(json!({"t":"end","id":id,"sha256":hash}))).await;
                    }
                }
            });
        }
    }

    /// Asks a connected phone to shut its daemon down.
    pub async fn stop_peer_service(&self, peer: &str) -> Result<()> {
        let tx = self.session_tx(peer).ok_or_else(|| anyhow!("peer not connected"))?;
        tx.send(Message::new(json!({"t":"service.stop"}))).await.map_err(|_| anyhow!("peer disconnected"))
    }

    /// Offers a file to a connected peer; streaming starts once the peer accepts.
    pub async fn send_file(self: &Arc<Self>, peer: &str, path: &Path) -> Result<String> {
        self.offer_file(peer, path, None).await
    }

    /// Sends a file into the phone folder `dir` (protocol §8); the receiver resolves when it is done.
    pub async fn push_file(self: &Arc<Self>, peer: &str, path: &Path, dir: &str) -> (String, oneshot::Receiver<TransferResult>) {
        let (tx, rx) = oneshot::channel();
        match self.offer_file(peer, path, Some(dir)).await {
            Ok(id) => {
                self.manage(&id);
                self.done_waiting.lock().unwrap().insert(id.clone(), tx);
                (id, rx)
            }
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
                (String::new(), rx)
            }
        }
    }

    /// Fetches the phone file `path` into `dest`; the receiver resolves when it is done.
    pub async fn pull_file(&self, peer: &str, path: &str, dest: &Path) -> (String, oneshot::Receiver<TransferResult>) {
        let (tx, rx) = oneshot::channel();
        let id = crypto::random_id();
        let Some(stx) = self.session_tx(peer) else {
            let _ = tx.send(Err("手机未连接".into()));
            return (id, rx);
        };
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        self.manage(&id);
        self.pulls.lock().unwrap().insert(id.clone(), Pull { peer: peer.into(), dest: dest.to_path_buf(), name });
        self.done_waiting.lock().unwrap().insert(id.clone(), tx);
        if stx.send(Message::new(json!({"t":"fs","rid":id,"op":"pull","path":path}))).await.is_err() {
            self.pulls.lock().unwrap().remove(&id);
            self.finish(&id, Err("手机未连接".into()));
        }
        (id, rx)
    }

    fn manage(&self, id: &str) {
        let mut m = self.managed.lock().unwrap();
        if m.len() > 4096 {
            m.clear();
        }
        m.insert(id.to_string());
    }

    /// Whether `id` was started by [`Self::push_file`] or [`Self::pull_file`] rather than
    /// sent by the user (those stay out of the main window's transfer list).
    pub fn is_managed(&self, id: &str) -> bool {
        self.managed.lock().unwrap().contains(id)
    }

    /// Stops a transfer started with [`Self::push_file`] or [`Self::pull_file`].
    pub fn cancel_transfer(&self, id: &str) {
        self.cancelled.lock().unwrap().insert(id.to_string());
        let peer = self.outbound.lock().unwrap().remove(id).map(|o| o.peer);
        let peer = peer.or_else(|| self.pulls.lock().unwrap().remove(id).map(|p| p.peer));
        let peer = peer.or_else(|| self.connected().first().map(|c| c.0.clone()));
        if let Some(tx) = peer.and_then(|p| self.session_tx(&p)) {
            let _ = tx.try_send(Message::new(json!({"t":"file.cancel","id":id})));
        }
        self.finish(id, Err("已取消".into()));
    }

    fn finish(&self, id: &str, r: TransferResult) {
        if let Some(tx) = self.done_waiting.lock().unwrap().remove(id) {
            let _ = tx.send(r);
        }
    }

    /// A file-manager request to a phone (protocol §8): `op` with `args` merged into the header.
    pub async fn fs(&self, peer: &str, op: &str, mut args: Value, timeout: Duration) -> Result<FsReply> {
        let tx = self.session_tx(peer).ok_or_else(|| anyhow!("手机未连接"))?;
        let rid = crypto::random_id();
        if !args.is_object() {
            args = json!({});
        }
        args["t"] = json!("fs");
        args["rid"] = json!(rid);
        args["op"] = json!(op);
        let (wtx, wrx) = oneshot::channel();
        self.fs_waiting.lock().unwrap().insert(rid.clone(), wtx);
        if tx.send(Message::new(args)).await.is_err() {
            self.fs_waiting.lock().unwrap().remove(&rid);
            bail!("手机未连接");
        }
        let r = tokio::time::timeout(timeout, wrx).await;
        self.fs_waiting.lock().unwrap().remove(&rid);
        match r {
            Ok(Ok(reply)) if reply.header.get("ok").and_then(Value::as_bool) == Some(true) => Ok(reply),
            Ok(Ok(reply)) => bail!("{}", reply.header.get("error").and_then(Value::as_str).unwrap_or("手机端操作失败")),
            Ok(Err(_)) => bail!("连接断开"),
            Err(_) => bail!("手机没有响应（手机 App 可能需要更新）"),
        }
    }

    async fn offer_file(self: &Arc<Self>, peer: &str, path: &Path, dir: Option<&str>) -> Result<String> {
        let meta = tokio::fs::metadata(path).await?;
        if !meta.is_file() {
            bail!("not a file");
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tx = self.session_tx(peer).ok_or_else(|| anyhow!("peer not connected"))?;
        let id = crypto::random_id();
        self.outbound.lock().unwrap().insert(
            id.clone(),
            Outbound { peer: peer.to_string(), path: path.to_path_buf(), name: name.clone(), size: meta.len() },
        );
        let mut offer = json!({"t":"file.offer","id":id,"name":name,"size":meta.len()});
        if let Some(d) = dir {
            offer["dir"] = json!(d);
        }
        tx.send(Message::new(offer))
            .await
            .map_err(|_| anyhow!("peer disconnected"))?;
        Ok(id)
    }

    fn session_tx(&self, peer: &str) -> Option<mpsc::Sender<Message>> {
        self.sessions.lock().unwrap().get(peer).map(|s| s.tx.clone())
    }

    fn emit(&self, e: Event) {
        match &e {
            Event::FileDone { id, path, .. } => self.finish(id, Ok(path.as_ref().map(PathBuf::from))),
            Event::FileFailed { id, reason, .. } => self.finish(id, Err(reason.clone())),
            _ => {}
        }
        let _ = self.events.send(e);
    }

    async fn discovery_loop(self: Arc<Self>, sock: UdpSocket) {
        let mut buf = [0u8; 2048];
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf).await else { continue };
            let Ok(v) = serde_json::from_slice::<Value>(&buf[..n]) else { continue };
            if v.get("t").and_then(Value::as_str) != Some("discover") {
                continue;
            }
            let reply = json!({"t":"here","v":1,"id":self.device_id(),"name":self.device_name(),"port":self.port});
            let _ = sock.send_to(&serde_json::to_vec(&reply).unwrap(), from).await;
        }
    }

    async fn handle_conn(self: &Arc<Self>, mut stream: TcpStream, addr: SocketAddr) -> Result<()> {
        let first = tokio::time::timeout(Duration::from_secs(10), read_plain(&mut stream)).await??;
        match first.get("t").and_then(Value::as_str) {
            Some("pair") => self.handle_pair(stream, first).await,
            Some("hello") => self.handle_session(stream, addr, first).await,
            Some("adb_wake") => self.handle_wake(stream, addr, first).await,
            Some("adb_pair") => {
                let port = |k: &str| first.get(k).and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok());
                let name = first.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let Some(pair_port) = port("pair_port") else { bail!("adb_pair without pair_port") };
                self.emit(Event::AdbPairHint { name, ip: addr.ip().to_string(), pair_port, connect_port: port("connect_port") });
                write_plain(&mut stream, &json!({"t":"ok"})).await
            }
            other => bail!("unexpected first frame {other:?}"),
        }
    }

    /// `{"t":"adb_wake","id","port","ts","mac"}`, mac = HMAC(psk, "wake" ‖ port ‖ ts).
    /// Answers `wake_ok` or `wake_rejected` so the phone can stop waiting, and forget
    /// this PC when it no longer knows the phone.
    async fn handle_wake(&self, mut stream: TcpStream, addr: SocketAddr, v: Value) -> Result<()> {
        let id = v.get("id").and_then(Value::as_str).unwrap_or_default();
        let Some(peer) = self.store.lock().unwrap().peer(id).cloned() else {
            write_plain(&mut stream, &json!({"t":"wake_rejected","code":"unknown_device"})).await?;
            bail!("wake from unknown device {id}");
        };
        if let Err(e) = verify_wake(&peer.psk, &v) {
            write_plain(&mut stream, &json!({"t":"wake_rejected","code":"bad_request"})).await?;
            return Err(e);
        }
        let port = v.get("port").and_then(Value::as_u64).unwrap_or(0);
        self.emit(Event::AdbWake { id: peer.id.clone(), name: peer.display_name().to_string(), addr: format!("{}:{port}", addr.ip()) });
        write_plain(&mut stream, &json!({"t":"wake_ok"})).await?;
        Ok(())
    }

    async fn handle_pair(self: &Arc<Self>, mut stream: TcpStream, req: Value) -> Result<()> {
        let field = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
        let peer_id = field("id").ok_or_else(|| anyhow!("missing id"))?;
        let peer_name = field("name").unwrap_or_else(|| "Phone".into());
        let platform = field("platform").unwrap_or_else(|| "android".into());
        let mode = field("mode").unwrap_or_default();
        let app = field("app");
        let pub_c = B64.decode(field("pub").unwrap_or_default())?;
        let nonce_c = B64.decode(field("nonce").unwrap_or_default())?;
        if nonce_c.len() != 32 {
            bail!("bad nonce");
        }

        let ecdh = Ecdh::new();
        let nonce_s = crypto::random_bytes::<32>();
        let shared = ecdh.agree(&pub_c)?;
        let (my_id, my_name) = (self.device_id(), self.device_name());
        write_plain(
            &mut stream,
            &json!({"t":"pair","v":1,"id":my_id,"name":my_name,"platform":"windows",
                    "pub":B64.encode(ecdh.public),"nonce":B64.encode(nonce_s)}),
        )
        .await?;
        let th = crypto::transcript(&pub_c, &ecdh.public, &nonce_c, &nonce_s);

        let psk = match mode.as_str() {
            "qr" => {
                let proof = read_plain(&mut stream).await?;
                let mac = B64.decode(proof.get("mac").and_then(Value::as_str).unwrap_or_default())?;
                let token = {
                    let mut tokens = self.tokens.lock().unwrap();
                    tokens.retain(|(_, until)| Instant::now() < *until);
                    let pos = tokens
                        .iter()
                        .position(|(t, _)| crypto::ct_eq(&crypto::hmac(t, &[b"client", &th]), &mac));
                    pos.map(|i| tokens.remove(i).0)
                };
                let Some(token) = token else {
                    write_plain(&mut stream, &json!({"t":"pair_rejected"})).await?;
                    self.emit(Event::PairFailed { reason: "二维码无效或已过期".into() });
                    bail!("bad pairing token");
                };
                let mac_s = crypto::hmac(&token, &[b"server", &th]);
                write_plain(&mut stream, &json!({"t":"pair_ok","mac":B64.encode(mac_s)})).await?;
                crypto::derive_psk(&shared, Some(&token), &th)
            }
            "code" => {
                let req_no = self.next.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = oneshot::channel();
                self.pending_pairs.lock().unwrap().insert(req_no, tx);
                self.emit(Event::PairRequest {
                    req: req_no,
                    name: peer_name.clone(),
                    platform: platform.clone(),
                    code: crypto::pairing_code(&th),
                });
                let accepted = matches!(tokio::time::timeout(PAIR_TIMEOUT, rx).await, Ok(Ok(true)));
                self.pending_pairs.lock().unwrap().remove(&req_no);
                if !accepted {
                    write_plain(&mut stream, &json!({"t":"pair_rejected"})).await?;
                    bail!("pairing rejected");
                }
                write_plain(&mut stream, &json!({"t":"pair_ok"})).await?;
                crypto::derive_psk(&shared, None, &th)
            }
            _ => bail!("unknown pairing mode {mode}"),
        };

        {
            let mut s = self.store.lock().unwrap();
            s.upsert_peer(Peer {
                id: peer_id.clone(),
                name: peer_name.clone(),
                platform,
                psk: B64.encode(psk),
                last_addr: None,
                alias: None,
                app,
            });
            s.save()?;
        }
        self.key_mismatch.lock().unwrap().remove(&peer_id);
        let _ = stream.shutdown().await;
        self.emit(Event::Paired { id: peer_id, name: peer_name });
        Ok(())
    }

    async fn handle_session(self: &Arc<Self>, mut stream: TcpStream, addr: SocketAddr, hello: Value) -> Result<()> {
        let peer_id = hello.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let nonce_c = B64.decode(hello.get("nonce").and_then(Value::as_str).unwrap_or_default())?;
        let Some(peer) = self.store.lock().unwrap().peer(&peer_id).cloned() else {
            write_plain(&mut stream, &json!({"t":"error","code":"unknown_device"})).await?;
            bail!("unknown device {peer_id}");
        };
        if nonce_c.len() != 32 {
            bail!("bad nonce");
        }
        let nonce_s = crypto::random_bytes::<32>();
        write_plain(&mut stream, &json!({"t":"hello","v":1,"id":self.device_id(),"nonce":B64.encode(nonce_s)}))
            .await?;
        let psk = B64.decode(&peer.psk)?;
        let (c2s, s2c) = crypto::session_keys(&psk, &nonce_c, &nonce_s);
        let (r, w) = stream.into_split();
        let mut reader = SecureReader::new(r, RecordCipher::new(&c2s));
        let mut writer = SecureWriter::new(w, RecordCipher::new(&s2c));

        writer
            .write(&Message::new(json!({"t":"auth","name":self.device_name(),"platform":"windows"})))
            .await?;
        let auth = match tokio::time::timeout(Duration::from_secs(10), reader.read()).await? {
            Ok(m) => m,
            Err(e) if e.is::<crypto::DecryptError>() => {
                // The phone's record doesn't open with our key: it holds a different
                // psk for us. It sees the same on its side and stops retrying.
                if self.key_mismatch.lock().unwrap().insert(peer_id.clone()) {
                    self.emit(Event::KeyMismatch { id: peer_id.clone(), name: peer.display_name().to_string() });
                }
                bail!("key mismatch with {peer_id}");
            }
            Err(e) => return Err(e),
        };
        if auth.kind() != "auth" {
            bail!("expected auth");
        }
        self.key_mismatch.lock().unwrap().remove(&peer_id);

        let (tx, mut rx) = mpsc::channel::<Message>(32);
        let gen = self.next.fetch_add(1, Ordering::Relaxed);
        self.sessions.lock().unwrap().insert(
            peer_id.clone(),
            Session { gen, name: peer.display_name().to_string(), addr, tx: tx.clone(), files_only: peer.app.is_some() },
        );
        {
            let mut s = self.store.lock().unwrap();
            if let Some(p) = s.config.peers.iter_mut().find(|p| p.id == peer_id) {
                p.last_addr = Some(addr.ip().to_string());
            }
            let _ = s.save();
        }
        self.emit(Event::PeerConnected { id: peer_id.clone(), name: peer.display_name().to_string(), addr: addr.ip().to_string() });

        let writer_task = tokio::spawn(async move {
            let mut ping = tokio::time::interval(PING_EVERY);
            ping.tick().await;
            loop {
                tokio::select! {
                    m = rx.recv() => match m {
                        Some(m) => if writer.write(&m).await.is_err() { break },
                        None => break,
                    },
                    _ = ping.tick() => if writer.write(&Message::new(json!({"t":"ping"}))).await.is_err() { break },
                }
            }
        });

        let mut inbound: HashMap<String, Inbound> = HashMap::new();
        let mut seen: VecDeque<String> = VecDeque::new();
        let result: Result<()> = async {
            loop {
                let m = tokio::time::timeout(IDLE_TIMEOUT, reader.read()).await.map_err(|_| anyhow!("idle timeout"))??;
                // A newer connection from the same peer replaced this one.
                if self.sessions.lock().unwrap().get(&peer_id).map(|s| s.gen) != Some(gen) {
                    return Ok(());
                }
                self.on_message(&peer_id, &tx, m, &mut inbound, &mut seen).await?;
            }
        }
        .await;

        writer_task.abort();
        for (id, ib) in inbound {
            if let Inbound::File { path, name, .. } = ib {
                let _ = tokio::fs::remove_file(&path).await;
                self.emit(Event::FileFailed { id, name, reason: "连接断开".into() });
            }
        }
        let removed = {
            let mut sessions = self.sessions.lock().unwrap();
            if sessions.get(&peer_id).map(|s| s.gen) == Some(gen) {
                sessions.remove(&peer_id);
                true
            } else {
                false
            }
        };
        if removed {
            self.emit(Event::PeerDisconnected { id: peer_id });
        }
        result
    }

    async fn on_message(
        self: &Arc<Self>,
        peer: &str,
        tx: &mpsc::Sender<Message>,
        m: Message,
        inbound: &mut HashMap<String, Inbound>,
        seen: &mut VecDeque<String>,
    ) -> Result<()> {
        let id = m.str("id").unwrap_or_default().to_string();
        match m.kind() {
            "ping" => {
                let _ = tx.send(Message::new(json!({"t":"pong"}))).await;
            }
            "pong" | "auth" => {}
            "unpaired" => {
                // The phone removed this PC.
                self.unpair(peer)?;
                self.emit(Event::PeerDisconnected { id: peer.into() });
                bail!("peer unpaired");
            }
            "notif" => {
                let s = |k: &str| m.str(k).unwrap_or_default().chars().take(2000).collect::<String>();
                self.emit(Event::PhoneNotification { peer: peer.into(), pkg: s("pkg"), app: s("app"), title: s("title"), text: s("text") });
            }
            "sms.code" | "sms.new" => {
                let s = |k: &str| m.str(k).filter(|v| !v.is_empty()).map(|v| v.chars().take(4000).collect::<String>());
                let code = s("code").filter(|c| c.len() <= 16);
                if m.kind() == "sms.new" || code.is_some() {
                    self.emit(Event::Sms { peer: peer.into(), from: s("from"), body: s("body"), code, source: s("source") });
                }
            }
            "open.url" => {
                let url = m.str("url").unwrap_or_default();
                if (url.starts_with("https://") || url.starts_with("http://")) && url.len() < 8192 {
                    self.emit(Event::OpenUrl { from: peer.into(), url: url.into() });
                }
            }
            "clip.text" => {
                if remember(seen, &id) && m.body.len() <= MAX_TEXT {
                    let text = String::from_utf8_lossy(&m.body).into_owned();
                    self.emit(Event::Clip { from: peer.into(), content: ClipContent::Text(text) });
                }
            }
            "clip.image" => {
                let size = m.u64("size").unwrap_or(0);
                if remember(seen, &id) && size <= MAX_IMAGE {
                    inbound.insert(id, Inbound::Image { hasher: Sha256::new(), buf: Vec::with_capacity(size as usize), size });
                }
            }
            "file.offer" if m.str("dir").is_some() => {
                // An upload into a folder the device picked while browsing this PC.
                let name = sanitize_name(m.str("name").unwrap_or("file"));
                let size = m.u64("size").unwrap_or(0);
                let dir = match pcfs::check_path(m.str("dir").unwrap_or_default()) {
                    Ok(_) if !self.remote_browse_allowed() => Err("这台电脑没有开启「允许已配对设备浏览文件」".to_string()),
                    Ok(d) if !d.is_dir() => Err("目标文件夹不存在".to_string()),
                    other => other,
                };
                let file = match &dir {
                    Ok(d) => {
                        let path = unique_path(d, &name);
                        tokio::fs::File::create(&path).await.map(|f| (f, path)).map_err(|e| pcfs::io_err(&e))
                    }
                    Err(e) => Err(e.clone()),
                };
                match file {
                    Ok((file, path)) => {
                        self.manage(&id);
                        let push = Some(Push { mtime: m.u64("mtime") });
                        inbound.insert(id.clone(), Inbound::File { hasher: Sha256::new(), file, path, name, size, done: 0, push });
                        let _ = tx.send(Message::new(json!({"t":"file.accept","id":id}))).await;
                    }
                    Err(e) => {
                        let _ = tx.send(Message::new(json!({"t":"file.reject","id":id,"error":e}))).await;
                    }
                }
            }
            "file.offer" => {
                let name = sanitize_name(m.str("name").unwrap_or("file"));
                let size = m.u64("size").unwrap_or(0);
                let dir = self.store.lock().unwrap().download_dir();
                tokio::fs::create_dir_all(&dir).await?;
                let path = unique_path(&dir, &name);
                match tokio::fs::File::create(&path).await {
                    Ok(file) => {
                        inbound.insert(id.clone(), Inbound::File { hasher: Sha256::new(), file, path, name, size, done: 0, push: None });
                        let _ = tx.send(Message::new(json!({"t":"file.accept","id":id}))).await;
                    }
                    Err(_) => {
                        let _ = tx.send(Message::new(json!({"t":"file.reject","id":id}))).await;
                    }
                }
            }
            "fs" => {
                let rid = m.str("rid").unwrap_or_default().to_string();
                let op = m.str("op").unwrap_or_default().to_string();
                if !self.remote_browse_allowed() {
                    let err = "这台电脑没有开启「允许已配对设备浏览文件」，请在电脑上的 SyncBoard 设置里打开";
                    let _ = tx.send(Message::new(json!({"t":"fs.res","rid":rid,"ok":false,"error":err}))).await;
                } else if op == "pull" {
                    let e = self.clone();
                    let tx = tx.clone();
                    let path = m.str("path").unwrap_or_default().to_string();
                    let peer = peer.to_string();
                    tokio::spawn(async move { e.serve_pull(peer, rid, path, tx).await });
                } else {
                    let host = self.fs_host.lock().unwrap().clone();
                    let args = m.header.clone();
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let r = tokio::task::spawn_blocking(move || pcfs::handle(&*host, &op, &args)).await;
                        let reply = match r {
                            Ok(Ok((mut h, body))) => {
                                h["t"] = json!("fs.res");
                                h["rid"] = json!(rid);
                                h["ok"] = json!(true);
                                Message::with_body(h, body)
                            }
                            Ok(Err(e)) => Message::new(json!({"t":"fs.res","rid":rid,"ok":false,"error":e})),
                            Err(_) => Message::new(json!({"t":"fs.res","rid":rid,"ok":false,"error":"电脑端出错"})),
                        };
                        let _ = tx.send(reply).await;
                    });
                }
            }
            "fs.res" => {
                let rid = m.str("rid").unwrap_or_default().to_string();
                let pull = self.pulls.lock().unwrap().remove(&rid);
                if let Some(p) = pull {
                    // A pull starts with fs.res (size, or an error), then chunks under the same id.
                    if m.header.get("ok").and_then(Value::as_bool) != Some(true) || p.peer != peer {
                        let reason = m.str("error").unwrap_or("手机端读取失败").to_string();
                        self.emit(Event::FileFailed { id: rid, name: p.name, reason });
                    } else {
                        let size = m.u64("size").unwrap_or(0);
                        if let Some(parent) = p.dest.parent() {
                            let _ = tokio::fs::create_dir_all(parent).await;
                        }
                        match tokio::fs::File::create(&p.dest).await {
                            Ok(file) => {
                                inbound.insert(rid, Inbound::File { hasher: Sha256::new(), file, path: p.dest, name: p.name, size, done: 0, push: None });
                            }
                            Err(e) => {
                                let _ = tx.send(Message::new(json!({"t":"file.cancel","id":rid}))).await;
                                self.emit(Event::FileFailed { id: rid, name: p.name, reason: format!("无法写入：{e}") });
                            }
                        }
                    }
                } else {
                    let w = self.fs_waiting.lock().unwrap().remove(&rid);
                    if let Some(w) = w {
                        let _ = w.send(FsReply { header: m.header.clone(), body: m.body.clone() });
                    }
                }
            }
            "chunk" if self.cancelled.lock().unwrap().contains(&id) => {
                if let Some(Inbound::File { path, file, .. }) = inbound.remove(&id) {
                    drop(file);
                    let _ = tokio::fs::remove_file(&path).await;
                }
            }
            "chunk" => match inbound.get_mut(&id) {
                Some(Inbound::Image { hasher, buf, size }) => {
                    hasher.update(&m.body);
                    buf.extend_from_slice(&m.body);
                    if buf.len() as u64 > *size {
                        inbound.remove(&id);
                    }
                }
                Some(Inbound::File { hasher, file, name, size, done, .. }) => {
                    use tokio::io::AsyncWriteExt as _;
                    hasher.update(&m.body);
                    file.write_all(&m.body).await?;
                    let before = *done * 50 / (*size).max(1);
                    *done += m.body.len() as u64;
                    if *done * 50 / (*size).max(1) != before {
                        self.emit(Event::FileProgress { id: id.clone(), name: name.clone(), done: *done, total: *size, incoming: true });
                    }
                }
                None => {}
            },
            "end" => {
                let want = m.str("sha256").unwrap_or_default().to_string();
                match inbound.remove(&id) {
                    Some(Inbound::Image { hasher, buf, .. }) => {
                        if hex::encode(hasher.finalize()) == want {
                            self.emit(Event::Clip { from: peer.into(), content: ClipContent::Image(buf) });
                        }
                    }
                    Some(Inbound::File { hasher, mut file, path, name, push, .. }) => {
                        use tokio::io::AsyncWriteExt as _;
                        file.flush().await?;
                        let ok = hex::encode(hasher.finalize()) == want;
                        match push.as_ref().and_then(|p| p.mtime) {
                            Some(t) if ok => {
                                let _ = file.into_std().await.set_modified(std::time::UNIX_EPOCH + Duration::from_millis(t));
                            }
                            _ => drop(file),
                        }
                        if ok {
                            if push.is_some() {
                                // Tells the uploader where the file landed (the name may have changed).
                                let _ = tx.send(Message::new(json!({"t":"file.done","id":id,"path":path.to_string_lossy()}))).await;
                            }
                            self.emit(Event::FileDone { id, name, path: Some(path.to_string_lossy().into_owned()), incoming: true });
                        } else {
                            let _ = tokio::fs::remove_file(&path).await;
                            if push.is_some() {
                                let _ = tx.send(Message::new(json!({"t":"file.failed","id":id,"error":"校验失败"}))).await;
                            }
                            self.emit(Event::FileFailed { id, name, reason: "校验失败".into() });
                        }
                    }
                    None => {}
                }
            }
            "file.cancel" if self.serving.lock().unwrap().contains(&id) => {
                // The device stopped a pull from this PC; stream_file confirms it.
                self.cancelled.lock().unwrap().insert(id);
            }
            "file.cancel" => {
                // Also the phone confirming a pull we cancelled.
                let ours = self.cancelled.lock().unwrap().remove(&id);
                if let Some(Inbound::File { path, name, file, .. }) = inbound.remove(&id) {
                    drop(file);
                    let _ = tokio::fs::remove_file(&path).await;
                    if !ours {
                        self.emit(Event::FileFailed { id, name, reason: "对方已取消".into() });
                    }
                }
            }
            "file.accept" => {
                let out = self.outbound.lock().unwrap().remove(&id);
                if let Some(out) = out {
                    if out.peer == peer {
                        let e = self.clone();
                        let tx = tx.clone();
                        tokio::spawn(async move { e.stream_file(id, out, tx).await });
                    }
                }
            }
            "file.reject" => {
                if let Some(out) = self.outbound.lock().unwrap().remove(&id) {
                    self.emit(Event::FileFailed { id, name: out.name, reason: "对方拒绝".into() });
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// A device reads a file from this PC (protocol §8 `pull`, PC side): `fs.res` with the
    /// size, then `chunk`s and `end` under the request id.
    async fn serve_pull(self: Arc<Self>, peer: String, rid: String, path: String, tx: mpsc::Sender<Message>) {
        let checked = pcfs::check_path(&path).and_then(|p| {
            let m = std::fs::metadata(&p).map_err(|e| pcfs::io_err(&e))?;
            if m.is_dir() { Err("不能读取文件夹".to_string()) } else { Ok((p, m)) }
        });
        let (p, meta) = match checked {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(Message::new(json!({"t":"fs.res","rid":rid,"ok":false,"error":e}))).await;
                return;
            }
        };
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);
        let head = json!({"t":"fs.res","rid":rid,"ok":true,"size":meta.len(),"name":name,"m":mtime});
        self.manage(&rid);
        self.serving.lock().unwrap().insert(rid.clone());
        if tx.send(Message::new(head)).await.is_ok() {
            let out = Outbound { peer, path: p, name, size: meta.len() };
            self.clone().stream_file(rid.clone(), out, tx).await;
        }
        self.serving.lock().unwrap().remove(&rid);
    }

    async fn stream_file(self: Arc<Self>, id: String, out: Outbound, tx: mpsc::Sender<Message>) {
        use tokio::io::AsyncReadExt;
        let res: Result<()> = async {
            let mut f = tokio::fs::File::open(&out.path).await?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; CHUNK_SIZE];
            let (mut done, mut seq) = (0u64, 0u64);
            loop {
                if self.cancelled.lock().unwrap().remove(&id) {
                    let _ = tx.send(Message::new(json!({"t":"file.cancel","id":id}))).await;
                    bail!("已取消");
                }
                let n = f.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                tx.send(Message::with_body(json!({"t":"chunk","id":id,"seq":seq}), buf[..n].to_vec()))
                    .await
                    .map_err(|_| anyhow!("连接断开"))?;
                let before = done * 50 / out.size.max(1);
                done += n as u64;
                seq += 1;
                if done * 50 / out.size.max(1) != before {
                    self.emit(Event::FileProgress { id: id.clone(), name: out.name.clone(), done, total: out.size, incoming: false });
                }
            }
            tx.send(Message::new(json!({"t":"end","id":id,"sha256":hex::encode(hasher.finalize())})))
                .await
                .map_err(|_| anyhow!("连接断开"))?;
            Ok(())
        }
        .await;
        match res {
            Ok(()) => self.emit(Event::FileDone { id, name: out.name, path: None, incoming: false }),
            Err(e) => self.emit(Event::FileFailed { id, name: out.name, reason: e.to_string() }),
        }
    }
}

/// Returns false if `id` was already seen recently.
fn remember(seen: &mut VecDeque<String>, id: &str) -> bool {
    if seen.iter().any(|s| s == id) {
        return false;
    }
    seen.push_back(id.to_string());
    if seen.len() > 128 {
        seen.pop_front();
    }
    true
}

fn sanitize_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() { "file".into() } else { cleaned }
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
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

pub fn local_ipv4s() -> Vec<String> {
    let mut v: Vec<(u8, String)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(ip) if !ip.is_link_local() => {
                let name = i.name.to_lowercase();
                // Prefer real adapters over virtual ones (VPN/TUN, Hyper-V, VMware...).
                let virt = ["vethernet", "vmware", "virtualbox", "tun", "tap", "clash", "meta", "wsl", "loopback"]
                    .iter()
                    .any(|k| name.contains(k));
                let rank = if virt || ip.octets()[0] == 198 { 1 } else { 0 };
                Some((rank, ip.to_string()))
            }
            _ => None,
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, ip)| ip).collect()
}

fn verify_wake(psk_b64: &str, v: &Value) -> Result<()> {
    let port = v.get("port").and_then(Value::as_u64).unwrap_or(0);
    let ts = v.get("ts").and_then(Value::as_u64).unwrap_or(0);
    let mac = B64.decode(v.get("mac").and_then(Value::as_str).unwrap_or_default())?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    if now.abs_diff(ts) > 300 {
        bail!("stale wake");
    }
    let psk = B64.decode(psk_b64)?;
    let want = crypto::hmac(&psk, &[b"wake", port.to_string().as_bytes(), ts.to_string().as_bytes()]);
    if !crypto::ct_eq(&want, &mac) {
        bail!("bad wake mac");
    }
    Ok(())
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_name("../../evil.txt"), "evil.txt");
        assert_eq!(sanitize_name(r"C:\x\a<b>.png"), "a_b_.png");
        assert_eq!(sanitize_name(".."), "file");
    }

    #[test]
    fn unique_paths() {
        let dir = std::env::temp_dir().join(format!("sb-test-{}", crypto::random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a (1).txt"));
        assert_eq!(unique_path(&dir, "b.txt"), dir.join("b.txt"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
