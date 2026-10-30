//! Phone <-> desktop messages. One JSON object per WebSocket frame, max 64 KB.

use crate::runtime::CharacterState;
use serde::{Deserialize, Serialize};

pub const SERVICE_TYPE: &str = "_mochi._tcp.local.";
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const PING_INTERVAL_SECS: u64 = 5;
pub const MISSED_PINGS_BEFORE_RECONNECT: u32 = 3;
pub const CONFIRM_TIMEOUT_SECS: u64 = 20;
/// Reconnect backoff after the network drops; the last value repeats.
pub const RECONNECT_BACKOFF_SECS: &[u64] = &[1, 2, 5, 10];

/// Phone -> desktop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PhoneMessage {
    /// First frame on a fresh phone. Both devices then show the same 6-digit code
    /// and both must approve before `Paired` is sent.
    PairRequest { device_name: String, public_key_b64: String },
    /// The phone's user tapped Approve.
    PairApprove,
    /// Later sessions: prove possession of the shared key.
    Auth { device_id: String, hmac_b64: String },
    Wake,
    Utterance { text: String, #[serde(rename = "final")] is_final: bool },
    /// Voice activity started; the desktop stops talking within 100 ms.
    UserSpeaking,
    Answer { yes: bool },
    Stop,
    Pong,
}

/// Desktop -> phone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DesktopMessage {
    /// Shown on the phone alongside the desktop's name while both sides approve.
    PairChallenge { desktop_name: String, code: String, nonce_b64: String },
    Paired { device_id: String },
    PairDenied,
    AuthOk,
    AuthFailed,
    State { state: CharacterState, #[serde(default, skip_serializing_if = "Option::is_none")] line: Option<String> },
    Narrate { text: String, #[serde(default, skip_serializing_if = "Option::is_none")] audio_url: Option<String> },
    ConfirmRequest { text: String },
    Done { summary: String },
    /// The desktop is closing or the user ended the session; the phone shows Mochi asleep.
    Sleepy,
    Ping,
}

/// Six digits derived deterministically from the pairing nonce so both screens agree.
pub fn pairing_code(nonce: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in nonce {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:06}", h % 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_with_snake_case_tags() {
        let m = PhoneMessage::Utterance { text: "reply to Sara".into(), is_final: true };
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(s, r#"{"type":"utterance","text":"reply to Sara","final":true}"#);
        assert_eq!(serde_json::from_str::<PhoneMessage>(&s).unwrap(), m);

        let d = DesktopMessage::State { state: CharacterState::Confirm, line: Some("Send it?".into()) };
        assert_eq!(serde_json::to_string(&d).unwrap(), r#"{"type":"state","state":"confirm","line":"Send it?"}"#);
    }

    #[test]
    fn pairing_code_is_six_digits_and_stable() {
        let a = pairing_code(b"nonce-1");
        assert_eq!(a.len(), 6);
        assert_eq!(a, pairing_code(b"nonce-1"));
        assert_ne!(a, pairing_code(b"nonce-2"));
    }
}
