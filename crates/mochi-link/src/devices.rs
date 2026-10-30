//! Paired phones. `devices.json` holds the id, name and date; the shared secret lives in
//! the platform secret store (Windows Credential Manager) under `phone-secret/<device_id>`.
//! Files written before 0.1.15 carried the secret inline as `secret_b64`; it is moved into
//! the store and scrubbed from the file the first time such a file is opened.

use base64::Engine;
use mochi_secrets::{keys, SharedSecrets};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// What the app sees. Never carries the secret, so it is safe to hand to the Settings panel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairedDevice {
    pub device_id: String,
    pub name: String,
    pub paired_at: String,
}

/// On-disk row; `secret_b64` only appears in files from before the secret store.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Row {
    device_id: String,
    name: String,
    paired_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret_b64: Option<String>,
}

impl From<&Row> for PairedDevice {
    fn from(r: &Row) -> Self {
        PairedDevice { device_id: r.device_id.clone(), name: r.name.clone(), paired_at: r.paired_at.clone() }
    }
}

pub struct DeviceStore {
    path: PathBuf,
    secrets: SharedSecrets,
    devices: Mutex<Vec<Row>>,
}

impl DeviceStore {
    pub fn open(path: &Path, secrets: SharedSecrets) -> Self {
        let mut rows: Vec<Row> = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let store = Self { path: path.to_path_buf(), secrets, devices: Mutex::new(Vec::new()) };
        // Migrate inline secrets into the store and rewrite the file without them.
        let mut migrated = false;
        for r in rows.iter_mut() {
            if let Some(b64) = r.secret_b64.take() {
                match store.secrets.set(&keys::phone_secret(&r.device_id), &b64) {
                    Ok(()) => migrated = true,
                    Err(e) => {
                        eprintln!("[mochi-link] could not move the pairing secret for {} into the secret store: {e}", r.name);
                        r.secret_b64 = Some(b64);
                    }
                }
            }
        }
        if migrated {
            store.save(&rows);
        }
        *store.devices.lock().unwrap() = rows;
        store
    }

    fn save(&self, list: &[Row]) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, serde_json::to_string_pretty(list).unwrap());
    }

    pub fn list(&self) -> Vec<PairedDevice> {
        self.devices.lock().unwrap().iter().map(PairedDevice::from).collect()
    }

    pub fn get(&self, device_id: &str) -> Option<PairedDevice> {
        self.devices.lock().unwrap().iter().find(|d| d.device_id == device_id).map(PairedDevice::from)
    }

    /// The shared secret for a paired phone, or None when unknown (or the store is unreadable).
    pub fn secret(&self, device_id: &str) -> Option<Vec<u8>> {
        let row_b64 = self.devices.lock().unwrap().iter().find(|d| d.device_id == device_id)?.secret_b64.clone();
        let b64 = match row_b64 {
            Some(b) => b, // migration failed earlier; still honour the pairing
            None => self.secrets.get(&keys::phone_secret(device_id)).ok().flatten()?,
        };
        B64.decode(b64).ok()
    }

    /// One phone at a time: adding a device replaces any previous pairing (and its secret).
    pub fn add(&self, name: &str, secret: &[u8; 32]) -> PairedDevice {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).expect("os randomness");
        let device_id: String = id.iter().map(|b| format!("{b:02x}")).collect();
        let b64 = B64.encode(secret);
        let mut g = self.devices.lock().unwrap();
        for old in g.drain(..) {
            let _ = self.secrets.delete(&keys::phone_secret(&old.device_id));
        }
        let mut row = Row { device_id: device_id.clone(), name: name.to_string(), paired_at: now_iso(), secret_b64: None };
        if let Err(e) = self.secrets.set(&keys::phone_secret(&device_id), &b64) {
            // Never lose a pairing the user just approved on two screens: fall back to the file.
            eprintln!("[mochi-link] secret store unavailable ({e}); keeping the pairing secret in devices.json");
            row.secret_b64 = Some(b64);
        }
        g.push(row.clone());
        self.save(&g);
        PairedDevice::from(&row)
    }

    pub fn remove(&self, device_id: &str) {
        let mut g = self.devices.lock().unwrap();
        g.retain(|d| d.device_id != device_id);
        let _ = self.secrets.delete(&keys::phone_secret(device_id));
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
    use mochi_secrets::MemoryStore;
    use std::sync::Arc;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mochi-devices-{}-{name}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn add_replaces_persists_and_removes() {
        let p = tmp("basic");
        let secrets: SharedSecrets = Arc::new(MemoryStore::default());
        let s = DeviceStore::open(&p, secrets.clone());
        let a = s.add("Pixel", &[1u8; 32]);
        let b = s.add("Galaxy", &[2u8; 32]);
        assert_ne!(a.device_id, b.device_id);
        assert_eq!(s.list().len(), 1, "one phone at a time");
        assert_eq!(s.secret(&b.device_id), Some(vec![2u8; 32]));
        assert_eq!(s.secret(&a.device_id), None, "replaced phone's secret is gone from the store");
        assert!(!std::fs::read_to_string(&p).unwrap().contains("secret"), "no secret material in devices.json");
        let reopened = DeviceStore::open(&p, secrets.clone());
        assert_eq!(reopened.list(), s.list());
        assert_eq!(reopened.secret(&b.device_id), Some(vec![2u8; 32]));
        s.remove(&b.device_id);
        assert!(s.list().is_empty());
        assert_eq!(secrets.get(&keys::phone_secret(&b.device_id)).unwrap(), None);
        let _ = std::fs::remove_file(&p);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20756), (2026, 10, 30));
    }

    #[test]
    fn old_files_with_inline_secrets_are_migrated_and_scrubbed() {
        let p = tmp("migrate");
        std::fs::write(&p, r#"[{"device_id":"ab12","name":"Pixel","secret_b64":"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=","paired_at":"2026-10-01"}]"#).unwrap();
        let secrets: SharedSecrets = Arc::new(MemoryStore::default());
        let s = DeviceStore::open(&p, secrets.clone());
        assert_eq!(s.secret("ab12"), Some(vec![1u8; 32]));
        assert_eq!(secrets.get("phone-secret/ab12").unwrap().as_deref(), Some("AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="));
        assert!(!std::fs::read_to_string(&p).unwrap().contains("secret_b64"), "secret scrubbed from the file");
        let _ = std::fs::remove_file(&p);
    }
}
