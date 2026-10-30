//! Platform-independent helpers for the Windows implementation (tested everywhere).

use mochi_core::types::{Bounds, UiElement};

/// UI Automation control type ids → Mochi role names.
pub fn role_name(control_type: i32) -> &'static str {
    match control_type {
        50000 => "button",
        50001 => "calendar",
        50002 => "checkbox",
        50003 => "combobox",
        50004 => "edit",
        50005 => "link",
        50006 => "image",
        50007 => "listitem",
        50008 => "list",
        50009 => "menu",
        50010 => "menubar",
        50011 => "menuitem",
        50012 => "progressbar",
        50013 => "radio",
        50014 => "scrollbar",
        50015 => "slider",
        50016 => "spinner",
        50017 => "statusbar",
        50018 => "tab",
        50019 => "tabitem",
        50020 => "text",
        50021 => "toolbar",
        50022 => "tooltip",
        50023 => "tree",
        50024 => "treeitem",
        50025 => "custom",
        50026 => "group",
        50027 => "thumb",
        50028 => "datagrid",
        50029 => "dataitem",
        50030 => "document",
        50031 => "splitbutton",
        50032 => "window",
        50033 => "pane",
        50034 => "header",
        50035 => "headeritem",
        50036 => "table",
        50037 => "titlebar",
        50038 => "separator",
        50039 => "semanticzoom",
        50040 => "appbar",
        _ => "unknown",
    }
}

pub fn role_is_interactive(role: &str) -> bool {
    matches!(
        role,
        "button" | "checkbox" | "combobox" | "edit" | "link" | "listitem" | "menuitem" | "radio" | "slider" | "spinner"
            | "tabitem" | "treeitem" | "splitbutton" | "dataitem" | "document" | "hyperlink"
    )
}

/// Roles whose text value is worth sending (fields the brain may need to read or type into).
pub fn role_has_value(role: &str) -> bool {
    matches!(role, "edit" | "combobox" | "document" | "spinner" | "slider")
}

/// Windows apps mark unsaved documents in the title: "*notes.txt - Notepad", "• Untitled".
pub fn unsaved_from_title(title: &str) -> bool {
    let t = title.trim_start();
    t.starts_with('*') || t.starts_with('•') || t.starts_with("● ") || t.to_lowercase().contains("(modified)") || t.to_lowercase().contains("(unsaved)")
}

