//! Shared data types: what Mochi sees, what Mochi can do, what the brain says back.

use serde::{Deserialize, Serialize};

/// Global screen coordinates in physical pixels (already offset per monitor).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Bounds {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
    pub fn overlap_ratio(&self, other: &Bounds) -> f64 {
        let ix = (self.x + self.w).min(other.x + other.w) - self.x.max(other.x);
        let iy = (self.y + self.h).min(other.y + other.h) - self.y.max(other.y);
        if ix <= 0.0 || iy <= 0.0 {
            return 0.0;
        }
        let inter = ix * iy;
        let union = self.w * self.h + other.w * other.h - inter;
        if union <= 0.0 {
            0.0
        } else {
            inter / union
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub bounds: Bounds,
    pub scale: f64,
    pub is_primary: bool,
    pub has_cursor: bool,
}

/// One node of the trimmed accessibility tree sent to the brain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiElement {
    /// Stable within one snapshot only. The brain refers to elements by this id.
    pub id: u32,
    pub role: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub value: String,
    pub bounds: Bounds,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub secure: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub app_id: String,
    pub title: String,
    pub bounds: Bounds,
    pub monitor_id: u32,
    /// Set when the OS reports unsaved changes (modified flag, `*` or `•` in the title).
    #[serde(default)]
    pub unsaved: bool,
}

/// A learned target that matched something in the current snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RememberedHint {
    pub phrase: String,
    pub element_id: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    pub window: WindowInfo,
    pub monitors: Vec<MonitorInfo>,
    pub elements: Vec<UiElement>,
    #[serde(default)]
    pub remembered: Vec<RememberedHint>,
    /// PNG bytes, base64, present only when the tree was insufficient or requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_png_b64: Option<String>,
}

impl ScreenSnapshot {
    pub fn element(&self, id: u32) -> Option<&UiElement> {
        self.elements.iter().find(|e| e.id == id)
    }
}

/// Mouse / keyboard actions the brain can request. Exactly one per cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    FocusWindow { app_id: String, #[serde(default)] title_contains: Option<String> },
    ClickElement { element_id: u32 },
    ClickPoint { x: f64, y: f64 },
    Type { text: String },
    Key { combo: String },
    Scroll { element_id: u32, direction: ScrollDirection, #[serde(default = "default_scroll")] amount: u32 },
}

fn default_scroll() -> u32 {
    3
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

/// What the brain streams back. The engine turns these into character events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepEvent {
    /// "Opening Gmail…" — spoken by the runtime if narration is on.
    Narrate { text: String },
    /// One action. `irreversible` is a hint; it can add caution, never remove it.
    Act { action: Action, #[serde(default)] irreversible: bool },
    /// Could not find what it needs. Triggers the Lost state and the teaching loop.
    Lost { looking_for: String },
    /// Task finished. `sent_user_words` picks heart (true) or thumbs up (false).
    Done { summary: String, #[serde(default)] sent_user_words: bool },
    /// Rare: a clarifying question. Treated like Confirm but with free-text answer.
    Ask { question: String },
}

/// A step the engine already executed, fed back to the brain as history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub action: Action,
    pub ok: bool,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MochiError {
    #[error("brain unreachable: {0}")]
    BrainUnreachable(String),
    #[error("brain returned malformed output: {0}")]
    BrainProtocol(String),
    #[error("perception failed: {0}")]
    Perception(String),
    #[error("action failed: {0}")]
    Action(String),
    #[error("memory: {0}")]
    Memory(#[from] rusqlite::Error),
    #[error("task cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, MochiError>;
