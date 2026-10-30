//! Shared by every brain adapter: how a [`BrainInput`] is rendered into a user message,
//! and how model text is parsed back into [`StepEvent`]s. Keeping both here means the
//! three brains see the same screen and are held to the same output contract.

use crate::brain::BrainInput;
use crate::types::{Action, StepEvent, UiElement};

pub const MAX_ELEMENTS: usize = 300;
pub const MAX_TEXT: usize = 80;

/// Compact, line-per-element rendering. Interactive elements first, text truncated.
pub fn render_user_message(input: &BrainInput) -> String {
    let s = &input.screen;
    let mut out = String::new();
    out.push_str(&format!("TASK: {}\n\n", input.task.trim()));
    out.push_str(&format!(
        "WINDOW: app={} title={:?} bounds=({:.0},{:.0},{:.0}x{:.0}) unsaved={}\n",
        s.window.app_id, s.window.title, s.window.bounds.x, s.window.bounds.y, s.window.bounds.w, s.window.bounds.h, s.window.unsaved
    ));
    if !s.remembered.is_empty() {
        out.push_str("REMEMBERED (the user taught me these earlier; prefer them):\n");
        for h in &s.remembered {
            out.push_str(&format!("  \"{}\" -> #{}\n", h.phrase, h.element_id));
        }
    }
    out.push_str("ELEMENTS (#id role \"label\" [value] @x,y wxh flags):\n");
    let mut els: Vec<&UiElement> = s.elements.iter().collect();
    els.sort_by_key(|e| (!e.interactive, !e.focused));
    for e in els.iter().take(MAX_ELEMENTS) {
        let mut flags = vec![];
        if e.interactive { flags.push("clickable"); }
        if e.focused { flags.push("focused"); }
        if e.secure { flags.push("secure"); }
        out.push_str(&format!(
            "  #{} {} \"{}\"{} @{:.0},{:.0} {:.0}x{:.0}{}\n",
            e.id,
            e.role,
            trunc(&e.label),
            if e.value.is_empty() { String::new() } else { format!(" [{}]", trunc(&e.value)) },
            e.bounds.x, e.bounds.y, e.bounds.w, e.bounds.h,
            if flags.is_empty() { String::new() } else { format!(" ({})", flags.join(",")) }
        ));
    }
    if s.elements.len() > MAX_ELEMENTS {
        out.push_str(&format!("  … {} more not shown\n", s.elements.len() - MAX_ELEMENTS));
    }
    if s.screenshot_png_b64.is_some() {
        out.push_str("A screenshot of the window is attached.\n");
    }
    if !input.history.is_empty() {
        out.push_str("\nSTEPS SO FAR:\n");
        for (i, st) in input.history.iter().enumerate() {
            out.push_str(&format!("  {}. {} -> {}{}\n", i + 1, describe_action(&st.action), if st.ok { "ok" } else { "FAILED" }, if st.note.is_empty() { String::new() } else { format!(" ({})", st.note) }));
        }
    }
    out.push_str("\nReturn the next event as one JSON object (narrate first if a new visible step begins, then exactly one act/lost/done).");
    out
}

pub fn describe_action(a: &Action) -> String {
    match a {
        Action::FocusWindow { app_id, title_contains } => format!("focus_window {app_id} {}", title_contains.clone().unwrap_or_default()),
        Action::ClickElement { element_id } => format!("click #{element_id}"),
        Action::ClickPoint { x, y } => format!("click @{x:.0},{y:.0}"),
        Action::Type { text } => format!("type {:?}", trunc(text)),
        Action::Key { combo } => format!("key {combo}"),
        Action::Scroll { element_id, direction, amount } => format!("scroll #{element_id} {direction:?} x{amount}"),
    }
}

fn trunc(s: &str) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= MAX_TEXT { s } else { format!("{}…", s.chars().take(MAX_TEXT).collect::<String>()) }
}

/// Pull every JSON object out of model text (code fences, prose around it, several per
/// message) and parse the ones that are StepEvents. Unknown objects are skipped.
pub fn parse_step_events(text: &str) -> Vec<StepEvent> {
    let mut out = vec![];
    for obj in json_objects(text) {
        if let Ok(ev) = serde_json::from_str::<StepEvent>(&obj) {
            out.push(ev);
        }
    }
    out
}

/// Scan for balanced top-level `{ … }` spans, respecting strings.
fn json_objects(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut spans = vec![];
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if esc { esc = false; } else if b == b'\\' { esc = true; } else if b == b'"' { in_str = false; }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => { if depth == 0 { start = i; } depth += 1; }
            b'}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 { spans.push(text[start..=i].to_string()); }
                }
            }
            _ => {}
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    #[test]
    fn parses_fenced_and_multiple_objects_and_ignores_junk() {
        let text = "Sure! Here you go:\n```json\n{\"kind\":\"narrate\",\"text\":\"Opening Gmail…\"}\n```\nthen {\"kind\":\"act\",\"action\":{\"type\":\"click_element\",\"element_id\":4},\"irreversible\":false} and {\"not\":\"an event\"} and a brace in a string: {\"kind\":\"narrate\",\"text\":\"curly } inside\"}";
        let evs = parse_step_events(text);
        assert_eq!(evs.len(), 3);
        assert!(matches!(&evs[0], StepEvent::Narrate { text } if text == "Opening Gmail…"));
        assert!(matches!(&evs[1], StepEvent::Act { action: Action::ClickElement { element_id: 4 }, irreversible: false }));
        assert!(matches!(&evs[2], StepEvent::Narrate { text } if text == "curly } inside"));
    }

    #[test]
    fn renders_interactive_first_with_remembered_and_history() {
        let screen = ScreenSnapshot {
            window: WindowInfo { app_id: "chrome.exe".into(), title: "Inbox - Gmail".into(), bounds: Bounds { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }, monitor_id: 0, unsaved: false },
            monitors: vec![],
            elements: vec![
                UiElement { id: 1, role: "text".into(), label: "Hello".into(), value: "".into(), bounds: Bounds::default(), interactive: false, focused: false, secure: false },
                UiElement { id: 2, role: "button".into(), label: "Reply".into(), value: "".into(), bounds: Bounds { x: 10.0, y: 20.0, w: 80.0, h: 30.0 }, interactive: true, focused: false, secure: false },
            ],
            remembered: vec![RememberedHint { phrase: "the reply button".into(), element_id: 2 }],
            screenshot_png_b64: None,
        };
        let input = BrainInput { task: "reply to Sara".into(), screen, memory: vec![], history: vec![Step { action: Action::ClickElement { element_id: 2 }, ok: true, note: "".into() }] };
        let m = render_user_message(&input);
        let reply_pos = m.find("#2 button").unwrap();
        let text_pos = m.find("#1 text").unwrap();
        assert!(reply_pos < text_pos, "interactive elements come first");
        assert!(m.contains("\"the reply button\" -> #2"));
        assert!(m.contains("1. click #2 -> ok"));
        assert!(m.contains("(clickable)"));
    }
}
