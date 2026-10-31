//! Google Antigravity CLI (`agy`) adapter.
//!
//! Each engine cycle runs `agy -p <prompt> --output-format json` in a private working folder
//! whose `AGENTS.md` carries Mochi's rules (the CLI reads that file as workspace instructions);
//! the rules are repeated at the top of the first message too. The JSON reply has
//! `conversation_id`, `status` and `response`; later cycles pass `--conversation <id>` so the
//! model keeps the conversation. There is no image flag in headless mode, so this brain has
//! no vision.
//!
//! Windows notes (antigravity-cli issue #76): `agy` blocks at start-up when stdin is an open
//! pipe, so stdin is always closed; the prompt travels on the command line. Prompts too long
//! for a Windows command line (32 K) use `--input-format stream-json` on stdin instead, which
//! is the documented programmatic path and reads stdin by design.
//!
//! Flags from the headless-mode docs: `-p/--print`, `--output-format text|json|stream-json`,
//! `--input-format text|stream-json` (stream-json in requires stream-json out),
//! `--conversation <ID>`, `--model <slug>`, `--print-timeout <dur>`.

use async_trait::async_trait;
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

/// Above this the prompt goes through stdin (stream-json) rather than argv.
pub const MAX_ARGV_PROMPT: usize = 28_000;

#[derive(Debug, Clone)]
pub struct AntigravityConfig {
    /// `agy` on PATH (installed with `winget install Google.AntigravityCLI`).
    pub binary: String,
    /// Model slug from `agy models`, or None for the CLI's default.
    pub model: Option<String>,
    /// Private working folder; AGENTS.md lives here and the CLI runs with it as cwd.
    pub work_dir: PathBuf,
    pub timeout: Duration,
}

impl Default for AntigravityConfig {
    fn default() -> Self {
        Self { binary: "agy".into(), model: None, work_dir: std::env::temp_dir().join("mochi-antigravity"), timeout: Duration::from_secs(120) }
    }
}

pub struct AntigravityBrain {
    cfg: AntigravityConfig,
    conversation: Mutex<Option<String>>,
}

impl AntigravityBrain {
    pub fn new(cfg: AntigravityConfig) -> Self {
        Self { cfg, conversation: Mutex::new(None) }
    }

    /// Make sure the working folder exists and carries Mochi's instructions.
    fn prepare_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.cfg.work_dir).map_err(|e| MochiError::BrainUnreachable(format!("agy work dir: {e}")))?;
        let agents = self.cfg.work_dir.join("AGENTS.md");
        let wanted = format!("# Mochi\n\n{SYSTEM_PROMPT}\n\nDo not run shell commands, read or write files, or use any tool; everything you need is in the message. Answer with the JSON objects only.\n");
        if std::fs::read_to_string(&agents).map(|s| s != wanted).unwrap_or(true) {
            std::fs::write(&agents, wanted).map_err(|e| MochiError::BrainUnreachable(format!("AGENTS.md: {e}")))?;
        }
        Ok(())
    }

    /// `prompt` is Some for the argv form, None for the stdin (stream-json) form.
    fn build_command(&self, resume: Option<&str>, prompt: Option<&str>) -> Command {
        let (program, lead) = mochi_core::launch::resolve_program(&self.cfg.binary);
        let mut cmd = Command::new(program);
        cmd.args(lead);
        cmd.current_dir(&self.cfg.work_dir);
        if let Some(id) = resume {
            cmd.arg("--conversation").arg(id);
        }
        if let Some(m) = &self.cfg.model {
            cmd.arg("--model").arg(m);
        }
        cmd.arg("--print-timeout").arg(format!("{}s", self.cfg.timeout.as_secs().saturating_sub(5).max(10)));
        match prompt {
            Some(p) => {
                cmd.arg("--output-format").arg("json");
                // `--print` takes the next token as the prompt: keep it last.
                cmd.arg("--print").arg(p);
                cmd.stdin(Stdio::null());
            }
            None => {
                cmd.arg("--input-format").arg("stream-json").arg("--output-format").arg("stream-json");
                cmd.stdin(Stdio::piped());
            }
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        cmd
    }

    async fn run_once(&self, resume: Option<&str>, prompt: &str) -> Result<Run> {
        let via_argv = prompt.len() <= MAX_ARGV_PROMPT;
        let mut child = self
            .build_command(resume, via_argv.then_some(prompt))
            .spawn()
            .map_err(|e| MochiError::BrainUnreachable(format!("cannot start `{}`: {e}", self.cfg.binary)))?;
        if !via_argv {
            if let Some(mut stdin) = child.stdin.take() {
                let line = serde_json::json!({ "event": "user", "message": { "content": prompt } }).to_string();
                stdin.write_all(line.as_bytes()).await.map_err(|e| MochiError::BrainUnreachable(e.to_string()))?;
                stdin.write_all(b"\n").await.ok();
                stdin.shutdown().await.ok();
            }
        }
        let output = match tokio::time::timeout(self.cfg.timeout, child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Err(MochiError::BrainUnreachable(e.to_string())),
            Err(_) => return Err(MochiError::BrainUnreachable("agy timed out".into())),
        };
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let reply = if via_argv { parse_json_reply(&stdout) } else { parse_stream_reply(&stdout) };
        Ok(Run { success: output.status.success(), status_text: output.status.to_string(), reply, stderr, stdout })
    }
}

