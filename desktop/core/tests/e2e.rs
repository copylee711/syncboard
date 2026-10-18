use serde_json::json;
use std::time::Duration;
use syncboard_core::client::{self, Identity};
use syncboard_core::crypto;
use syncboard_core::frame::Message;
use syncboard_core::{ClipContent, Engine, Event};
use tokio::sync::mpsc::UnboundedReceiver;

async fn next_event(rx: &mut UnboundedReceiver<Event>, pred: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let e = rx.recv().await.expect("events closed");
            if pred(&e) {
                return e;
            }
        }
    })
    .await
    .expect("timed out waiting for event")
}

fn temp_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("sb-e2e-{}", crypto::random_id()))
}

#[tokio::test]
async fn qr_pair_session_clipboard_and_files() {
    let dir = temp_dir();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::start(&dir, 0, tx).await.unwrap();
    engine.with_store(|s| s.config.settings.download_dir = Some(dir.join("dl")));
    let addr = format!("127.0.0.1:{}", engine.port());
    let me = Identity { id: "phone1".into(), name: "Test Phone".into() };

    // Unknown devices are refused.
    assert!(client::connect_session(&addr, &me, &[0u8; 32]).await.is_err());

    // A wrong token is rejected; a valid one pairs, and is single-use.
    let uri = engine.pairing_uri();
    let p = client::parse_pair_uri(&uri).unwrap();
    assert!(client::pair_qr(&addr, &me, &[1u8; 32]).await.is_err());
    let (server_id, psk) = client::pair_qr(&addr, &me, &p.token).await.unwrap();
    assert_eq!(server_id, engine.device_id());
    assert!(client::pair_qr(&addr, &me, &p.token).await.is_err());
    next_event(&mut rx, |e| matches!(e, Event::Paired { .. })).await;

    let (mut r, mut w) = client::connect_session(&addr, &me, &psk).await.unwrap();
    next_event(&mut rx, |e| matches!(e, Event::PeerConnected { .. })).await;

    // phone → PC text, duplicates by id are dropped
    let msg = Message::with_body(json!({"t":"clip.text","id":"aa","hash":"x"}), "你好 phone".as_bytes().to_vec());
    w.write(&msg).await.unwrap();
    w.write(&msg).await.unwrap();
    match next_event(&mut rx, |e| matches!(e, Event::Clip { .. })).await {
        Event::Clip { content, .. } => assert_eq!(content, ClipContent::Text("你好 phone".into())),
        _ => unreachable!(),
    }

    // PC → phone text and image
    engine.send_clip(ClipContent::Text("from pc".into()));
    let m = r.read().await.unwrap();
    assert_eq!(m.kind(), "clip.text");
    assert_eq!(m.body, b"from pc");

    let png: Vec<u8> = (0..600_000u32).map(|i| (i % 251) as u8).collect();
    engine.send_clip(ClipContent::Image(png.clone()));
    let head = r.read().await.unwrap();
    assert_eq!(head.kind(), "clip.image");
    let mut got = Vec::new();
    loop {
        let m = r.read().await.unwrap();
        match m.kind() {
            "chunk" => got.extend_from_slice(&m.body),
            "end" => {
                assert_eq!(m.str("sha256").unwrap(), crypto::sha256_hex(&png));
                break;
            }
            k => panic!("unexpected {k}"),
        }
    }
    assert_eq!(got, png);

    // phone → PC file
    let data: Vec<u8> = (0..700_000u32).map(|i| (i * 7 % 256) as u8).collect();
    w.write(&Message::new(json!({"t":"file.offer","id":"f1","name":"../a.bin","size":data.len()}))).await.unwrap();
    assert_eq!(r.read().await.unwrap().kind(), "file.accept");
    for (seq, c) in data.chunks(256 * 1024).enumerate() {
        w.write(&Message::with_body(json!({"t":"chunk","id":"f1","seq":seq}), c.to_vec())).await.unwrap();
    }
    w.write(&Message::new(json!({"t":"end","id":"f1","sha256":crypto::sha256_hex(&data)}))).await.unwrap();
    match next_event(&mut rx, |e| matches!(e, Event::FileDone { .. } | Event::FileFailed { .. })).await {
        Event::FileDone { path, .. } => {
            let path = path.unwrap();
            assert!(path.ends_with("a.bin"));
            assert_eq!(std::fs::read(path).unwrap(), data);
        }
        e => panic!("{e:?}"),
    }

    // PC → phone file
    let src = dir.join("out.txt");
    std::fs::write(&src, b"hello file").unwrap();
    let id = engine.send_file("phone1", &src).await.unwrap();
    let offer = r.read().await.unwrap();
    assert_eq!(offer.kind(), "file.offer");
    w.write(&Message::new(json!({"t":"file.accept","id":id}))).await.unwrap();
    let chunk = r.read().await.unwrap();
    assert_eq!(chunk.body, b"hello file");
    assert_eq!(r.read().await.unwrap().kind(), "end");

    drop(w);
    drop(r);
    next_event(&mut rx, |e| matches!(e, Event::PeerDisconnected { .. })).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn code_pairing_needs_acceptance() {
    let dir = temp_dir();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::start(&dir, 0, tx).await.unwrap();
    let addr = format!("127.0.0.1:{}", engine.port());
    let me = Identity { id: "phone2".into(), name: "P2".into() };

    let (code_tx, code_rx) = tokio::sync::oneshot::channel();
    let a2 = addr.clone();
    let task = tokio::spawn(async move {
        client::pair_code(&a2, &Identity { id: "phone2".into(), name: "P2".into() }, |c| {
            let _ = code_tx.send(c.to_string());
        })
        .await
    });
    let (req, server_code) = match next_event(&mut rx, |e| matches!(e, Event::PairRequest { .. })).await {
        Event::PairRequest { req, code, .. } => (req, code),
        _ => unreachable!(),
    };
    assert_eq!(code_rx.await.unwrap(), server_code);
    engine.answer_pair(req, true);
    let (_, psk) = task.await.unwrap().unwrap();
    client::connect_session(&addr, &me, &psk).await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn wake_is_answered_and_unpaired_phones_are_rejected() {
    use base64::Engine as _;
    use syncboard_core::frame::{read_plain, write_plain};
    let b64 = base64::engine::general_purpose::STANDARD;
    let dir = temp_dir();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::start(&dir, 0, tx).await.unwrap();
    let addr = format!("127.0.0.1:{}", engine.port());
    let me = Identity { id: "phone3".into(), name: "P3".into() };
    let p = client::parse_pair_uri(&engine.pairing_uri()).unwrap();
    let (_, psk) = client::pair_qr(&addr, &me, &p.token).await.unwrap();

    let wake = |psk: [u8; 32]| {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let mac = crypto::hmac(&psk, &[b"wake", b"40000", ts.to_string().as_bytes()]);
        json!({"t":"adb_wake","v":1,"id":"phone3","port":40000,"ts":ts,"mac":b64.encode(mac)})
    };
    let send = |v: serde_json::Value| {
        let addr = addr.clone();
        async move {
            let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
            write_plain(&mut s, &v).await.unwrap();
            read_plain(&mut s).await.unwrap()
        }
    };

    assert_eq!(send(wake(psk)).await["t"], "wake_ok");
    next_event(&mut rx, |e| matches!(e, Event::AdbWake { .. })).await;
    assert_eq!(send(wake([9u8; 32])).await["code"], "bad_request");

    engine.unpair("phone3").unwrap();
    let r = send(wake(psk)).await;
    assert_eq!((r["t"].as_str(), r["code"].as_str()), (Some("wake_rejected"), Some("unknown_device")));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn unpair_tells_the_connected_phone() {
    let dir = temp_dir();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::start(&dir, 0, tx).await.unwrap();
    let addr = format!("127.0.0.1:{}", engine.port());
    let me = Identity { id: "phone4".into(), name: "P4".into() };
    let p = client::parse_pair_uri(&engine.pairing_uri()).unwrap();
    let (_, psk) = client::pair_qr(&addr, &me, &p.token).await.unwrap();
    let (mut r, _w) = client::connect_session(&addr, &me, &psk).await.unwrap();
    next_event(&mut rx, |e| matches!(e, Event::PeerConnected { .. })).await;

    engine.unpair("phone4").unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let m = r.read().await.unwrap();
            if m.kind() != "ping" {
                return m;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(m.kind(), "unpaired");
    assert!(client::connect_session(&addr, &me, &psk).await.is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn phone_unpair_removes_it_on_the_pc() {
    let dir = temp_dir();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::start(&dir, 0, tx).await.unwrap();
    let addr = format!("127.0.0.1:{}", engine.port());
    let me = Identity { id: "phone5".into(), name: "P5".into() };
    let p = client::parse_pair_uri(&engine.pairing_uri()).unwrap();
    let (_, psk) = client::pair_qr(&addr, &me, &p.token).await.unwrap();
    let (_r, mut w) = client::connect_session(&addr, &me, &psk).await.unwrap();
    next_event(&mut rx, |e| matches!(e, Event::PeerConnected { .. })).await;

    w.write(&Message::new(json!({"t":"unpaired"}))).await.unwrap();
    next_event(&mut rx, |e| matches!(e, Event::PeerDisconnected { .. })).await;
    assert!(engine.peers().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn wrong_psk_is_reported_as_key_mismatch() {
    let dir = temp_dir();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::start(&dir, 0, tx).await.unwrap();
    let addr = format!("127.0.0.1:{}", engine.port());
    let me = Identity { id: "phone6".into(), name: "P6".into() };
    let p = client::parse_pair_uri(&engine.pairing_uri()).unwrap();
    let (_, psk) = client::pair_qr(&addr, &me, &p.token).await.unwrap();

    // The phone holds a stale key: both ends fail to open the other's auth record.
    let err = client::connect_session(&addr, &me, &[7u8; 32]).await.err().expect("wrong psk must fail");
    assert!(err.is::<crypto::DecryptError>(), "{err:#}");
    match next_event(&mut rx, |e| matches!(e, Event::KeyMismatch { .. })).await {
        Event::KeyMismatch { id, .. } => assert_eq!(id, "phone6"),
        _ => unreachable!(),
    }
    assert!(engine.key_mismatch("phone6"));

    // The right key connects again and clears the flag.
    let _s = client::connect_session(&addr, &me, &psk).await.unwrap();
    next_event(&mut rx, |e| matches!(e, Event::PeerConnected { .. })).await;
    assert!(!engine.key_mismatch("phone6"));
    let _ = std::fs::remove_dir_all(dir);
}
