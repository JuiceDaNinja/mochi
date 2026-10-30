//! Persisted user settings: which brain, how to reach it, narration default.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BrainKind {
    ClaudeCode,
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
    pub local: LocalSettings,
    pub narration: bool,
    /// Character size multiplier (0.6–1.2).
    pub scale: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            brain: BrainKind::ClaudeCode,
            claude: ClaudeSettings { binary: "claude".into(), model: None },
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
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(Self::path(dir), serde_json::to_string_pretty(self).unwrap())
    }
}
