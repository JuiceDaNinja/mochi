//! Claude Code CLI adapter.
//!
//! Each engine cycle spawns `claude -p` in headless mode with the user message on stdin as
//! stream-json (so a screenshot can ride along as an image block) and reads stream-json
//! back. The first cycle pins a session id; later cycles `--resume` it, so Claude keeps
//! the conversation and the engine only has to send what changed.
//!
//! Flags verified against Claude Code 2.1: `-p --input-format stream-json
//! --output-format stream-json --verbose --tools "" --max-turns 1 --system-prompt …
//! --session-id <uuid> | --resume <uuid>`.

use async_trait::async_trait;
use mochi_core::brain::SYSTEM_PROMPT;
use mochi_core::prompt::{parse_step_events, render_user_message};
use mochi_core::*;
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    /// Path or name of the CLI. Default "claude" (on PATH). On Windows this is usually
    /// `claude.cmd` from the npm global bin, which `Command` resolves via PATHEXT.
    pub binary: String,
    /// Model alias ("sonnet", "opus", …) or None for the user's default.
    pub model: Option<String>,
    /// Working directory for the CLI; keeps it out of the user's projects.
    pub cwd: Option<std::path::PathBuf>,
    pub timeout: Duration,
    /// Attach screenshots when the engine provides them.
    pub vision: bool,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self { binary: "claude".into(), model: None, cwd: None, timeout: Duration::from_secs(90), vision: true }
    }
}

pub struct ClaudeBrain {
    cfg: ClaudeConfig,
    /// Session id for the current task; None until the first cycle.
    session: Mutex<Option<String>>,
}

impl ClaudeBrain {
    pub fn new(cfg: ClaudeConfig) -> Self {
        Self { cfg, session: Mutex::new(None) }
    }

    fn build_command(&self, resume: Option<&str>, new_id: &str) -> Command {
        let mut cmd = Command::new(&self.cfg.binary);
        cmd.arg("-p")
            .arg("--input-format").arg("stream-json")
            .arg("--output-format").arg("stream-json")
            .arg("--verbose")
            .arg("--tools").arg("")
            .arg("--max-turns").arg("1")
            .arg("--disable-slash-commands")
            .arg("--permission-mode").arg("dontAsk");
        match resume {
            Some(id) => { cmd.arg("--resume").arg(id); }
            None => { cmd.arg("--session-id").arg(new_id).arg("--system-prompt").arg(SYSTEM_PROMPT); }
        }
        if let Some(m) = &self.cfg.model {
            cmd.arg("--model").arg(m);
        }
        if let Some(d) = &self.cfg.cwd {
            cmd.current_dir(d);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd
    }

    /// The stdin frame: text plus, when allowed and present, the screenshot.
    fn user_frame(&self, input: &BrainInput) -> String {
        let text = render_user_message(input);
        let mut content = vec![json!({ "type": "text", "text": text })];
        if self.cfg.vision {
            if let Some(b64) = &input.screen.screenshot_png_b64 {
                content.push(json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64 } }));
            }
        }
        json!({ "type": "user", "message": { "role": "user", "content": content } }).to_string() + "\n"
    }
}

/// What one stream-json line contributes: assistant text, the final result, or nothing.
pub fn classify_line(line: &str) -> LineKind {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return LineKind::Other };
    match v.get("type").and_then(Value::as_str) {
        Some("assistant") => {
            let text = v["message"]["content"]
                .as_array()
                .map(|blocks| blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""))
                .unwrap_or_default();
            LineKind::Assistant(text)
        }
        Some("result") => LineKind::Result {
            text: v.get("result").and_then(Value::as_str).unwrap_or("").to_string(),
            session_id: v.get("session_id").and_then(Value::as_str).map(str::to_string),
            is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        },
        _ => LineKind::Other,
    }
}

#[derive(Debug, PartialEq)]
pub enum LineKind {
    Assistant(String),
    Result { text: String, session_id: Option<String>, is_error: bool },
    Other,
}

