//! Persisted user settings: which brain, how to reach it, narration default.
//!
//! `settings.json` holds everything except the local-AI API key, which goes to the platform
//! secret store (Windows Credential Manager). A key found inline in an older file is moved
//! there on load and scrubbed on the next save.

use mochi_secrets::{keys, SharedSecrets};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BrainKind {
    ClaudeCode,
    Codex,
    Antigravity,
    Local,
    /// Scripted Gmail demo on a fake screen (phase 1 behaviour).
    Mock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeSettings {
    pub binary: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexSettings {
    pub binary: String,
    pub model: Option<String>,
}

fn default_codex() -> CodexSettings {
    CodexSettings { binary: "codex".into(), model: None }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravitySettings {
    /// `agy` on PATH (winget install Google.AntigravityCLI).
    pub binary: String,
    /// Model slug from `agy models`, or None for the default.
    pub model: Option<String>,
}

fn default_antigravity() -> AntigravitySettings {
    AntigravitySettings { binary: "agy".into(), model: None }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalSettings {
    pub host: String,
    pub port: u16,
    pub model: String,
    pub api_key: Option<String>,
    pub vision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub brain: BrainKind,
    pub claude: ClaudeSettings,
    /// Older settings files predate this field.
    #[serde(default = "default_codex")]
    pub codex: CodexSettings,
    /// Older settings files predate this field.
    #[serde(default = "default_antigravity")]
    pub antigravity: AntigravitySettings,
    pub local: LocalSettings,
    pub narration: bool,
    /// Character size multiplier (0.6–1.2).
    pub scale: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            brain: BrainKind::ClaudeCode,
            claude: ClaudeSettings { binary: "claude".into(), model: Some("sonnet".into()) },
            codex: default_codex(),
            antigravity: default_antigravity(),
            local: LocalSettings { host: "127.0.0.1".into(), port: 11434, model: "llama3.2".into(), api_key: None, vision: false },
            narration: true,
            scale: 0.8,
        }
    }
}

impl Settings {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }
    /// Settings with the API key filled in from the secret store (the returned value is
    /// complete; only the file on disk lacks the key).
    pub fn load(dir: &Path, secrets: &SharedSecrets) -> Self {
        let mut s: Settings = std::fs::read_to_string(Self::path(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        match s.local.api_key.take().filter(|k| !k.is_empty()) {
            // Pre-0.1.15 file with the key inline: move it into the store; the next save scrubs the file.
            Some(inline) => {
                if let Err(e) = secrets.set(keys::LOCAL_API_KEY, &inline) {
                    eprintln!("[mochi] could not move the local API key into the secret store: {e}");
                }
                s.local.api_key = Some(inline);
            }
            None => s.local.api_key = secrets.get(keys::LOCAL_API_KEY).ok().flatten(),
        }
        s
    }
    /// Writes the file without the API key and the key to the secret store.
    pub fn save(&self, dir: &Path, secrets: &SharedSecrets) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let key = self.local.api_key.clone().filter(|k| !k.is_empty());
        let stored = match &key {
            Some(k) => secrets.set(keys::LOCAL_API_KEY, k),
            None => secrets.delete(keys::LOCAL_API_KEY),
        };
        let mut on_disk = self.clone();
        match stored {
            Ok(()) => on_disk.local.api_key = None,
            Err(e) => eprintln!("[mochi] secret store unavailable ({e}); keeping the local API key in settings.json"),
        }
        std::fs::write(Self::path(dir), serde_json::to_string_pretty(&on_disk).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn api_key_lives_in_the_secret_store_not_the_file() {
        let dir = std::env::temp_dir().join(format!("mochi-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let secrets: SharedSecrets = Arc::new(mochi_secrets::MemoryStore::default());
        let mut s = Settings::default();
        s.local.api_key = Some("sk-test".into());
        s.save(&dir, &secrets).unwrap();
        assert!(!std::fs::read_to_string(Settings::path(&dir)).unwrap().contains("sk-test"));
        assert_eq!(Settings::load(&dir, &secrets).local.api_key.as_deref(), Some("sk-test"));
        s.local.api_key = None;
        s.save(&dir, &secrets).unwrap();
        assert_eq!(secrets.get(keys::LOCAL_API_KEY).unwrap(), None);

        // An old file with the key inline is migrated.
        std::fs::write(Settings::path(&dir), serde_json::to_string(&{ let mut o = Settings::default(); o.local.api_key = Some("old-key".into()); o }).unwrap()).unwrap();
        let fresh: SharedSecrets = Arc::new(mochi_secrets::MemoryStore::default());
        let loaded = Settings::load(&dir, &fresh);
        assert_eq!(loaded.local.api_key.as_deref(), Some("old-key"));
        assert_eq!(fresh.get(keys::LOCAL_API_KEY).unwrap().as_deref(), Some("old-key"));
        loaded.save(&dir, &fresh).unwrap();
        assert!(!std::fs::read_to_string(Settings::path(&dir)).unwrap().contains("old-key"), "scrubbed on save");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
