//! The PC side of the protocol: listens for phones, handles pairing, sessions,
//! clipboard messages and file transfers. UI and OS clipboard live outside; they
//! talk to the engine through `Event`s and the public methods.

use crate::crypto::{self, Ecdh, RecordCipher};
use crate::frame::{read_plain, write_plain, Message, SecureReader, SecureWriter, CHUNK_SIZE};
use crate::store::{Peer, Store};
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
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
}

struct Session {
    gen: u64,
    name: String,
    addr: SocketAddr,
    tx: mpsc::Sender<Message>,
}

enum Inbound {
    Image { hasher: Sha256, buf: Vec<u8>, size: u64 },
    File { hasher: Sha256, file: tokio::fs::File, path: PathBuf, name: String, size: u64, done: u64 },
}

struct Outbound {
    peer: String,
    path: PathBuf,
    name: String,
    size: u64,
}

pub struct Engine {
    store: Mutex<Store>,
    events: mpsc::UnboundedSender<Event>,
    sessions: Mutex<HashMap<String, Session>>,
    tokens: Mutex<Vec<([u8; 32], Instant)>>,
    pending_pairs: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    outbound: Mutex<HashMap<String, Outbound>>,
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

    /// Creates a fresh single-use pairing token and returns the QR payload.
    pub fn pairing_uri(&self) -> String {
        let token = crypto::random_bytes::<32>();
        {
            let mut t = self.tokens.lock().unwrap();
            t.retain(|(_, at)| at.elapsed() < TOKEN_TTL);
            t.push((token, Instant::now()));
        }
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
            B64URL.encode(token)
        )
    }

    pub fn answer_pair(&self, req: u64, accept: bool) {
        if let Some(tx) = self.pending_pairs.lock().unwrap().remove(&req) {
            let _ = tx.send(accept);
        }
    }

    pub fn unpair(&self, id: &str) -> Result<()> {
        self.sessions.lock().unwrap().remove(id);
        let mut s = self.store.lock().unwrap();
        s.config.peers.retain(|p| p.id != id);
        s.save()
    }

    /// Sends clipboard content to every connected peer.
    pub fn send_clip(self: &Arc<Self>, content: ClipContent) {
        let targets: Vec<mpsc::Sender<Message>> =
            self.sessions.lock().unwrap().values().map(|s| s.tx.clone()).collect();
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

    /// Offers a file to a connected peer; streaming starts once the peer accepts.
    pub async fn send_file(self: &Arc<Self>, peer: &str, path: &Path) -> Result<String> {
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
        tx.send(Message::new(json!({"t":"file.offer","id":id,"name":name,"size":meta.len()})))
            .await
            .map_err(|_| anyhow!("peer disconnected"))?;
        Ok(id)
    }

    fn session_tx(&self, peer: &str) -> Option<mpsc::Sender<Message>> {
        self.sessions.lock().unwrap().get(peer).map(|s| s.tx.clone())
    }

    fn emit(&self, e: Event) {
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
            other => bail!("unexpected first frame {other:?}"),
        }
    }

    async fn handle_pair(self: &Arc<Self>, mut stream: TcpStream, req: Value) -> Result<()> {
        let field = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
        let peer_id = field("id").ok_or_else(|| anyhow!("missing id"))?;
        let peer_name = field("name").unwrap_or_else(|| "Phone".into());
        let platform = field("platform").unwrap_or_else(|| "android".into());
        let mode = field("mode").unwrap_or_default();
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
                    tokens.retain(|(_, at)| at.elapsed() < TOKEN_TTL);
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
            });
            s.save()?;
        }
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
        let auth = tokio::time::timeout(Duration::from_secs(10), reader.read()).await??;
        if auth.kind() != "auth" {
            bail!("expected auth");
        }

        let (tx, mut rx) = mpsc::channel::<Message>(32);
        let gen = self.next.fetch_add(1, Ordering::Relaxed);
        self.sessions.lock().unwrap().insert(
            peer_id.clone(),
            Session { gen, name: peer.name.clone(), addr, tx: tx.clone() },
        );
        {
            let mut s = self.store.lock().unwrap();
            if let Some(p) = s.config.peers.iter_mut().find(|p| p.id == peer_id) {
                p.last_addr = Some(addr.ip().to_string());
            }
            let _ = s.save();
        }
        self.emit(Event::PeerConnected { id: peer_id.clone(), name: peer.name.clone(), addr: addr.ip().to_string() });

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
            "file.offer" => {
                let name = sanitize_name(m.str("name").unwrap_or("file"));
                let size = m.u64("size").unwrap_or(0);
                let dir = self.store.lock().unwrap().download_dir();
                tokio::fs::create_dir_all(&dir).await?;
                let path = unique_path(&dir, &name);
                match tokio::fs::File::create(&path).await {
                    Ok(file) => {
                        inbound.insert(id.clone(), Inbound::File { hasher: Sha256::new(), file, path, name, size, done: 0 });
                        let _ = tx.send(Message::new(json!({"t":"file.accept","id":id}))).await;
                    }
                    Err(_) => {
                        let _ = tx.send(Message::new(json!({"t":"file.reject","id":id}))).await;
                    }
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
                    Some(Inbound::File { hasher, mut file, path, name, .. }) => {
                        use tokio::io::AsyncWriteExt as _;
                        file.flush().await?;
                        drop(file);
                        if hex::encode(hasher.finalize()) == want {
                            self.emit(Event::FileDone { id, name, path: Some(path.to_string_lossy().into_owned()), incoming: true });
                        } else {
                            let _ = tokio::fs::remove_file(&path).await;
                            self.emit(Event::FileFailed { id, name, reason: "校验失败".into() });
                        }
                    }
                    None => {}
                }
            }
            "file.cancel" => {
                if let Some(Inbound::File { path, name, file, .. }) = inbound.remove(&id) {
                    drop(file);
                    let _ = tokio::fs::remove_file(&path).await;
                    self.emit(Event::FileFailed { id, name, reason: "对方已取消".into() });
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

    async fn stream_file(self: Arc<Self>, id: String, out: Outbound, tx: mpsc::Sender<Message>) {
        use tokio::io::AsyncReadExt;
        let res: Result<()> = async {
            let mut f = tokio::fs::File::open(&out.path).await?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; CHUNK_SIZE];
            let (mut done, mut seq) = (0u64, 0u64);
            loop {
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
