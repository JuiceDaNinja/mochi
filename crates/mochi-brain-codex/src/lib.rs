//! OpenAI Codex CLI adapter.
//!
//! Each engine cycle runs `codex exec --json` with the user message on stdin. Mochi's rules
//! live in an `AGENTS.md` inside a private working folder (Codex reads that file as project
//! instructions) and are repeated at the top of the first message. Screenshots go in with
//! `-i`, the final answer comes back through `-o <file>`, and later cycles use
//! `codex exec resume <thread id>` so Codex keeps the conversation.
//!
//! Flags verified against codex-cli 0.166: `exec [PROMPT] --json -m -i -C -s read-only
//! --skip-git-repo-check -o <FILE>`; `exec resume <SESSION_ID> [PROMPT]`.

use async_trait::async_trait;
use base64::Engine;
use mochi_core::brain::SYSTEM_PROMPT;
use mochi_core::prompt::{parse_step_events, render_user_message};
use mochi_core::*;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct CodexConfig {
    /// `codex` on PATH (npm global: `codex.cmd` on Windows, resolved via PATHEXT).
    pub binary: String,
    /// Model name, or None for the CLI's default.
    pub model: Option<String>,
    /// Private working folder; AGENTS.md, screenshots and the last-message file live here.
    pub work_dir: PathBuf,
    pub timeout: Duration,
    pub vision: bool,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self { binary: "codex".into(), model: None, work_dir: std::env::temp_dir().join("mochi-codex"), timeout: Duration::from_secs(120), vision: true }
    }
}

pub struct CodexBrain {
    cfg: CodexConfig,
    thread: Mutex<Option<String>>,
}

impl CodexBrain {
    pub fn new(cfg: CodexConfig) -> Self {
        Self { cfg, thread: Mutex::new(None) }
    }

    /// Make sure the working folder exists and carries Mochi's instructions.
    fn prepare_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.cfg.work_dir).map_err(|e| MochiError::BrainUnreachable(format!("codex work dir: {e}")))?;
        let agents = self.cfg.work_dir.join("AGENTS.md");
        let wanted = format!("# Mochi\n\n{SYSTEM_PROMPT}\n\nDo not run shell commands or read files; everything you need is in the message. Answer with the JSON objects only.\n");
        if std::fs::read_to_string(&agents).map(|s| s != wanted).unwrap_or(true) {
            std::fs::write(&agents, wanted).map_err(|e| MochiError::BrainUnreachable(format!("AGENTS.md: {e}")))?;
        }
        Ok(())
    }

    fn build_command(&self, resume: Option<&str>, image: Option<&PathBuf>, last_msg: &PathBuf) -> Command {
        let (program, lead) = mochi_core::launch::resolve_program(&self.cfg.binary);
        let mut cmd = Command::new(program);
        cmd.args(lead);
        cmd.arg("exec");
        if let Some(id) = resume {
            cmd.arg("resume").arg(id);
        }
        cmd.arg("--json")
            .arg("--skip-git-repo-check")
            .arg("-C").arg(&self.cfg.work_dir)
            .arg("-o").arg(last_msg);
        if let Some(m) = &self.cfg.model {
            cmd.arg("-m").arg(m);
        }
        if let Some(img) = image {
            cmd.arg("-i").arg(img);
        }
        // Prompt comes on stdin (no command-line length limits on Windows).
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        cmd
    }
}