#[async_trait]
impl Brain for ClaudeBrain {
    fn name(&self) -> &'static str {
        "claude-code"
    }
    fn capabilities(&self) -> BrainCapabilities {
        BrainCapabilities { vision: self.cfg.vision, streaming: true }
    }

    async fn next_step(&self, input: BrainInput, out: mpsc::Sender<StepEvent>) -> Result<()> {
        let frame = self.user_frame(&input);
        let mut text = self.one_turn(frame, &out).await?;
        if terminal_of(&text).is_none() {
            // The model stopped after narrating. One nudge inside the same session.
            let nudge = json!({ "type": "user", "message": { "role": "user", "content": "Now the action: return exactly one act, lost, or done JSON object." } }).to_string() + "\n";
            text = self.one_turn(nudge, &out).await?;
        }
        match terminal_of(&text) {
            Some(ev) => { out.send(ev).await.ok(); Ok(()) }
            None => Err(MochiError::BrainProtocol(format!("no act/lost/done in: {}", text.chars().take(200).collect::<String>()))),
        }
    }

    async fn end_task(&self) {
        *self.session.lock().unwrap() = None;
    }
}

/// The last non-narration event in a turn's final text.
fn terminal_of(text: &str) -> Option<StepEvent> {
    parse_step_events(text).into_iter().rev().find(|e| !matches!(e, StepEvent::Narrate { .. }))
}

impl ClaudeBrain {
    /// Run one CLI turn: spawn, write the frame, stream narration out, return the final text.
    async fn one_turn(&self, frame: String, out: &mpsc::Sender<StepEvent>) -> Result<String> {
        let existing = self.session.lock().unwrap().clone();
        let new_id = uuid::Uuid::new_v4().to_string();
        let mut child = self
            .build_command(existing.as_deref(), &new_id)
            .spawn()
            .map_err(|e| MochiError::BrainUnreachable(format!("cannot start `{}`: {e}", self.cfg.binary)))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(frame.as_bytes()).await.map_err(|e| MochiError::BrainUnreachable(e.to_string()))?;
            stdin.shutdown().await.ok();
        }
        let stdout = child.stdout.take().expect("piped stdout");
        let mut lines = BufReader::new(stdout).lines();

