//! Paired phones. One JSON file; the secret is base64 inside it.
//! TODO(phase 4): move the secret into Windows Credential Manager / Keychain.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairedDevice {
    pub device_id: String,
    pub name: String,
    pub secret_b64: String,
    pub paired_at: String,
}

impl PairedDevice {
    pub fn secret(&self) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode(&self.secret_b64).unwrap_or_default()
    }
}

pub struct DeviceStore {
    path: PathBuf,
    devices: Mutex<Vec<PairedDevice>>,
}

impl DeviceStore {
    pub fn open(path: &Path) -> Self {
        let devices = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { path: path.to_path_buf(), devices: Mutex::new(devices) }
    }

    fn save(&self, list: &[PairedDevice]) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, serde_json::to_string_pretty(list).unwrap());
    }

    pub fn list(&self) -> Vec<PairedDevice> {
        self.devices.lock().unwrap().clone()
    }

    pub fn get(&self, device_id: &str) -> Option<PairedDevice> {
        self.devices.lock().unwrap().iter().find(|d| d.device_id == device_id).cloned()
    }

    /// One phone at a time: adding a device replaces any previous pairing.
    pub fn add(&self, name: &str, secret: &[u8; 32]) -> PairedDevice {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).expect("os randomness");
        let d = PairedDevice {
            device_id: id.iter().map(|b| format!("{b:02x}")).collect(),
            name: name.to_string(),
            secret_b64: base64::engine::general_purpose::STANDARD.encode(secret),
            paired_at: now_iso(),
        };
        let mut g = self.devices.lock().unwrap();
        g.clear();
        g.push(d.clone());
        self.save(&g);
        d
    }

    pub fn remove(&self, device_id: &str) {
        let mut g = self.devices.lock().unwrap();
        g.retain(|d| d.device_id != device_id);
        self.save(&g);
    }
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // Good enough for a "paired on" label without a date crate: days since epoch.
    let days = secs / 86400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_replaces_persists_and_removes() {
        let p = std::env::temp_dir().join(format!("mochi-devices-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let s = DeviceStore::open(&p);
        let a = s.add("Pixel", &[1u8; 32]);
        let b = s.add("Galaxy", &[2u8; 32]);
        assert_ne!(a.device_id, b.device_id);
        assert_eq!(s.list().len(), 1, "one phone at a time");
        assert_eq!(s.get(&b.device_id).unwrap().secret(), vec![2u8; 32]);
        let reopened = DeviceStore::open(&p);
        assert_eq!(reopened.list(), s.list());
        s.remove(&b.device_id);
        assert!(s.list().is_empty());
        let _ = std::fs::remove_file(&p);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20756), (2026, 10, 30));
    }
}
