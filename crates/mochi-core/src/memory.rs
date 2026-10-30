//! The teaching loop's memory: where things are, never what they say.
//! One SQLite file, local only.

use crate::types::{Bounds, Result, ScreenSnapshot, UiElement};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearnedTarget {
    pub id: i64,
    pub app_id: String,
    pub window_match: Option<String>,
    /// The user's words: "the reply button".
    pub user_phrase: String,
    pub ax_label: Option<String>,
    pub ax_role: Option<String>,
    /// Relative to the window, 0..1.
    pub rel_bounds: Bounds,
    pub hits: i64,
    pub misses: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSetting {
    pub app_id: String,
    pub narration_on: bool,
    pub extra_irreversible_labels: Vec<String>,
    pub allowed: bool,
}

pub struct Memory {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS learned_target (
  id            INTEGER PRIMARY KEY,
  app_id        TEXT NOT NULL,
  window_match  TEXT,
  user_phrase   TEXT NOT NULL,
  ax_label      TEXT,
  ax_role       TEXT,
  rel_x REAL NOT NULL, rel_y REAL NOT NULL, rel_w REAL NOT NULL, rel_h REAL NOT NULL,
  thumb_hash    TEXT,
  hits          INTEGER NOT NULL DEFAULT 0,
  misses        INTEGER NOT NULL DEFAULT 0,
  learned_at    TEXT NOT NULL DEFAULT (datetime('now')),
  last_used_at  TEXT
);
CREATE INDEX IF NOT EXISTS learned_target_app ON learned_target(app_id);
CREATE TABLE IF NOT EXISTS app_setting (
  app_id        TEXT PRIMARY KEY,
  narration_on  INTEGER NOT NULL DEFAULT 1,
  extra_irreversible_labels TEXT NOT NULL DEFAULT '[]',
  allowed       INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS gate_log (
  id INTEGER PRIMARY KEY,
  at TEXT NOT NULL DEFAULT (datetime('now')),
  app_id TEXT NOT NULL,
  label TEXT,
  rule TEXT NOT NULL,
  answer TEXT NOT NULL
);
"#;

impl Memory {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Learning: store the element under the user's cursor for the brain's `looking_for` phrase.
    pub fn learn(&self, screen: &ScreenSnapshot, element: &UiElement, user_phrase: &str) -> Result<LearnedTarget> {
        let rel = relative(&element.bounds, &screen.window.bounds);
        let app = screen.window.app_id.to_lowercase();
        let conn = self.conn.lock().unwrap();
        // If this phrase already exists for the app, overwrite its location (a re-teach after a miss).
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM learned_target WHERE app_id=?1 AND user_phrase=?2",
                params![app, user_phrase],
                |r| r.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => {
                conn.execute(
                    "UPDATE learned_target SET ax_label=?1, ax_role=?2, rel_x=?3, rel_y=?4, rel_w=?5, rel_h=?6, misses=misses+1, last_used_at=datetime('now') WHERE id=?7",
                    params![element.label, element.role, rel.x, rel.y, rel.w, rel.h, id],
                )?;
                id
            }
            None => {
                conn.execute(
                    "INSERT INTO learned_target (app_id, window_match, user_phrase, ax_label, ax_role, rel_x, rel_y, rel_w, rel_h) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![app, window_match_for(&screen.window.title), user_phrase, element.label, element.role, rel.x, rel.y, rel.w, rel.h],
                )?;
                conn.last_insert_rowid()
            }
        };
        drop(conn);
        Ok(self.get(id)?.expect("just inserted"))
    }

    pub fn get(&self, id: i64) -> Result<Option<LearnedTarget>> {
        let conn = self.conn.lock().unwrap();
        let t = conn
            .query_row(
                "SELECT id, app_id, window_match, user_phrase, ax_label, ax_role, rel_x, rel_y, rel_w, rel_h, hits, misses FROM learned_target WHERE id=?1",
                params![id],
                row_to_target,
            )
            .optional()?;
        Ok(t)
    }

    /// All targets for an app whose window_match (if any) matches the title.
    pub fn targets_for(&self, app_id: &str, window_title: &str) -> Result<Vec<LearnedTarget>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_id, window_match, user_phrase, ax_label, ax_role, rel_x, rel_y, rel_w, rel_h, hits, misses FROM learned_target WHERE app_id=?1 ORDER BY hits DESC",
        )?;
        let rows = stmt.query_map(params![app_id.to_lowercase()], row_to_target)?;
        let mut out = vec![];
        for r in rows {
            let t = r?;
            let ok = match &t.window_match {
                Some(m) => regex::Regex::new(m).map(|re| re.is_match(window_title)).unwrap_or(true),
                None => true,
            };
            if ok {
                out.push(t);
            }
        }
        Ok(out)
    }

    /// Lookup on each snapshot: match stored targets to current elements, by label+role, then by bounds overlap.
    pub fn resolve(&self, screen: &ScreenSnapshot) -> Result<Vec<(LearnedTarget, u32)>> {
        let targets = self.targets_for(&screen.window.app_id, &screen.window.title)?;
        let mut out = vec![];
        for t in targets {
            let by_label = screen.elements.iter().find(|e| {
                t.ax_label.as_deref().map_or(false, |l| !l.is_empty() && e.label.eq_ignore_ascii_case(l))
                    && t.ax_role.as_deref().map_or(true, |r| e.role.eq_ignore_ascii_case(r))
            });
            let found = by_label.or_else(|| {
                let want = absolute(&t.rel_bounds, &screen.window.bounds);
                screen
                    .elements
                    .iter()
                    .filter(|e| e.interactive)
                    .map(|e| (e, e.bounds.overlap_ratio(&want)))
                    .filter(|(_, r)| *r > 0.4)
                    .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                    .map(|(e, _)| e)
            });
            if let Some(e) = found {
                out.push((t, e.id));
            }
        }
        Ok(out)
    }

    pub fn record_hit(&self, id: i64, screen: &ScreenSnapshot, element: &UiElement) -> Result<()> {
        let rel = relative(&element.bounds, &screen.window.bounds);
        self.conn.lock().unwrap().execute(
            "UPDATE learned_target SET hits=hits+1, rel_x=?1, rel_y=?2, rel_w=?3, rel_h=?4, last_used_at=datetime('now') WHERE id=?5",
            params![rel.x, rel.y, rel.w, rel.h, id],
        )?;
        Ok(())
    }

    pub fn record_miss(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE learned_target SET misses=misses+1 WHERE id=?1", params![id])?;
        // Auto-forget: more misses than hits after 5 uses.
        conn.execute("DELETE FROM learned_target WHERE id=?1 AND hits+misses>=5 AND misses>hits", params![id])?;
        Ok(())
    }

    /// "Forget that": drop the most recently used target.
    pub fn forget_last(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let last: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, user_phrase FROM learned_target ORDER BY COALESCE(last_used_at, learned_at) DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, phrase)) = last {
            conn.execute("DELETE FROM learned_target WHERE id=?1", params![id])?;
            return Ok(Some(phrase));
        }
        Ok(None)
    }

    pub fn forget(&self, id: i64) -> Result<()> {
        self.conn.lock().unwrap().execute("DELETE FROM learned_target WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn wipe(&self) -> Result<()> {
        self.conn.lock().unwrap().execute_batch("DELETE FROM learned_target; DELETE FROM gate_log;")?;
        Ok(())
    }

    pub fn all_targets(&self) -> Result<Vec<LearnedTarget>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_id, window_match, user_phrase, ax_label, ax_role, rel_x, rel_y, rel_w, rel_h, hits, misses FROM learned_target ORDER BY app_id, user_phrase",
        )?;
        let rows = stmt.query_map([], row_to_target)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn app_setting(&self, app_id: &str) -> Result<AppSetting> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(i64, String, i64)> = conn
            .query_row(
                "SELECT narration_on, extra_irreversible_labels, allowed FROM app_setting WHERE app_id=?1",
                params![app_id.to_lowercase()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            Some((n, labels, allowed)) => AppSetting {
                app_id: app_id.to_lowercase(),
                narration_on: n != 0,
                extra_irreversible_labels: serde_json::from_str(&labels).unwrap_or_default(),
                allowed: allowed != 0,
            },
            None => AppSetting {
                app_id: app_id.to_lowercase(),
                narration_on: true,
                extra_irreversible_labels: vec![],
                allowed: !is_sensitive_default(app_id),
            },
        })
    }

    pub fn set_app_setting(&self, s: &AppSetting) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO app_setting (app_id, narration_on, extra_irreversible_labels, allowed) VALUES (?1,?2,?3,?4)
             ON CONFLICT(app_id) DO UPDATE SET narration_on=excluded.narration_on, extra_irreversible_labels=excluded.extra_irreversible_labels, allowed=excluded.allowed",
            params![s.app_id.to_lowercase(), s.narration_on as i64, serde_json::to_string(&s.extra_irreversible_labels).unwrap(), s.allowed as i64],
        )?;
        Ok(())
    }

    pub fn gate_log_count(&self, answer: &str) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM gate_log WHERE answer=?1", params![answer], |r| r.get(0))?)
    }

    pub fn log_gate(&self, app_id: &str, label: &str, rule: &str, answer: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO gate_log (app_id, label, rule, answer) VALUES (?1,?2,?3,?4)",
            params![app_id.to_lowercase(), label, rule, answer],
        )?;
        Ok(())
    }
}

