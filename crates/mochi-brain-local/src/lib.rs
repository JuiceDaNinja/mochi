//! Local AI adapter: any OpenAI-compatible `/v1/chat/completions` endpoint reachable by
//! IP:port (Ollama, LM Studio, llama.cpp server, vLLM, …). The conversation for one task
//! is kept client-side and resent each cycle.

use async_trait::async_trait;
use mochi_core::brain::SYSTEM_PROMPT;
use mochi_core::prompt::{parse_step_events, render_user_message};
use mochi_core::*;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct LocalConfig {
    pub host: String,
    pub port: u16,
    /// Model name as the server knows it, e.g. "llama3.2" or "qwen2.5-vl".
    pub model: String,
    /// Optional bearer token (LM Studio and vLLM accept anything; some servers require one).
    pub api_key: Option<String>,
    /// Send screenshots as image_url data URIs. Only for multimodal models.
    pub vision: bool,
    pub timeout: Duration,
    /// How many past turns to resend. Keeps small-context local models from drowning.
    pub history_turns: usize,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self { host: "127.0.0.1".into(), port: 11434, model: "llama3.2".into(), api_key: None, vision: false, timeout: Duration::from_secs(120), history_turns: 6 }
    }
}

impl LocalConfig {
    pub fn url(&self) -> String {
        let host = if self.host.contains(':') && !self.host.starts_with('[') { format!("[{}]", self.host) } else { self.host.clone() };
        format!("http://{host}:{}/v1/chat/completions", self.port)
    }
}

pub struct LocalBrain {
    cfg: LocalConfig,
    client: reqwest::Client,
    /// (role, content) pairs for the current task, after the system prompt.
    transcript: Mutex<Vec<Value>>,
}

impl LocalBrain {
    pub fn new(cfg: LocalConfig) -> Self {
        let client = reqwest::Client::builder().timeout(cfg.timeout).no_proxy().build().expect("reqwest client");
        Self { cfg, client, transcript: Mutex::new(vec![]) }
    }

    fn user_message(&self, input: &BrainInput) -> Value {
        let text = render_user_message(input);
        match (&self.cfg.vision, &input.screen.screenshot_png_b64) {
            (true, Some(b64)) => json!({ "role": "user", "content": [
                { "type": "text", "text": text },
                { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{b64}") } }
            ]}),
            _ => json!({ "role": "user", "content": text }),
        }
    }

    /// Quick reachability probe for the settings screen: GET /v1/models.
    pub async fn probe(&self) -> std::result::Result<Vec<String>, String> {
        let url = self.cfg.url().replace("/chat/completions", "/models");
        let mut req = self.client.get(&url);
        if let Some(k) = &self.cfg.api_key { req = req.bearer_auth(k); }
        let v: Value = req.send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
        Ok(v["data"].as_array().map(|a| a.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect()).unwrap_or_default())
    }
}

#[async_trait]
impl Brain for LocalBrain {
    fn name(&self) -> &'static str { "local" }
    fn capabilities(&self) -> BrainCapabilities { BrainCapabilities { vision: self.cfg.vision, streaming: false } }

    async fn next_step(&self, input: BrainInput, out: mpsc::Sender<StepEvent>) -> Result<()> {
        let user = self.user_message(&input);
        let mut messages = vec![json!({ "role": "system", "content": SYSTEM_PROMPT })];
        {
            let t = self.transcript.lock().unwrap();
            let keep = t.len().saturating_sub(self.cfg.history_turns * 2);
            messages.extend(t[keep..].iter().cloned());
        }
        messages.push(user.clone());

        let body = json!({ "model": self.cfg.model, "messages": messages, "temperature": 0.2, "stream": false });
        let mut req = self.client.post(self.cfg.url()).json(&body);
        if let Some(k) = &self.cfg.api_key { req = req.bearer_auth(k); }
        let resp = req.send().await.map_err(|e| MochiError::BrainUnreachable(format!("{}: {e}", self.cfg.url())))?;
        let status = resp.status();
        let v: Value = resp.json().await.map_err(|e| MochiError::BrainProtocol(e.to_string()))?;
        if !status.is_success() {
            return Err(MochiError::BrainUnreachable(format!("{status}: {}", v["error"]["message"].as_str().unwrap_or("request failed"))));
        }
        let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();

        let mut events = parse_step_events(&text);
        if !events.iter().any(|e| !matches!(e, StepEvent::Narrate { .. })) {
            return Err(MochiError::BrainProtocol(format!("no act/lost/done in: {}", text.chars().take(200).collect::<String>())));
        }
        // Keep the transcript: what we asked (without the image, to stay small) and what it said.
        {
            let mut t = self.transcript.lock().unwrap();
            t.push(json!({ "role": "user", "content": render_user_message(&input) }));
            t.push(json!({ "role": "assistant", "content": text }));
        }
        // Narrations first, then exactly one terminal.
        let terminal_idx = events.iter().rposition(|e| !matches!(e, StepEvent::Narrate { .. })).unwrap();
        let terminal = events.remove(terminal_idx);
        for ev in events.into_iter().filter(|e| matches!(e, StepEvent::Narrate { .. })) {
            if out.send(ev).await.is_err() { return Ok(()); }
        }
        out.send(terminal).await.ok();
        Ok(())
    }