/// "chrome.exe" from a full image path; lowercase.
pub fn app_id_from_path(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

/// Does a user-given app id ("chrome", "Chrome.exe", "google chrome") match an exe name?
pub fn app_matches(wanted: &str, exe: &str, title: &str) -> bool {
    let w = wanted.trim().to_lowercase();
    if w.is_empty() {
        return true;
    }
    let exe = exe.to_lowercase();
    let stem = exe.strip_suffix(".exe").unwrap_or(&exe);
    let w_stem = w.strip_suffix(".exe").unwrap_or(&w);
    exe == w || stem == w_stem || title.to_lowercase().contains(w_stem) || stem.contains(w_stem) || w_stem.split_whitespace().last().map_or(false, |last| stem == last)
}

/// Keep the tree small and useful: drop invisible/empty, interactive and focused first, cap count.
pub fn trim_elements(mut els: Vec<UiElement>, cap: usize) -> Vec<UiElement> {
    els.retain(|e| e.bounds.w >= 1.0 && e.bounds.h >= 1.0 && (e.interactive || !e.label.is_empty() || !e.value.is_empty()));
    els.sort_by_key(|e| (!e.focused, !e.interactive));
    els.truncate(cap);
    // Re-sort by position for a readable listing (top-to-bottom, left-to-right), ids stay stable.
    els.sort_by(|a, b| (a.bounds.y as i64 / 24, a.bounds.x as i64).cmp(&(b.bounds.y as i64 / 24, b.bounds.x as i64)));
    els
}

/// One key chord like "ctrl+shift+t". Returns (modifiers as VK codes, main key as a `KeySpec`).
#[derive(Debug, PartialEq, Clone)]
pub enum KeySpec {
    /// A virtual-key code.
    Vk(u16),
    /// A printable character, looked up with VkKeyScan at press time.
    Char(char),
}

pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_LWIN: u16 = 0x5B;

pub fn parse_combo(combo: &str) -> Option<(Vec<u16>, KeySpec)> {
    let parts: Vec<&str> = combo.split(['+', '-']).map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let mut mods = vec![];
    for p in &parts[..parts.len() - 1] {
        mods.push(match p.to_lowercase().as_str() {
            "ctrl" | "control" | "cmd" => VK_CONTROL,
            "shift" => VK_SHIFT,
            "alt" | "option" | "menu" => VK_MENU,
            "win" | "super" | "meta" => VK_LWIN,
            _ => return None,
        });
    }
    let last = parts[parts.len() - 1];
    let key = match last.to_lowercase().as_str() {
        "enter" | "return" => KeySpec::Vk(0x0D),
        "tab" => KeySpec::Vk(0x09),
        "esc" | "escape" => KeySpec::Vk(0x1B),
        "space" => KeySpec::Vk(0x20),
        "backspace" => KeySpec::Vk(0x08),
        "delete" | "del" => KeySpec::Vk(0x2E),
        "insert" => KeySpec::Vk(0x2D),
        "home" => KeySpec::Vk(0x24),
        "end" => KeySpec::Vk(0x23),
        "pageup" | "pgup" => KeySpec::Vk(0x21),
        "pagedown" | "pgdn" => KeySpec::Vk(0x22),
        "up" => KeySpec::Vk(0x26),
        "down" => KeySpec::Vk(0x28),
        "left" => KeySpec::Vk(0x25),
        "right" => KeySpec::Vk(0x27),
        "win" | "super" | "meta" => KeySpec::Vk(VK_LWIN),
        f if f.starts_with('f') && f[1..].parse::<u8>().map_or(false, |n| (1..=24).contains(&n)) => KeySpec::Vk(0x70 + f[1..].parse::<u16>().unwrap() - 1),
        s if s.chars().count() == 1 => KeySpec::Char(last.chars().next().unwrap()),
        _ => return None,
    };
    Some((mods, key))
}

/// Screen pixel → SendInput absolute coordinate (0..65535 across the virtual desktop).
pub fn to_absolute(px: f64, origin: i32, extent: i32) -> i32 {
    let extent = extent.max(1) as f64;
    (((px - origin as f64) * 65535.0) / extent).round().clamp(0.0, 65535.0) as i32
}

/// Nearest-neighbour downscale of BGRA rows to at most `max_w` wide; returns (w, h, rgba).
pub fn downscale_bgra_to_rgba(src: &[u8], w: usize, h: usize, max_w: usize) -> (usize, usize, Vec<u8>) {
    let scale = if w > max_w { max_w as f64 / w as f64 } else { 1.0 };
    let nw = ((w as f64) * scale).round().max(1.0) as usize;
    let nh = ((h as f64) * scale).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(nw * nh * 4);
    for y in 0..nh {
        let sy = ((y as f64 / scale) as usize).min(h - 1);
        for x in 0..nw {
            let sx = ((x as f64 / scale) as usize).min(w - 1);
            let i = (sy * w + sx) * 4;
            out.extend_from_slice(&[src[i + 2], src[i + 1], src[i], 255]);
        }
    }
    (nw, nh, out)
}

pub fn rect_to_bounds(l: i32, t: i32, r: i32, b: i32) -> Bounds {
    Bounds { x: l as f64, y: t as f64, w: (r - l).max(0) as f64, h: (b - t).max(0) as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_parse() {
        assert_eq!(parse_combo("ctrl+enter"), Some((vec![VK_CONTROL], KeySpec::Vk(0x0D))));
        assert_eq!(parse_combo("Ctrl+Shift+T"), Some((vec![VK_CONTROL, VK_SHIFT], KeySpec::Char('T'))));
        assert_eq!(parse_combo("alt+f4"), Some((vec![VK_MENU], KeySpec::Vk(0x73))));
        assert_eq!(parse_combo("enter"), Some((vec![], KeySpec::Vk(0x0D))));
        assert_eq!(parse_combo("bogus+x"), None);
        assert_eq!(parse_combo(""), None);
    }

    #[test]
    fn titles_and_app_ids() {
        assert!(unsaved_from_title("*notes.txt - Notepad"));
        assert!(unsaved_from_title("• Untitled - Word"));
        assert!(!unsaved_from_title("notes.txt - Notepad"));
        assert_eq!(app_id_from_path(r"C:\Program Files\Google\Chrome\Application\Chrome.EXE"), "chrome.exe");
        assert!(app_matches("chrome", "chrome.exe", ""));
        assert!(app_matches("Chrome.exe", "chrome.exe", ""));
        assert!(app_matches("google chrome", "chrome.exe", ""));
        assert!(app_matches("gmail", "chrome.exe", "Inbox - Gmail - Google Chrome"));
        assert!(!app_matches("firefox", "chrome.exe", "Inbox"));
    }

    #[test]
    fn absolute_coords_span_the_virtual_desktop() {
        assert_eq!(to_absolute(0.0, 0, 1920), 0);
        assert_eq!(to_absolute(1920.0, 0, 1920), 65535);
        assert_eq!(to_absolute(-960.0, -1920, 3840), 16384);
    }

    #[test]
    fn trim_keeps_focused_and_interactive_first_then_caps() {
        let mk = |id, role: &str, label: &str, interactive, focused, y| UiElement { id, role: role.into(), label: label.into(), value: "".into(), bounds: Bounds { x: 0.0, y, w: 10.0, h: 10.0 }, interactive, focused, secure: false };
        let els = vec![
            mk(1, "text", "", false, false, 0.0),     // dropped: no label, not interactive
            mk(2, "text", "Hello", false, false, 0.0),
            mk(3, "button", "Send", true, false, 100.0),
            mk(4, "edit", "Body", true, true, 50.0),
            mk(5, "text", "Footer", false, false, 200.0),
        ];
        let t = trim_elements(els, 3);
        let ids: Vec<u32> = t.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![2, 4, 3], "focused + interactive survive the cap; listed top to bottom");
    }

    #[test]
    fn downscale_swaps_bgra_to_rgba() {
        let src = vec![1, 2, 3, 0, 4, 5, 6, 0];
        let (w, h, out) = downscale_bgra_to_rgba(&src, 2, 1, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, vec![3, 2, 1, 255]);
    }
}
