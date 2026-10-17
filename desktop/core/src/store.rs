//! Persistent state: this device's identity, paired peers and settings.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub platform: String,
    /// base64 pre-shared key from pairing.
    pub psk: String,
    #[serde(default)]
    pub last_addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub sync_text: bool,
    pub sync_images: bool,
    pub auto_accept_files: bool,
    /// None = Downloads\SyncBoard.
    pub download_dir: Option<PathBuf>,
    /// Restart the phone daemon over USB when a paired phone is offline and plugged in.
    pub auto_adb: bool,
    /// Custom adb.exe path; None = search PATH and the Android SDK.
    pub adb_path: Option<PathBuf>,
    /// "system", "light" or "dark".
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            sync_text: true,
            sync_images: true,
            auto_accept_files: true,
            download_dir: None,
            auto_adb: true,
            adb_path: None,
            theme: "system".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub device_id: String,
    pub name: String,
    #[serde(default)]
    pub peers: Vec<Peer>,
    #[serde(default)]
    pub settings: Settings,
}

pub struct Store {
    path: PathBuf,
    pub config: Config,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Store> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("config.json");
        let config = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(_) => Config {
                device_id: crate::crypto::random_id(),
                name: default_name(),
                peers: Vec::new(),
                settings: Settings::default(),
            },
        };
        let store = Store { path, config };
        store.save()?;
        Ok(store)
    }

    pub fn save(&self) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.config)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn peer(&self, id: &str) -> Option<&Peer> {
        self.config.peers.iter().find(|p| p.id == id)
    }

    pub fn upsert_peer(&mut self, peer: Peer) {
        self.config.peers.retain(|p| p.id != peer.id);
        self.config.peers.push(peer);
    }

    pub fn download_dir(&self) -> PathBuf {
        self.config.settings.download_dir.clone().unwrap_or_else(default_download_dir)
    }
}

fn default_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

fn default_download_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| ".".into());
    home.join("Downloads").join("SyncBoard")
}
