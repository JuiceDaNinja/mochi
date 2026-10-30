//! The safety gate. Lives in the engine, not in any brain, so no prompt can skip it.
//! Classifies every action as safe or irreversible before it is executed.

use crate::types::{Action, ScreenSnapshot};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum GateVerdict {
    Safe,
    /// Needs a yes. `description` is what Mochi says: "Send it?", "Close without saving?".
    Irreversible { description: String, rule: String },
    /// Never allowed, whatever the user says. `reason` is spoken in Mochi's words.
    Blocked { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRules {
    /// Element labels (case-insensitive, whole word) that mean "cannot be undone".
    pub irreversible_labels: Vec<String>,
    /// Per-app extra labels, keyed by app id (e.g. "chrome.exe", "outlook.exe").
    #[serde(default)]
    pub app_labels: HashMap<String, Vec<String>>,
    /// Key combos that submit things.
    pub irreversible_keys: Vec<String>,
    /// Key combos that close things (irreversible only when the window is unsaved).
    pub close_keys: Vec<String>,
}

impl Default for GateRules {
    fn default() -> Self {
        Self {
            irreversible_labels: [
                "send", "post", "publish", "submit", "delete", "remove", "discard", "pay", "buy",
                "purchase", "confirm", "transfer", "tweet", "reply all", "share", "unsubscribe",
                "empty trash", "move to trash", "permanently delete", "place order", "checkout",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            app_labels: HashMap::from([
                ("terminal".to_string(), vec!["enter".to_string()]),
                ("windowsterminal.exe".to_string(), vec!["enter".to_string()]),
            ]),
            irreversible_keys: ["ctrl+enter", "cmd+enter", "ctrl+shift+enter"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            close_keys: ["alt+f4", "ctrl+w", "cmd+w", "cmd+q"].iter().map(|s| s.to_string()).collect(),
        }
    }
}

pub struct SafetyGate {
    rules: GateRules,
    patterns: Vec<(Regex, String)>,
}

impl SafetyGate {
    pub fn new(rules: GateRules) -> Self {
        let patterns = rules
            .irreversible_labels
            .iter()
            .map(|l| (word_regex(l), l.clone()))
            .collect();
        Self { rules, patterns }
    }

    pub fn with_defaults() -> Self {
        Self::new(GateRules::default())
    }

    /// Add labels the user marked as irreversible for one app (from app_setting).
    pub fn add_app_labels(&mut self, app_id: &str, labels: &[String]) {
        self.rules
            .app_labels
            .entry(app_id.to_lowercase())
            .or_default()
            .extend(labels.iter().cloned());
    }

    pub fn classify(&self, action: &Action, screen: &ScreenSnapshot, brain_hint: bool) -> GateVerdict {
        let app = screen.window.app_id.to_lowercase();
        let verdict = match action {
            Action::Type { .. } => {
                if let Some(el) = screen.elements.iter().find(|e| e.focused) {
                    if el.secure || is_password_role(&el.role) {
                        return GateVerdict::Blocked {
                            reason: "That's a password box. Mochi can't type in those.".into(),
                        };
                    }
                }
                GateVerdict::Safe
            }
            Action::ClickElement { element_id } => match screen.element(*element_id) {
                Some(el) => self.classify_label(&app, &el.label, &el.role),
                None => GateVerdict::Safe,
            },
            Action::ClickPoint { x, y } => {
                // Resolve the point to an element if we can; else trust the hint only.
                match screen.elements.iter().filter(|e| e.bounds.contains(*x, *y)).min_by(|a, b| {
                    (a.bounds.w * a.bounds.h).partial_cmp(&(b.bounds.w * b.bounds.h)).unwrap()
                }) {
                    Some(el) => self.classify_label(&app, &el.label, &el.role),
                    None => GateVerdict::Safe,
                }
            }
            Action::Key { combo } => {
                let c = combo.to_lowercase();
                if self.rules.irreversible_keys.iter().any(|k| *k == c) {
                    GateVerdict::Irreversible { description: "Send it?".into(), rule: format!("key:{c}") }
                } else if self.rules.close_keys.iter().any(|k| *k == c) && screen.window.unsaved {
                    GateVerdict::Irreversible {
                        description: "This one closes without saving… okay?".into(),
                        rule: format!("close-unsaved:{c}"),
                    }
                } else if c == "enter" && self.rules.app_labels.get(&app).map_or(false, |v| v.iter().any(|l| l == "enter")) {
                    GateVerdict::Irreversible { description: "Run it?".into(), rule: format!("app:{app}:enter") }
                } else {
                    GateVerdict::Safe
                }
            }
            Action::FocusWindow { .. } | Action::Scroll { .. } => GateVerdict::Safe,
        };

        // The brain's hint can only add caution.
        match (&verdict, brain_hint) {
            (GateVerdict::Safe, true) => GateVerdict::Irreversible {
                description: "Are you sure?".into(),
                rule: "brain-hint".into(),
            },
            _ => verdict,
        }
    }

    fn classify_label(&self, app: &str, label: &str, role: &str) -> GateVerdict {
        let l = label.trim();
        if l.is_empty() {
            return GateVerdict::Safe;
        }
        if let Some(extra) = self.rules.app_labels.get(app) {
            for x in extra {
                if word_regex(x).is_match(l) {
                    return GateVerdict::Irreversible { description: describe(l), rule: format!("app:{app}:{x}") };
                }
            }
        }
        for (re, name) in &self.patterns {
            if re.is_match(l) {
                return GateVerdict::Irreversible { description: describe(l), rule: format!("label:{name}") };
            }
        }
        if role.eq_ignore_ascii_case("submit") || role.eq_ignore_ascii_case("submitbutton") {
            return GateVerdict::Irreversible { description: describe(l), rule: "role:submit".into() };
        }
        GateVerdict::Safe
    }
}

fn is_password_role(role: &str) -> bool {
    let r = role.to_lowercase();
    r.contains("password") || r.contains("secure")
}

fn word_regex(label: &str) -> Regex {
    Regex::new(&format!(r"(?i)\b{}\b", regex::escape(label))).expect("static label regex")
}

/// Turn a button label into Mochi's confirm line: "Send" -> "Send it?", "Delete" -> "Delete it? Really really?"
fn describe(label: &str) -> String {
    let l = label.trim().to_lowercase();
    let verb = l.split_whitespace().next().unwrap_or("do").to_string();
    match verb.as_str() {
        "delete" | "remove" | "discard" => format!("{} it? Really really?", cap(&verb)),
        "post" | "publish" | "share" | "tweet" => "Posting this for everyone to see. Sure?".to_string(),
        "pay" | "buy" | "purchase" | "checkout" | "place" => "This spends money. Are you sure?".to_string(),
        _ => format!("{} it?", cap(&verb)),
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn snap(elements: Vec<UiElement>, unsaved: bool) -> ScreenSnapshot {
        ScreenSnapshot {
            window: WindowInfo {
                app_id: "chrome.exe".into(),
                title: "Inbox - Gmail".into(),
                bounds: Bounds { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 },
                monitor_id: 0,
                unsaved,
            },
            monitors: vec![],
            elements,
            remembered: vec![],
            screenshot_png_b64: None,
        }
    }

    fn el(id: u32, role: &str, label: &str) -> UiElement {
        UiElement {
            id,
            role: role.into(),
            label: label.into(),
            value: "".into(),
            bounds: Bounds { x: 10.0 * id as f64, y: 10.0, w: 50.0, h: 20.0 },
            interactive: true,
            focused: false,
            secure: false,
        }
    }

    #[test]
    fn send_button_is_irreversible_with_spoken_line() {
        let g = SafetyGate::with_defaults();
        let s = snap(vec![el(1, "button", "Send"), el(2, "button", "Reply")], false);
        assert_eq!(
            g.classify(&Action::ClickElement { element_id: 1 }, &s, false),
            GateVerdict::Irreversible { description: "Send it?".into(), rule: "label:send".into() }
        );
        assert_eq!(g.classify(&Action::ClickElement { element_id: 2 }, &s, false), GateVerdict::Safe);
    }

    #[test]
    fn label_match_is_whole_word() {
        let g = SafetyGate::with_defaults();
        let s = snap(vec![el(1, "button", "Sender filters"), el(2, "button", "Delete forever")], false);
        assert_eq!(g.classify(&Action::ClickElement { element_id: 1 }, &s, false), GateVerdict::Safe);
        assert!(matches!(g.classify(&Action::ClickElement { element_id: 2 }, &s, false), GateVerdict::Irreversible { .. }));
    }

    #[test]
    fn closing_unsaved_window_needs_confirm_but_saved_does_not() {
        let g = SafetyGate::with_defaults();
        let key = Action::Key { combo: "Alt+F4".into() };
        assert!(matches!(g.classify(&key, &snap(vec![], true), false), GateVerdict::Irreversible { .. }));
        assert_eq!(g.classify(&key, &snap(vec![], false), false), GateVerdict::Safe);
    }

    #[test]
    fn brain_hint_adds_caution_but_cannot_remove_it() {
        let g = SafetyGate::with_defaults();
        let s = snap(vec![el(1, "button", "Archive"), el(2, "button", "Send")], false);
        assert!(matches!(g.classify(&Action::ClickElement { element_id: 1 }, &s, true), GateVerdict::Irreversible { .. }));
        // hint=false on Send still irreversible
        assert!(matches!(g.classify(&Action::ClickElement { element_id: 2 }, &s, false), GateVerdict::Irreversible { .. }));
    }

    #[test]
    fn typing_into_password_field_is_blocked() {
        let g = SafetyGate::with_defaults();
        let mut pw = el(1, "edit", "Password");
        pw.focused = true;
        pw.secure = true;
        let s = snap(vec![pw], false);
        assert!(matches!(g.classify(&Action::Type { text: "hunter2".into() }, &s, false), GateVerdict::Blocked { .. }));
    }

    #[test]
    fn click_point_resolves_to_smallest_element_under_it() {
        let g = SafetyGate::with_defaults();
        let mut big = el(1, "pane", "Compose");
        big.bounds = Bounds { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let mut send = el(2, "button", "Send");
        send.bounds = Bounds { x: 100.0, y: 500.0, w: 80.0, h: 30.0 };
        let s = snap(vec![big, send], false);
        assert!(matches!(g.classify(&Action::ClickPoint { x: 120.0, y: 510.0 }, &s, false), GateVerdict::Irreversible { .. }));
        assert_eq!(g.classify(&Action::ClickPoint { x: 10.0, y: 10.0 }, &s, false), GateVerdict::Safe);
    }

    #[test]
    fn user_added_app_labels_apply() {
        let mut g = SafetyGate::with_defaults();
        g.add_app_labels("chrome.exe", &["Archive".to_string()]);
        let s = snap(vec![el(1, "button", "Archive")], false);
        assert!(matches!(g.classify(&Action::ClickElement { element_id: 1 }, &s, false), GateVerdict::Irreversible { .. }));
    }
}