fn row_to_target(r: &rusqlite::Row<'_>) -> rusqlite::Result<LearnedTarget> {
    Ok(LearnedTarget {
        id: r.get(0)?,
        app_id: r.get(1)?,
        window_match: r.get(2)?,
        user_phrase: r.get(3)?,
        ax_label: r.get(4)?,
        ax_role: r.get(5)?,
        rel_bounds: Bounds { x: r.get(6)?, y: r.get(7)?, w: r.get(8)?, h: r.get(9)? },
        hits: r.get(10)?,
        misses: r.get(11)?,
    })
}

fn relative(b: &Bounds, win: &Bounds) -> Bounds {
    let (w, h) = (win.w.max(1.0), win.h.max(1.0));
    Bounds { x: (b.x - win.x) / w, y: (b.y - win.y) / h, w: b.w / w, h: b.h / h }
}

fn absolute(rel: &Bounds, win: &Bounds) -> Bounds {
    Bounds { x: win.x + rel.x * win.w, y: win.y + rel.y * win.h, w: rel.w * win.w, h: rel.h * win.h }
}

/// A loose regex from the window title's app-ish suffix: "Inbox - Gmail - Chrome" -> "Gmail".
fn window_match_for(title: &str) -> Option<String> {
    let parts: Vec<&str> = title.split(" - ").map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        Some(regex::escape(parts[parts.len() - 2]))
    } else {
        None
    }
}

