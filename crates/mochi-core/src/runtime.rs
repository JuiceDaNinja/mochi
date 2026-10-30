//! Events the character runtime consumes and the states it can be in.
//! Mirrors the "Character runtime" table in the architecture doc.

use crate::types::Bounds;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterState {
    Idle,
    Listening,
    Thinking,
    Working,
    Success,
    Lost,
    Confirm,
    Learning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoneKind {
    /// Task sent the user's own words somewhere: heart.
    Heart,
    /// Anything else: thumbs up.
    ThumbsUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gesture {
    Click,
    Type,
}

/// Everything the runtime needs to know, and nothing about tasks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CharacterEvent {
    Wake,
    Thinking { on: bool },
    Narrate { text: String },
    MoveTo { bounds: Bounds, monitor_id: u32 },
    Gesture { gesture: Gesture },
    Lost { looking_for: String },
    Learning,
    ConfirmRequest { text: String },
    ConfirmAnswered { yes: bool },
    Done { kind: DoneKind, summary: String },
    /// Mochi says this and returns home (blocked action, brain asleep, interrupted).
    Say { text: String },
    Cancel,
}

/// Mochi's fixed lines. The runtime picks a random one per state; narration is live.
pub mod lines {
    pub const WAKE: &[&str] = &["Mm? I'm here!", "Yes! What should I do?", "Listening…"];
    pub const START: &[&str] = &["Okay! On it!", "Got it, let's go!", "Leave it to Mochi!"];
    pub const AFTER_YES: &[&str] = &["Okay! Sending!", "Here goes!"];
    pub const AFTER_NO: &[&str] = &["Phew, okay. Not sending.", "Got it, leaving it alone."];
    pub const SUCCESS: &[&str] = &["Done!", "All sent! Yay!", "Finished! That was fun.", "Ta-da!"];
    pub const LOST: &[&str] = &[
        "Mochi… can't find it…",
        "Where is it… I looked everywhere…",
        "I'm sorry… can you show me?",
        "It's not here… hmm… hmm…",
    ];
    pub const LEARNING: &[&str] = &["Oh! There it is! Thank you!", "I'll remember that one!", "Got it, got it. Next time I'll know."];
    pub const REMEMBERED: &[&str] = &["I remember this one!"];
    pub const SLEEPY: &[&str] = &["Mochi's a bit sleepy… try again in a moment?", "I can't hear you well right now…"];
    pub const INTERRUPTED: &[&str] = &["Oh! Stopping.", "Okay, I'll wait."];
    pub const TIMEOUT: &[&str] = &["Okay, I'll leave it."];
}
