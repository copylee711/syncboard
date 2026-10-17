//! Windows Mobile Hotspot, for networks where the phone and PC can't see each other
//! (client isolation, campus subnets). The phone joins the PC's hotspot, so mDNS and
//! direct connections work.

use windows::Networking::Connectivity::NetworkInformation;
use windows::Networking::NetworkOperators::{
    NetworkOperatorTetheringManager, TetheringOperationStatus, TetheringOperationalState,
};

pub struct Hotspot {
    pub ssid: String,
    pub passphrase: String,
    pub on: bool,
}

impl Hotspot {
    /// Standard Wi-Fi QR text; the phone camera joins the network when scanned.
    pub fn qr_text(&self) -> String {
        let esc = |s: &str| {
            s.chars().fold(String::new(), |mut out, c| {
                if matches!(c, '\\' | ';' | ',' | ':' | '"') {
                    out.push('\\');
                }
                out.push(c);
                out
            })
        };
        format!("WIFI:T:WPA;S:{};P:{};;", esc(&self.ssid), esc(&self.passphrase))
    }
}

fn manager() -> Result<NetworkOperatorTetheringManager, String> {
    let profile = NetworkInformation::GetInternetConnectionProfile()
        .map_err(|_| "电脑当前没有联网，无法开启移动热点".to_string())?;
    NetworkOperatorTetheringManager::CreateFromConnectionProfile(&profile)
        .map_err(|e| format!("这台电脑不支持移动热点：{}", e.message()))
}

pub fn info() -> Result<Hotspot, String> {
    let m = manager()?;
    let cfg = m.GetCurrentAccessPointConfiguration().map_err(|e| e.message().to_string())?;
    Ok(Hotspot {
        ssid: cfg.Ssid().map(|s| s.to_string()).unwrap_or_default(),
        passphrase: cfg.Passphrase().map(|s| s.to_string()).unwrap_or_default(),
        on: m.TetheringOperationalState().ok() == Some(TetheringOperationalState::On),
    })
}

/// Turns the hotspot on with the user's existing name and password.
pub fn start() -> Result<Hotspot, String> {
    let m = manager()?;
    if m.TetheringOperationalState().ok() != Some(TetheringOperationalState::On) {
        let r = m
            .StartTetheringAsync()
            .and_then(|op| op.get())
            .map_err(|e| format!("开启热点失败：{}", e.message()))?;
        if r.Status().ok() != Some(TetheringOperationStatus::Success) {
            let msg = r.AdditionalErrorMessage().map(|s| s.to_string()).unwrap_or_default();
            return Err(format!("开启热点失败{}", if msg.is_empty() { String::new() } else { format!("：{msg}") }));
        }
    }
    info()
}

pub fn stop() {
    if let Ok(m) = manager() {
        let _ = m.StopTetheringAsync().and_then(|op| op.get());
    }
}
