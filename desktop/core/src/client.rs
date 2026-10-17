//! Phone-side protocol, used by integration tests and `sbcli phone` to exercise the
//! engine without a real device. Mirrors android/.../proto/Client.kt.

use crate::crypto::{self, Ecdh, RecordCipher};
use crate::frame::{read_plain, write_plain, Message, SecureReader, SecureWriter};
use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine as _;
use serde_json::{json, Value};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

pub struct PairUri {
    pub hosts: Vec<String>,
    pub port: u16,
    pub id: String,
    pub token: Vec<u8>,
}

pub fn parse_pair_uri(uri: &str) -> Result<PairUri> {
    let q = uri.split_once('?').map(|(_, q)| q).ok_or_else(|| anyhow!("no query"))?;
    let get = |k: &str| q.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).map(str::to_owned);
    Ok(PairUri {
        hosts: get("h").unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
        port: get("p").and_then(|p| p.parse().ok()).unwrap_or(crate::engine::PORT),
        id: get("id").unwrap_or_default(),
        token: B64URL.decode(get("t").unwrap_or_default())?,
    })
}

pub struct Identity {
    pub id: String,
    pub name: String,
}

/// QR pairing; returns (server id, psk).
pub async fn pair_qr(addr: &str, me: &Identity, token: &[u8]) -> Result<(String, [u8; 32])> {
    let mut s = TcpStream::connect(addr).await?;
    let ecdh = Ecdh::new();
    let nonce_c = crypto::random_bytes::<32>();
    write_plain(
        &mut s,
        &json!({"t":"pair","v":1,"mode":"qr","id":me.id,"name":me.name,"platform":"android",
                "pub":B64.encode(ecdh.public),"nonce":B64.encode(nonce_c)}),
    )
    .await?;
    let resp = read_plain(&mut s).await?;
    let pub_s = B64.decode(resp["pub"].as_str().unwrap_or_default())?;
    let nonce_s = B64.decode(resp["nonce"].as_str().unwrap_or_default())?;
    let shared = ecdh.agree(&pub_s)?;
    let th = crypto::transcript(&ecdh.public, &pub_s, &nonce_c, &nonce_s);
    let mac = crypto::hmac(token, &[b"client", &th]);
    write_plain(&mut s, &json!({"t":"pair_proof","mac":B64.encode(mac)})).await?;
    let ok = read_plain(&mut s).await?;
    if ok["t"] != "pair_ok" {
        bail!("pairing rejected");
    }
    let mac_s = B64.decode(ok["mac"].as_str().unwrap_or_default())?;
    if !crypto::ct_eq(&mac_s, &crypto::hmac(token, &[b"server", &th])) {
        bail!("server proof mismatch");
    }
    let server_id = resp["id"].as_str().unwrap_or_default().to_string();
    Ok((server_id, crypto::derive_psk(&shared, Some(token), &th)))
}

/// Code pairing; `on_code` is called with the 6-digit code to show.
pub async fn pair_code(addr: &str, me: &Identity, on_code: impl FnOnce(&str)) -> Result<(String, [u8; 32])> {
    let mut s = TcpStream::connect(addr).await?;
    let ecdh = Ecdh::new();
    let nonce_c = crypto::random_bytes::<32>();
    write_plain(
        &mut s,
        &json!({"t":"pair","v":1,"mode":"code","id":me.id,"name":me.name,"platform":"android",
                "pub":B64.encode(ecdh.public),"nonce":B64.encode(nonce_c)}),
    )
    .await?;
    let resp = read_plain(&mut s).await?;
    let pub_s = B64.decode(resp["pub"].as_str().unwrap_or_default())?;
    let nonce_s = B64.decode(resp["nonce"].as_str().unwrap_or_default())?;
    let shared = ecdh.agree(&pub_s)?;
    let th = crypto::transcript(&ecdh.public, &pub_s, &nonce_c, &nonce_s);
    on_code(&crypto::pairing_code(&th));
    let ok = read_plain(&mut s).await?;
    if ok["t"] != "pair_ok" {
        bail!("pairing rejected");
    }
    let server_id = resp["id"].as_str().unwrap_or_default().to_string();
    Ok((server_id, crypto::derive_psk(&shared, None, &th)))
}

pub async fn connect_session(
    addr: &str,
    me: &Identity,
    psk: &[u8],
) -> Result<(SecureReader<OwnedReadHalf>, SecureWriter<OwnedWriteHalf>)> {
    let mut s = TcpStream::connect(addr).await?;
    s.set_nodelay(true)?;
    let nonce_c = crypto::random_bytes::<32>();
    write_plain(&mut s, &json!({"t":"hello","v":1,"id":me.id,"nonce":B64.encode(nonce_c)})).await?;
    let resp: Value = read_plain(&mut s).await?;
    if resp["t"] != "hello" {
        bail!("session refused: {resp}");
    }
    let nonce_s = B64.decode(resp["nonce"].as_str().unwrap_or_default())?;
    let (c2s, s2c) = crypto::session_keys(psk, &nonce_c, &nonce_s);
    let (r, w) = s.into_split();
    let mut reader = SecureReader::new(r, RecordCipher::new(&s2c));
    let mut writer = SecureWriter::new(w, RecordCipher::new(&c2s));
    writer.write(&Message::new(json!({"t":"auth","name":me.name,"platform":"android"}))).await?;
    let auth = reader.read().await?;
    if auth.kind() != "auth" {
        bail!("expected auth");
    }
    Ok((reader, writer))
}