fn is_sensitive_default(app_id: &str) -> bool {
    let a = app_id.to_lowercase();
    ["1password", "bitwarden", "keepass", "lastpass", "dashlane", "authenticator"].iter().any(|s| a.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn screen(elements: Vec<UiElement>) -> ScreenSnapshot {
        ScreenSnapshot {
            window: WindowInfo {
                app_id: "chrome.exe".into(),
                title: "Inbox - Gmail - Google Chrome".into(),
                bounds: Bounds { x: 100.0, y: 50.0, w: 1000.0, h: 800.0 },
                monitor_id: 0,
                unsaved: false,
            },
            monitors: vec![],
            elements,
            remembered: vec![],
            screenshot_png_b64: None,
        }
    }

    fn reply(id: u32, x: f64) -> UiElement {
        UiElement {
            id,
            role: "button".into(),
            label: "Reply".into(),
            value: "".into(),
            bounds: Bounds { x, y: 700.0, w: 80.0, h: 32.0 },
            interactive: true,
            focused: false,
            secure: false,
        }
    }

    #[test]
    fn learns_then_resolves_by_label_and_by_position() {
        let m = Memory::in_memory().unwrap();
        let s = screen(vec![reply(7, 300.0)]);
        let t = m.learn(&s, &s.elements[0], "the reply button").unwrap();
        assert_eq!(t.user_phrase, "the reply button");
        assert_eq!(t.window_match.as_deref(), Some("Gmail"));

        // Same label, new id: resolves by label.
        let s2 = screen(vec![reply(42, 300.0)]);
        let hits = m.resolve(&s2).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, 42);

        // Label gone (icon-only button), same place: resolves by overlap.
        let mut unlabeled = reply(9, 305.0);
        unlabeled.label = "".into();
        let s3 = screen(vec![unlabeled]);
        assert_eq!(m.resolve(&s3).unwrap()[0].1, 9);

        // Different window title (not Gmail): no match.
        let mut s4 = screen(vec![reply(1, 300.0)]);
        s4.window.title = "Calendar - Google Chrome".into();
        assert!(m.resolve(&s4).unwrap().is_empty());
    }

    #[test]
    fn reteach_overwrites_and_auto_forget_after_repeated_misses() {
        let m = Memory::in_memory().unwrap();
        let s = screen(vec![reply(7, 300.0)]);
        let t = m.learn(&s, &s.elements[0], "the reply button").unwrap();
        let moved = screen(vec![reply(8, 900.0)]);
        let t2 = m.learn(&moved, &moved.elements[0], "the reply button").unwrap();
        assert_eq!(t.id, t2.id);
        assert!(t2.rel_bounds.x > t.rel_bounds.x);
        assert_eq!(m.all_targets().unwrap().len(), 1);

        for _ in 0..4 {
            m.record_miss(t.id).unwrap();
        }
        assert!(m.all_targets().unwrap().is_empty(), "misses > hits after 5 uses deletes the row");
    }

    #[test]
    fn forget_last_and_app_settings() {
        let m = Memory::in_memory().unwrap();
        let s = screen(vec![reply(7, 300.0)]);
        m.learn(&s, &s.elements[0], "the reply button").unwrap();
        assert_eq!(m.forget_last().unwrap().as_deref(), Some("the reply button"));
        assert_eq!(m.forget_last().unwrap(), None);

        let d = m.app_setting("Chrome.exe").unwrap();
        assert!(d.narration_on && d.allowed);
        assert!(!m.app_setting("1Password.exe").unwrap().allowed, "password managers are off by default");
        m.set_app_setting(&AppSetting { app_id: "chrome.exe".into(), narration_on: false, extra_irreversible_labels: vec!["Archive".into()], allowed: true }).unwrap();
        let s = m.app_setting("chrome.exe").unwrap();
        assert!(!s.narration_on);
        assert_eq!(s.extra_irreversible_labels, vec!["Archive".to_string()]);
    }
}
