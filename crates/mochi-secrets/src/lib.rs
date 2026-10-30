//! Where Mochi keeps secrets.
//!
//! Three things on the desktop are secret: the phone pairing secret, the local-AI API key,
//! and the private key of the phone link's TLS certificate. They used to sit in plain JSON
//! and PEM files in the app data folder. Now they go through a [`SecretStore`]:
//!
//! - **Windows**: Credential Manager (generic credentials, target `Mochi/<key>`).
//! - **macOS**: the login Keychain (service `Mochi`).
//! - **Elsewhere** (Linux dev boxes, CI): a JSON file next to the other data, so the code
//!   paths are identical even where there is no OS vault.
//! - **Tests**: [`MemoryStore`].
//!
//! Values are small UTF-8 strings (base64 for raw bytes); Credential Manager caps a blob at
//! 2560 bytes, which is plenty for a 32-byte secret or a PKCS#8 P-256 key in PEM.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Keys of the secrets Mochi stores. Kept in one place so a listing of the vault reads well.
pub mod keys {
    /// Phone pairing secret; the device id is appended: `phone-secret/<device_id>`.
    pub const PHONE_SECRET: &str = "phone-secret";
    /// Bearer token for the local OpenAI-compatible brain.
    pub const LOCAL_API_KEY: &str = "local-api-key";
    /// PEM private key of the phone link's TLS certificate.
    pub const LINK_TLS_KEY: &str = "link-tls-key";

    pub fn phone_secret(device_id: &str) -> String {
        format!("{PHONE_SECRET}/{device_id}")
    }
}

pub trait SecretStore: Send + Sync {
    /// `Ok(None)` when there is no such secret.
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    /// Deleting a missing secret is not an error.
    fn delete(&self, key: &str) -> Result<(), String>;
    /// For logs and the Settings panel.
    fn backend_name(&self) -> &'static str;
}

pub type SharedSecrets = Arc<dyn SecretStore>;

/// The store for this platform. `data_dir` is only used by the file fallback.
pub fn open_default(data_dir: &Path) -> SharedSecrets {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let _ = data_dir;
        Arc::new(NativeStore)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Arc::new(FileStore::open(data_dir.join("secrets.json")))
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
    fn backend_name(&self) -> &'static str {
        "memory"
    }
}

/// Plain JSON file. Only used where there is no OS vault; the file is created with
/// owner-only permissions on Unix.
pub struct FileStore {
    path: PathBuf,
    map: Mutex<HashMap<String, String>>,
}

impl FileStore {
    pub fn open(path: PathBuf) -> Self {
        let map = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { path, map: Mutex::new(map) }
    }
    fn save(&self, map: &HashMap<String, String>) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(map).unwrap()).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

impl SecretStore for FileStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let mut m = self.map.lock().unwrap();
        m.insert(key.into(), value.into());
        self.save(&m)
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        let mut m = self.map.lock().unwrap();
        if m.remove(key).is_some() {
            self.save(&m)?;
        }
        Ok(())
    }
    fn backend_name(&self) -> &'static str {
        "file"
    }
}

/// Windows Credential Manager / macOS Keychain through the `keyring` crate.
#[cfg(any(windows, target_os = "macos"))]
pub struct NativeStore;

#[cfg(any(windows, target_os = "macos"))]
impl NativeStore {
    const SERVICE: &'static str = "Mochi";
    fn entry(key: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(Self::SERVICE, key).map_err(|e| e.to_string())
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl SecretStore for NativeStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        match Self::entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        Self::entry(key)?.set_password(value).map_err(|e| e.to_string())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
    fn backend_name(&self) -> &'static str {
        if cfg!(windows) { "Windows Credential Manager" } else { "macOS Keychain" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exercise(s: &dyn SecretStore) {
        assert_eq!(s.get("a").unwrap(), None);
        s.set("a", "1").unwrap();
        s.set("b", "two").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("1"));
        s.set("a", "1b").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("1b"));
        s.delete("a").unwrap();
        s.delete("a").unwrap();
        assert_eq!(s.get("a").unwrap(), None);
        assert_eq!(s.get("b").unwrap().as_deref(), Some("two"));
    }

    #[test]
    fn memory_store() {
        exercise(&MemoryStore::default());
    }

    #[test]
    fn file_store_persists() {
        let p = std::env::temp_dir().join(format!("mochi-secrets-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        exercise(&FileStore::open(p.clone()));
        let again = FileStore::open(p.clone());
        assert_eq!(again.get("b").unwrap().as_deref(), Some("two"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn key_names() {
        assert_eq!(keys::phone_secret("ab12"), "phone-secret/ab12");
    }
}