        // Narration is forwarded as soon as the assistant message carrying it arrives;
        // the terminal event is taken from the final text so partial objects never fire twice.
        let mut final_text = String::new();
        let mut session_id = None;
        let mut sent_narrations = 0usize;
        let read = async {
            while let Ok(Some(line)) = lines.next_line().await {
                match classify_line(&line) {
                    LineKind::Assistant(text) => {
                        final_text = text.clone();
                        let narr: Vec<_> = parse_step_events(&text).into_iter().filter(|e| matches!(e, StepEvent::Narrate { .. })).collect();
                        for ev in narr.into_iter().skip(sent_narrations) {
                            sent_narrations += 1;
                            if out.send(ev).await.is_err() {
                                return Ok::<(), MochiError>(());
                            }
                        }
                    }
                    LineKind::Result { text, session_id: sid, is_error } => {
                        if !text.is_empty() {
                            final_text = text;
                        }
                        session_id = sid;
                        if is_error {
                            return Err(MochiError::BrainUnreachable(final_text.clone()));
                        }
                    }
                    LineKind::Other => {}
                }
            }
            Ok(())
        };
        match tokio::time::timeout(self.cfg.timeout, read).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => { let _ = child.kill().await; return Err(e); }
            Err(_) => { let _ = child.kill().await; return Err(MochiError::BrainUnreachable("claude timed out".into())); }
        }
        let status = child.wait().await.map_err(|e| MochiError::BrainUnreachable(e.to_string()))?;
        if !status.success() && final_text.is_empty() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                use tokio::io::AsyncReadExt;
                let _ = e.read_to_string(&mut err).await;
            }
            return Err(MochiError::BrainUnreachable(format!("claude exited with {status}: {}", err.trim())));
        }
        if existing.is_none() {
            *self.session.lock().unwrap() = Some(session_id.unwrap_or(new_id));
        }
        Ok(final_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_stream_json_lines() {
        let a = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"{\"kind\":\"narrate\",\"text\":\"Hi\"}"}]}}"#;
        assert_eq!(classify_line(a), LineKind::Assistant("{\"kind\":\"narrate\",\"text\":\"Hi\"}".into()));
        let r = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"abc"}"#;
        assert_eq!(classify_line(r), LineKind::Result { text: "done".into(), session_id: Some("abc".into()), is_error: false });
        assert_eq!(classify_line(r#"{"type":"system","subtype":"init"}"#), LineKind::Other);
        assert_eq!(classify_line("not json"), LineKind::Other);
    }

    #[test]
    fn first_call_sets_session_and_system_prompt_resume_does_not() {
        let b = ClaudeBrain::new(ClaudeConfig { model: Some("sonnet".into()), ..Default::default() });
        let first = format!("{:?}", b.build_command(None, "id-1").as_std());
        assert!(first.contains("--session-id") && first.contains("id-1") && first.contains("--system-prompt") && first.contains("--model"));
        let later = format!("{:?}", b.build_command(Some("id-1"), "id-2").as_std());
        assert!(later.contains("--resume") && !later.contains("--system-prompt") && !later.contains("id-2"));
    }

    fn gmail_input(task: &str) -> BrainInput {
        let el = |id, role: &str, label: &str, focused| UiElement { id, role: role.into(), label: label.into(), value: "".into(), bounds: Bounds { x: 100.0 * id as f64, y: 200.0, w: 90.0, h: 32.0 }, interactive: true, focused, secure: false };
        BrainInput {
            task: task.into(),
            screen: ScreenSnapshot {
                window: WindowInfo { app_id: "chrome.exe".into(), title: "Sara Kim — Dinner Friday? - Gmail - Google Chrome".into(), bounds: Bounds { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }, monitor_id: 0, unsaved: false },
                monitors: vec![],
                elements: vec![el(1, "button", "Archive", false), el(2, "button", "Reply", false), el(3, "button", "Forward", false), el(4, "edit", "Search mail", false)],
                remembered: vec![],
                screenshot_png_b64: None,
            },
            memory: vec![],
            history: vec![],
        }
    }

    /// Runs only where the Claude Code CLI is installed and logged in (it is in CI here).
    #[tokio::test]
    async fn live_claude_picks_the_reply_button_and_keeps_the_session() {
        if std::process::Command::new("claude").arg("--version").output().is_err() {
            eprintln!("claude CLI not installed; skipping live test");
            return;
        }
        let brain = ClaudeBrain::new(ClaudeConfig { model: Some("sonnet".into()), ..Default::default() });
        let (tx, mut rx) = mpsc::channel(8);
        brain.next_step(gmail_input("reply to Sara and say I'll be there at 6"), tx).await.expect("first cycle");
        let mut evs = vec![];
        while let Some(e) = rx.recv().await { evs.push(e); }
        eprintln!("cycle 1: {evs:?}");
        let last = evs.last().expect("an event");
        assert!(matches!(last, StepEvent::Act { action: Action::ClickElement { element_id: 2 }, .. }), "expected click #2 (Reply), got {last:?}");
        let sid = brain.session.lock().unwrap().clone();
        assert!(sid.is_some());

        // Second cycle resumes: the body is focused now, so the brain should type.
        let mut input = gmail_input("reply to Sara and say I'll be there at 6");
        input.screen.elements.push(UiElement { id: 9, role: "edit".into(), label: "Message body".into(), value: "".into(), bounds: Bounds { x: 100.0, y: 400.0, w: 900.0, h: 300.0 }, interactive: true, focused: true, secure: false });
        input.screen.elements.push(UiElement { id: 10, role: "button".into(), label: "Send".into(), value: "".into(), bounds: Bounds { x: 100.0, y: 720.0, w: 90.0, h: 32.0 }, interactive: true, focused: false, secure: false });
        input.history.push(Step { action: Action::ClickElement { element_id: 2 }, ok: true, note: "".into() });
        let (tx, mut rx) = mpsc::channel(8);
        brain.next_step(input, tx).await.expect("second cycle");
        let mut evs = vec![];
        while let Some(e) = rx.recv().await { evs.push(e); }
        eprintln!("cycle 2: {evs:?}");
        assert!(matches!(evs.last(), Some(StepEvent::Act { action: Action::Type { .. }, .. })), "expected type, got {:?}", evs.last());
        assert_eq!(brain.session.lock().unwrap().clone(), sid, "session id is kept across cycles");
    }
}