struct Run {
    success: bool,
    status_text: String,
    reply: Reply,
    stderr: String,
    stdout: String,
}

/// What we need from either output format.
#[derive(Debug, Default, PartialEq)]
pub struct Reply {
    pub conversation_id: Option<String>,
    pub response: String,
    pub status: Option<String>,
    pub error: Option<String>,
}

/// `--output-format json`: one object with `conversation_id`, `status`, `response`, `error?`.
/// Tolerates leading/trailing noise (the renderer may print a line or two before it).
pub fn parse_json_reply(stdout: &str) -> Reply {
    let candidates = std::iter::once(stdout.trim()).chain(stdout.lines().map(str::trim)).chain(outermost_object(stdout));
    for c in candidates {
        if let Ok(v) = serde_json::from_str::<Value>(c) {
            if v.get("response").is_some() || v.get("status").is_some() {
                return Reply {
                    conversation_id: v.get("conversation_id").and_then(Value::as_str).map(str::to_string),
                    response: v.get("response").and_then(Value::as_str).unwrap_or_default().to_string(),
                    status: v.get("status").and_then(Value::as_str).map(str::to_string),
                    error: v.get("error").and_then(Value::as_str).map(str::to_string),
                };
            }
        }
    }
    Reply::default()
}

fn outermost_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    (end > start).then(|| &s[start..=end])
}

/// `--output-format stream-json`: JSON lines. Field names are not pinned by the docs, so this
/// is lenient: any `conversation_id` wins, text is taken from a final `response` field, or
/// else from assistant/result events' string content blocks.
pub fn parse_stream_reply(stdout: &str) -> Reply {
    let mut r = Reply::default();
    let mut assistant_text = String::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
        if let Some(id) = v.get("conversation_id").and_then(Value::as_str) {
            r.conversation_id = Some(id.to_string());
        }
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            r.error = Some(e.to_string());
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            r.status = Some(s.to_string());
        }
        if let Some(resp) = v.get("response").and_then(Value::as_str) {
            r.response = resp.to_string();
            continue;
        }
        let kind = v.get("event").or_else(|| v.get("type")).and_then(Value::as_str).unwrap_or("");
        if kind.contains("assistant") || kind.contains("result") || kind.contains("response") {
            collect_text(v.get("message").unwrap_or(&v), &mut assistant_text);
        }
    }
    if r.response.is_empty() {
        r.response = assistant_text;
    }
    r
}

fn collect_text(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => { out.push_str(s); out.push('\n'); }
        Value::Array(a) => a.iter().for_each(|x| collect_text(x, out)),
        Value::Object(o) => {
            if let Some(Value::String(t)) = o.get("text") { out.push_str(t); out.push('\n'); }
            else if let Some(c) = o.get("content") { collect_text(c, out); }
        }
        _ => {}
    }
}

fn terminal_of(text: &str) -> Option<StepEvent> {
    parse_step_events(text).into_iter().rev().find(|e| !matches!(e, StepEvent::Narrate { .. }))
}

