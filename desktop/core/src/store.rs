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
    /// The user's own name for this device (备注), shown instead of `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Set for file-manager apps that pair like a phone ("twinfile"); None for the
    /// SyncBoard phone service. They get no clipboard and no adb handling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

impl Peer {
    pub fn display_name(&self) -> &str {
        self.alias.as_deref().filter(|a| !a.is_empty()).unwrap_or(&self.name)
    }
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
    /// Put verification codes from the phone's text messages on the clipboard.
    pub copy_sms_codes: bool,
    /// Show phone messages (codes, texts) as Windows notifications.
    pub phone_notifications: bool,
    /// Phone apps (package names) whose notifications aren't shown.
    pub muted_apps: Vec<String>,
    /// Skip a phone app's notifications while its PC version (WeChat, QQ...) is running.
    pub skip_running_apps: bool,
    /// Shrink images from the phone to what Windows clipboard history (Win+V) keeps.
    pub fit_clipboard_history: bool,
    /// Let paired devices (TwinFile) browse and change this PC's files.
    pub allow_remote_browse: bool,
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
            copy_sms_codes: true,
            phone_notifications: true,
            muted_apps: Vec::new(),
            skip_running_apps: true,
            fit_clipboard_history: false,
            allow_remote_browse: false,
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

    pub fn upsert_peer(&mut self, mut peer: Peer) {
        // Pairing again keeps the 备注.
        if peer.alias.is_none() {
            peer.alias = self.peer(&peer.id).and_then(|p| p.alias.clone());
        }
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
