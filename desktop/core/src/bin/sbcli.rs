//! Headless test tool.
//!   sbcli serve [dir]        run the PC engine with clipboard sync
//!   sbcli phone <uri> [dir]  act as a phone: pair via QR uri, then print/sync text

use anyhow::Result;
use std::io::BufRead;
use std::path::PathBuf;
use syncboard_core::client::{self, Identity};
use syncboard_core::frame::Message;
use syncboard_core::{Engine, Event, PORT};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => serve(args.get(2).map(PathBuf::from)).await,
        Some("phone") => phone(&args[2]).await,
        _ => {
            eprintln!("usage: sbcli serve [dir] | sbcli phone <uri>");
            Ok(())
        }
    }
}

async fn serve(dir: Option<PathBuf>) -> Result<()> {
    let dir = dir.unwrap_or_else(|| PathBuf::from(std::env::var("APPDATA").unwrap()).join("SyncBoard"));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::start(&dir, PORT, tx).await?;
    let uri = engine.pairing_uri();
    let qr = qrcode::QrCode::new(uri.as_bytes())?;
    println!("{}", qr.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(true).build());
    println!("PAIR {uri}");
    #[cfg(windows)]
    {
        let e2 = engine.clone();
        let _sync = syncboard_core::sync::start(engine.clone(), rx, move |ev| {
            println!("EVENT {}", serde_json::to_string(&ev).unwrap());
            if let Event::PairRequest { req, code, .. } = ev {
                println!("auto-accepting pairing code {code}");
                e2.answer_pair(req, true);
            }
        });
        // stdin: "send <peerId> <path>"
        let e3 = engine.clone();
        let rt = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                if let Some(rest) = line.strip_prefix("send ") {
                    if let Some((peer, path)) = rest.split_once(' ') {
                        let (e, peer, path) = (e3.clone(), peer.to_string(), PathBuf::from(path));
                        rt.spawn(async move { println!("send: {:?}", e.send_file(&peer, &path).await) });
                    }
                }
            }
        });
        std::future::pending::<()>().await;
    }
    #[cfg(not(windows))]
    {
        let _ = rx;
        std::future::pending::<()>().await;
    }
    Ok(())
}

async fn phone(uri: &str) -> Result<()> {
    let p = client::parse_pair_uri(uri)?;
    let addr = format!("{}:{}", p.hosts.first().cloned().unwrap_or("127.0.0.1".into()), p.port);
    let me = Identity { id: syncboard_core::crypto::random_id(), name: "FakePhone".into() };
    let (_, psk) = client::pair_qr(&addr, &me, &p.token).await?;
    println!("paired");
    let (mut r, mut w) = client::connect_session(&addr, &me, &psk).await?;
    println!("connected as {}", me.id);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    loop {
        tokio::select! {
            m = r.read() => {
                let m = m?;
                match m.kind() {
                    "clip.text" => println!("RECV text: {}", String::from_utf8_lossy(&m.body)),
                    "ping" => w.write(&Message::new(serde_json::json!({"t":"pong"}))).await?,
                    k => println!("RECV {k} {}", m.header),
                }
            }
            Some(line) = rx.recv() => {
                let id = syncboard_core::crypto::random_id();
                let hash = syncboard_core::crypto::sha256_hex(line.as_bytes());
                w.write(&Message::with_body(serde_json::json!({"t":"clip.text","id":id,"hash":hash}), line.into_bytes())).await?;
            }
        }
    }
}