#[async_trait]
impl Brain for AntigravityBrain {
    fn name(&self) -> &'static str {
        "antigravity"
    }
    fn capabilities(&self) -> BrainCapabilities {
        BrainCapabilities { vision: false, streaming: false }
    }

    async fn next_step(&self, input: BrainInput, out: mpsc::Sender<StepEvent>) -> Result<()> {
        self.prepare_dir()?;
        let full_prompt = |with_rules: bool| {
            let mut p = String::new();
            if with_rules {
                p.push_str("Rules (also in AGENTS.md):\n");
                p.push_str(SYSTEM_PROMPT);
                p.push_str("\n\n");
            }
            p.push_str(&render_user_message(&input));
            p
        };

        let mut resume = self.conversation.lock().unwrap().clone();
        let mut run = self.run_once(resume.as_deref(), &full_prompt(resume.is_none())).await?;
        if resume.is_some() && run.reply.response.trim().is_empty() {
            // The conversation is unusable (expired, CLI drift, …): start a fresh one.
            *self.conversation.lock().unwrap() = None;
            resume = None;
            run = self.run_once(None, &full_prompt(true)).await?;
        }
        if resume.is_none() {
            if let Some(id) = run.reply.conversation_id.clone() {
                *self.conversation.lock().unwrap() = Some(id);
            }
        }

        let text = run.reply.response;
        if text.trim().is_empty() {
            let detail = run.reply.error
                .or_else(|| run.stderr.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string))
                .or_else(|| run.stdout.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.chars().take(300).collect()))
                .unwrap_or_else(|| if run.success { "empty reply (is the output renderer silent? see antigravity-cli #76)".into() } else { "no output".into() });
            let status = run.reply.status.unwrap_or(run.status_text);
            return Err(MochiError::BrainUnreachable(format!("agy {status}: {detail}")));
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
        *self.conversation.lock().unwrap() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_json_shape() {
        let out = r#"{"conversation_id":"c-1","status":"SUCCESS","response":"{\"kind\":\"narrate\",\"text\":\"Hi\"}\n{\"kind\":\"done\",\"summary\":\"ok\",\"gesture\":\"heart\"}","duration_seconds":1.2,"num_turns":1,"usage":{"input_tokens":10,"output_tokens":5}}"#;
        let r = parse_json_reply(out);
        assert_eq!(r.conversation_id.as_deref(), Some("c-1"));
        assert_eq!(r.status.as_deref(), Some("SUCCESS"));
        assert!(matches!(terminal_of(&r.response), Some(StepEvent::Done { .. })));
        // Noise around the object is tolerated.
        let noisy = format!("Loading…\n{out}\n");
        assert_eq!(parse_json_reply(&noisy).conversation_id.as_deref(), Some("c-1"));
        let err = parse_json_reply(r#"{"conversation_id":"c-2","status":"ERROR","response":"","error":"RESOURCE_EXHAUSTED"}"#);
        assert_eq!(err.error.as_deref(), Some("RESOURCE_EXHAUSTED"));
        assert_eq!(parse_json_reply("nothing here"), Reply::default());
    }

    #[test]
    fn parses_stream_json_leniently() {
        let out = r#"{"event":"system","conversation_id":"c-9"}
{"event":"assistant","message":{"content":[{"type":"text","text":"{\"kind\":\"act\",\"action\":{\"type\":\"click_element\",\"element_id\":2},\"narration\":\"Clicking Reply\"}"}]}}
{"event":"result","status":"SUCCESS"}"#;
        let r = parse_stream_reply(out);
        assert_eq!(r.conversation_id.as_deref(), Some("c-9"));
        assert_eq!(r.status.as_deref(), Some("SUCCESS"));
        assert!(matches!(terminal_of(&r.response), Some(StepEvent::Act { action: Action::ClickElement { element_id: 2 }, .. })));
        // A final object with `response` wins over collected text.
        let r2 = parse_stream_reply("{\"event\":\"assistant\",\"message\":\"junk\"}\n{\"response\":\"final\",\"conversation_id\":\"c\"}");
        assert_eq!(r2.response, "final");
    }

    #[test]
    fn command_shape_argv_and_stdin_forms() {
        let dir = std::env::temp_dir().join("mochi-agy-test");
        let b = AntigravityBrain::new(AntigravityConfig { model: Some("gemini-3-pro".into()), work_dir: dir.clone(), ..Default::default() });
        let first = format!("{:?}", b.build_command(None, Some("hello")).as_std());
        assert!(first.contains("--output-format") && first.contains("\"json\"") && first.contains("--model") && !first.contains("--conversation"));
        assert!(first.ends_with("\"hello\"") || first.contains("\"--print\" \"hello\""), "prompt is the last argument: {first}");
        let later = format!("{:?}", b.build_command(Some("c-1"), Some("hi")).as_std());
        assert!(later.contains("--conversation") && later.contains("c-1"));
        let streamed = format!("{:?}", b.build_command(Some("c-1"), None).as_std());
        assert!(streamed.contains("--input-format") && streamed.contains("stream-json") && !streamed.contains("\"--print\""));
        b.prepare_dir().unwrap();
        assert!(std::fs::read_to_string(dir.join("AGENTS.md")).unwrap().contains("Mochi"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Runs only where `agy` is installed.
    #[tokio::test]
    async fn live_agy_if_available() {
        let installed = std::process::Command::new("agy").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
        if !installed {
            eprintln!("agy not installed; skipping live test");
            return;
        }
        let brain = AntigravityBrain::new(AntigravityConfig::default());
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