/// Pull a thread/session id out of the JSONL stream, whichever key this version uses.
pub fn find_thread_id(jsonl: &str) -> Option<String> {
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        for key in ["thread_id", "session_id", "conversation_id"] {
            if let Some(id) = v.get(key).and_then(Value::as_str) {
                return Some(id.to_string());
            }
            if let Some(id) = v.get("thread").and_then(|t| t.get("id")).and_then(Value::as_str) {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Assistant text from the JSONL stream, as a fallback when the `-o` file is missing.
pub fn collect_agent_text(jsonl: &str) -> String {
    let mut out = String::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let item = v.get("item").unwrap_or(&v);
        let is_msg = item.get("type").and_then(Value::as_str).map_or(false, |t| t.contains("agent_message") || t == "message");
        if is_msg {
            if let Some(t) = item.get("text").and_then(Value::as_str) {
                out.push_str(t);
                out.push('\n');
            }
        }
    }
    out
}

fn terminal_of(text: &str) -> Option<StepEvent> {
    parse_step_events(text).into_iter().rev().find(|e| !matches!(e, StepEvent::Narrate { .. }))
}

#[async_trait]
impl Brain for CodexBrain {
    fn name(&self) -> &'static str {
        "codex"
    }
    fn capabilities(&self) -> BrainCapabilities {
        BrainCapabilities { vision: self.cfg.vision, streaming: false }
    }

    async fn next_step(&self, input: BrainInput, out: mpsc::Sender<StepEvent>) -> Result<()> {
        self.prepare_dir()?;
        let resume = self.thread.lock().unwrap().clone();

        // Screenshot to a file for -i.
        let image = match (&self.cfg.vision, &input.screen.screenshot_png_b64) {
            (true, Some(b64)) => {
                let p = self.cfg.work_dir.join("screen.png");
                let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap_or_default();
                std::fs::write(&p, bytes).ok().map(|_| p)
            }
            _ => None,
        };
        let last_msg = self.cfg.work_dir.join("last.txt");
        let _ = std::fs::remove_file(&last_msg);

        let mut prompt = String::new();
        if resume.is_none() {
            prompt.push_str("Rules (also in AGENTS.md):\n");
            prompt.push_str(SYSTEM_PROMPT);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&render_user_message(&input));

        let mut child = self
            .build_command(resume.as_deref(), image.as_ref(), &last_msg)
            .spawn()
            .map_err(|e| MochiError::BrainUnreachable(format!("cannot start `{}`: {e}", self.cfg.binary)))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await.map_err(|e| MochiError::BrainUnreachable(e.to_string()))?;
            stdin.shutdown().await.ok();
        }
        let output = match tokio::time::timeout(self.cfg.timeout, child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Err(MochiError::BrainUnreachable(e.to_string())),
            Err(_) => return Err(MochiError::BrainUnreachable("codex timed out".into())),
        };
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if resume.is_none() {
            if let Some(id) = find_thread_id(&stdout) {
                *self.thread.lock().unwrap() = Some(id);
            }
        }
        let text = std::fs::read_to_string(&last_msg).ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| collect_agent_text(&stdout));
        if text.trim().is_empty() {
            let detail = stderr.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string)
                .or_else(|| stdout.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.chars().take(300).collect()))
                .unwrap_or_else(|| "no output".into());
            return Err(MochiError::BrainUnreachable(format!("codex exited with {}: {detail}", output.status)));
        }

        let events = parse_step_events(&text);
        for ev in events.iter().filter(|e| matches!(e, StepEvent::Narrate { .. })) {
            if out.send(ev.clone()).await.is_err() {
                return Ok(());
            }
        }
        match terminal_of(&text) {
            Some(ev) => { out.send(ev).await.ok(); Ok(()) }
            None => Err(MochiError::BrainProtocol(format!("no act/lost/done in: {}", text.chars().take(200).collect::<String>()))),
        }
    }

    async fn end_task(&self) {
        *self.thread.lock().unwrap() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_thread_id_in_any_of_the_known_shapes() {
        assert_eq!(find_thread_id(r#"{"type":"thread.started","thread_id":"t-1"}"#).as_deref(), Some("t-1"));
        assert_eq!(find_thread_id(r#"{"type":"session_configured","session_id":"s-2"}"#).as_deref(), Some("s-2"));
        assert_eq!(find_thread_id(r#"{"type":"x","thread":{"id":"t-3"}}"#).as_deref(), Some("t-3"));
        assert_eq!(find_thread_id("not json\n{\"type\":\"turn.completed\"}"), None);
    }

    #[test]
    fn collects_agent_text_from_items() {
        let jsonl = r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"kind\":\"narrate\",\"text\":\"Hi\"}"}}
{"type":"item.completed","item":{"type":"command_execution","command":"ls"}}
{"type":"item.completed","item":{"type":"agent_message","text":"{\"kind\":\"done\",\"summary\":\"ok\"}"}}"#;
        let text = collect_agent_text(jsonl);
        let evs = parse_step_events(&text);
        assert_eq!(evs.len(), 2);
        assert!(matches!(terminal_of(&text), Some(StepEvent::Done { .. })));
    }

    #[test]
    fn command_shape_first_turn_and_resume() {
        let dir = std::env::temp_dir().join("mochi-codex-test");
        let b = CodexBrain::new(CodexConfig { model: Some("gpt-5-codex".into()), work_dir: dir.clone(), ..Default::default() });
        let last = dir.join("last.txt");
        let first = format!("{:?}", b.build_command(None, None, &last).as_std());
        assert!(first.contains("\"exec\"") && first.contains("--json") && first.contains("-m") && !first.contains("resume"));
        let img = dir.join("screen.png");
        let later = format!("{:?}", b.build_command(Some("t-1"), Some(&img), &last).as_std());
        assert!(later.contains("resume") && later.contains("t-1") && later.contains("-i"));
        b.prepare_dir().unwrap();
        assert!(std::fs::read_to_string(dir.join("AGENTS.md")).unwrap().contains("Mochi"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Runs only where Codex is installed and signed in.
    #[tokio::test]
    async fn live_codex_if_available() {
        let logged_in = std::process::Command::new("codex").args(["login", "status"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains("Logged in")).unwrap_or(false);
        if !logged_in {
            eprintln!("codex not signed in; skipping live test");
            return;
        }
        let brain = CodexBrain::new(CodexConfig::default());
        let input = BrainInput {
            task: "reply to Sara".into(),
            screen: ScreenSnapshot {
                window: WindowInfo { app_id: "chrome.exe".into(), title: "Gmail".into(), bounds: Bounds { x: 0.0, y: 0.0, w: 100.0, h: 100.0 }, monitor_id: 0, unsaved: false },
                monitors: vec![],
                elements: vec![UiElement { id: 2, role: "button".into(), label: "Reply".into(), value: "".into(), bounds: Bounds::default(), interactive: true, focused: false, secure: false }],
                remembered: vec![],
                screenshot_png_b64: None,
            },
            memory: vec![],
            history: vec![],
        };
        let (tx, mut rx) = mpsc::channel(8);
        brain.next_step(input, tx).await.unwrap();
        let mut last = None;
        while let Some(e) = rx.recv().await { last = Some(e); }
        assert!(matches!(last, Some(StepEvent::Act { action: Action::ClickElement { element_id: 2 }, .. })));
    }
}
