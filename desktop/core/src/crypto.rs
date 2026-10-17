//! Primitives from docs/protocol.md: HKDF/HMAC helpers, P-256 ECDH, and the
//! per-direction AES-256-GCM record cipher.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::Aes256Gcm;
use anyhow::{anyhow, Result};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use p256::ecdh::EphemeralSecret;
use p256::{EncodedPoint, PublicKey};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

pub fn random_id() -> String {
    hex::encode(random_bytes::<8>())
}

pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(sha256(&[data]))
}

pub fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) {
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, out)
        .expect("hkdf output length");
}

pub fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("hmac key");
    for p in parts {
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

/// Constant-time comparison for MACs.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub struct Ecdh {
    secret: EphemeralSecret,
    pub public: [u8; 65],
}

impl Ecdh {
    pub fn new() -> Self {
        let secret = EphemeralSecret::random(&mut OsRng);
        let point = EncodedPoint::from(secret.public_key());
        let mut public = [0u8; 65];
        public.copy_from_slice(point.as_bytes());
        Ecdh { secret, public }
    }

    /// Returns the 32-byte x-coordinate of the shared point.
    pub fn agree(&self, peer_public: &[u8]) -> Result<[u8; 32]> {
        let pk = PublicKey::from_sec1_bytes(peer_public).map_err(|_| anyhow!("bad public key"))?;
        let shared = self.secret.diffie_hellman(&pk);
        let mut out = [0u8; 32];
        out.copy_from_slice(shared.raw_secret_bytes());
        Ok(out)
    }
}

/// Transcript hash binding both public keys and nonces.
pub fn transcript(pub_c: &[u8], pub_s: &[u8], nonce_c: &[u8], nonce_s: &[u8]) -> [u8; 32] {
    sha256(&[pub_c, pub_s, nonce_c, nonce_s])
}

pub fn pairing_code(th: &[u8; 32]) -> String {
    let d = sha256(&[b"syncboard/v1/code", th]);
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

pub fn derive_psk(shared: &[u8; 32], token: Option<&[u8]>, th: &[u8; 32]) -> [u8; 32] {
    let mut ikm = shared.to_vec();
    if let Some(t) = token {
        ikm.extend_from_slice(t);
    }
    let mut psk = [0u8; 32];
    hkdf(&ikm, th, b"syncboard/v1/psk", &mut psk);
    psk
}

/// Returns (client→server key, server→client key).
pub fn session_keys(psk: &[u8], nonce_c: &[u8], nonce_s: &[u8]) -> ([u8; 32], [u8; 32]) {
    let mut salt = nonce_c.to_vec();
    salt.extend_from_slice(nonce_s);
    let mut k = [0u8; 64];
    hkdf(psk, &salt, b"syncboard/v1/session", &mut k);
    let mut c2s = [0u8; 32];
    let mut s2c = [0u8; 32];
    c2s.copy_from_slice(&k[..32]);
    s2c.copy_from_slice(&k[32..]);
    (c2s, s2c)
}

/// One direction of an encrypted stream. Nonce = 4 zero bytes ‖ u64 counter.
pub struct RecordCipher {
    aead: Aes256Gcm,
    counter: u64,
}

impl RecordCipher {
    pub fn new(key: &[u8; 32]) -> Self {
        RecordCipher { aead: Aes256Gcm::new(key.into()), counter: 0 }
    }

    fn next_nonce(&mut self) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[4..].copy_from_slice(&self.counter.to_be_bytes());
        self.counter += 1;
        n
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let n = self.next_nonce();
        self.aead.encrypt(&n.into(), plaintext).expect("aes-gcm encrypt")
    }

    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let n = self.next_nonce();
        self.aead
            .decrypt(&n.into(), ciphertext)
            .map_err(|_| anyhow!("decryption failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecdh_agrees() {
        let a = Ecdh::new();
        let b = Ecdh::new();
        assert_eq!(a.agree(&b.public).unwrap(), b.agree(&a.public).unwrap());
    }

    #[test]
    fn record_roundtrip_and_counter() {
        let k = [7u8; 32];
        let mut tx = RecordCipher::new(&k);
        let mut rx = RecordCipher::new(&k);
        for msg in [&b"hello"[..], b"", b"world"] {
            let c = tx.seal(msg);
            assert_eq!(rx.open(&c).unwrap(), msg);
        }
        // Replaying an old record fails because the counter moved on.
        let mut tx2 = RecordCipher::new(&k);
        let old = tx2.seal(b"hello");
        assert!(rx.open(&old).is_err());
    }

    /// Fixed vectors shared with the Android implementation (see ProtocolVectorsTest.kt).
    #[test]
    fn interop_vectors() {
        let psk = [1u8; 32];
        let (c2s, s2c) = session_keys(&psk, &[2u8; 32], &[3u8; 32]);
        let mut tx = RecordCipher::new(&c2s);
        let ct = tx.seal(b"syncboard");
        let th = transcript(&[4u8; 65], &[5u8; 65], &[6u8; 32], &[7u8; 32]);
        let psk2 = derive_psk(&[8u8; 32], Some(&[9u8; 32]), &th);
        println!("c2s={}", hex::encode(c2s));
        println!("s2c={}", hex::encode(s2c));
        println!("ct={}", hex::encode(&ct));
        println!("th={}", hex::encode(th));
        println!("code={}", pairing_code(&th));
        println!("psk={}", hex::encode(psk2));
        println!("hmac={}", hex::encode(hmac(&[9u8; 32], &[b"client", &th])));
    }
}