    async fn end_task(&self) {
        self.transcript.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A two-request fake server: answers like a local model and records what it was sent.
    async fn fake_server(replies: Vec<&'static str>) -> (u16, tokio::sync::mpsc::Receiver<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            for reply in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 1 << 20];
                let mut n = 0;
                loop {
                    let k = sock.read(&mut buf[n..]).await.unwrap();
                    n += k;
                    let s = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Some(idx) = s.find("\r\n\r\n") {
                        let len: usize = s[..idx].lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                        if n >= idx + 4 + len {
                            tx.send(serde_json::from_str(&s[idx + 4..idx + 4 + len]).unwrap()).await.unwrap();
                            break;
                        }
                    }
                    if k == 0 { break; }
                }
                let body = json!({ "choices": [{ "message": { "role": "assistant", "content": reply } }] }).to_string();
                let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body);
                sock.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        (port, rx)
    }

    fn input() -> BrainInput {
        BrainInput {
            task: "reply to Sara".into(),
            screen: ScreenSnapshot {
                window: WindowInfo { app_id: "chrome.exe".into(), title: "Gmail".into(), bounds: Bounds { x: 0.0, y: 0.0, w: 100.0, h: 100.0 }, monitor_id: 0, unsaved: false },
                monitors: vec![],
                elements: vec![UiElement { id: 2, role: "button".into(), label: "Reply".into(), value: "".into(), bounds: Bounds::default(), interactive: true, focused: false, secure: false }],
                remembered: vec![],
                screenshot_png_b64: Some("AAAA".into()),
            },
            memory: vec![],
            history: vec![],
        }
    }

    #[tokio::test]
    async fn talks_openai_format_keeps_transcript_and_attaches_images_only_with_vision() {
        let (port, mut seen) = fake_server(vec![
            "```json\n{\"kind\":\"narrate\",\"text\":\"Found it!\"}\n{\"kind\":\"act\",\"action\":{\"type\":\"click_element\",\"element_id\":2}}\n```",
            "{\"kind\":\"done\",\"summary\":\"ok\",\"sent_user_words\":false}",
        ]).await;
        let brain = LocalBrain::new(LocalConfig { port, model: "test-model".into(), vision: true, ..Default::default() });

        let (tx, mut rx) = mpsc::channel(8);
        brain.next_step(input(), tx).await.unwrap();
        let mut evs = vec![];
        while let Some(e) = rx.recv().await { evs.push(e); }
        assert!(matches!(evs[0], StepEvent::Narrate { .. }));
        assert!(matches!(evs[1], StepEvent::Act { action: Action::ClickElement { element_id: 2 }, .. }));

        let req1 = seen.recv().await.unwrap();
        assert_eq!(req1["model"], "test-model");
        assert_eq!(req1["messages"][0]["role"], "system");
        assert!(req1["messages"][1]["content"][1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));

        let (tx, mut rx) = mpsc::channel(8);
        brain.next_step(input(), tx).await.unwrap();
        assert!(matches!(rx.recv().await, Some(StepEvent::Done { .. })));
        let req2 = seen.recv().await.unwrap();
        let msgs = req2["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4, "system + previous user + previous assistant + new user");
        assert_eq!(msgs[2]["role"], "assistant");
    }

    #[tokio::test]
    async fn unreachable_server_is_a_brain_unreachable_error() {
        let brain = LocalBrain::new(LocalConfig { port: 1, timeout: Duration::from_secs(2), ..Default::default() });
        let (tx, _rx) = mpsc::channel(8);
        assert!(matches!(brain.next_step(input(), tx).await, Err(MochiError::BrainUnreachable(_))));
    }

    #[test]
    fn url_handles_ipv6() {
        assert_eq!(LocalConfig { host: "::1".into(), port: 8080, ..Default::default() }.url(), "http://[::1]:8080/v1/chat/completions");
        assert_eq!(LocalConfig::default().url(), "http://127.0.0.1:11434/v1/chat/completions");
    }
}
