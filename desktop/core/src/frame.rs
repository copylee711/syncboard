//! Wire framing: plain JSON frames for the handshake, then encrypted messages.

use crate::crypto::RecordCipher;
use anyhow::{bail, Result};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_PLAIN_FRAME: usize = 64 * 1024;
pub const MAX_RECORD: usize = 1024 * 1024 + 16 + 4096;
pub const CHUNK_SIZE: usize = 256 * 1024;

pub async fn read_plain<R: AsyncRead + Unpin>(r: &mut R) -> Result<Value> {
    let len = r.read_u32().await? as usize;
    if len > MAX_PLAIN_FRAME {
        bail!("plain frame too large: {len}");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

pub async fn write_plain<W: AsyncWrite + Unpin>(w: &mut W, v: &Value) -> Result<()> {
    let buf = serde_json::to_vec(v)?;
    w.write_u32(buf.len() as u32).await?;
    w.write_all(&buf).await?;
    w.flush().await?;
    Ok(())
}

/// A decoded application message: JSON header plus optional binary body.
#[derive(Debug, Clone)]
pub struct Message {
    pub header: Value,
    pub body: Vec<u8>,
}

impl Message {
    pub fn new(header: Value) -> Self {
        Message { header, body: Vec::new() }
    }

    pub fn with_body(header: Value, body: Vec<u8>) -> Self {
        Message { header, body }
    }

    pub fn kind(&self) -> &str {
        self.header.get("t").and_then(Value::as_str).unwrap_or("")
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.header.get(key).and_then(Value::as_str)
    }

    pub fn u64(&self, key: &str) -> Option<u64> {
        self.header.get(key).and_then(Value::as_u64)
    }

    fn encode(&self) -> Vec<u8> {
        let h = serde_json::to_vec(&self.header).expect("json");
        let mut out = Vec::with_capacity(4 + h.len() + self.body.len());
        out.extend_from_slice(&(h.len() as u32).to_be_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(&self.body);
        out
    }

    fn decode(plain: Vec<u8>) -> Result<Self> {
        if plain.len() < 4 {
            bail!("short message");
        }
        let hlen = u32::from_be_bytes([plain[0], plain[1], plain[2], plain[3]]) as usize;
        if 4 + hlen > plain.len() {
            bail!("bad header length");
        }
        let header = serde_json::from_slice(&plain[4..4 + hlen])?;
        let body = plain[4 + hlen..].to_vec();
        Ok(Message { header, body })
    }
}

pub struct SecureReader<R> {
    inner: R,
    cipher: RecordCipher,
}

impl<R: AsyncRead + Unpin> SecureReader<R> {
    pub fn new(inner: R, cipher: RecordCipher) -> Self {
        SecureReader { inner, cipher }
    }

    pub async fn read(&mut self) -> Result<Message> {
        let len = self.inner.read_u32().await? as usize;
        if len > MAX_RECORD {
            bail!("record too large: {len}");
        }
        let mut buf = vec![0u8; len];
        self.inner.read_exact(&mut buf).await?;
        Message::decode(self.cipher.open(&buf)?)
    }
}

pub struct SecureWriter<W> {
    inner: W,
    cipher: RecordCipher,
}

impl<W: AsyncWrite + Unpin> SecureWriter<W> {
    pub fn new(inner: W, cipher: RecordCipher) -> Self {
        SecureWriter { inner, cipher }
    }

    pub async fn write(&mut self, m: &Message) -> Result<()> {
        let ct = self.cipher.seal(&m.encode());
        self.inner.write_u32(ct.len() as u32).await?;
        self.inner.write_all(&ct).await?;
        self.inner.flush().await?;
        Ok(())
    }
}
