//! Mochi desktop connector (Tauri 2).
//!
//! Phase 2: the character window plus a real task engine. The brain is picked in settings
//! (Claude Code CLI, a local model over IP:port, or the scripted mock); the screen is read
//! with Windows UI Automation and driven with SendInput when running on Windows.

mod settings;

use mochi_brain_claude::{ClaudeBrain, ClaudeConfig};
use mochi_brain_codex::{CodexBrain, CodexConfig};
use mochi_brain_local::{LocalBrain, LocalConfig};
use mochi_brain_mock::{FakeDesktop, MockBrain};
use mochi_core::engine::EngineConfig;
use mochi_core::protocol::DesktopMessage;
use mochi_core::*;
// mochi_core exports its own `Result<T>` alias; Tauri commands need the std one.
use std::result::Result;
use mochi_link::{Link, LinkConfig, LinkEvent, LinkHandle};
use mochi_platform_windows::WindowsDesktop;
use serde::{Deserialize, Serialize};
use settings::{BrainKind, Settings};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Listener, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::mpsc;

/// Release builds have no console: append diagnostics to <app data>/mochi.log.
static LOG_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
fn log(msg: &str) {
    eprintln!("[mochi] {msg}");
    if let Some(p) = LOG_PATH.get() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let _ = writeln!(f, "{secs} {msg}");
        }
    }
}

#[tauri::command]
fn log_js(line: String) {
    log(&format!("js: {line}"));
}

/// Where the running task reads the screen from.
#[derive(Clone)]
enum Platform {
    Fake(Arc<FakeDesktop>),
    Windows(Arc<WindowsDesktop>),
}

struct AppState {
    data_dir: PathBuf,
    settings: Mutex<Settings>,
    engine: Mutex<Option<EngineHandle>>,
    platform: Mutex<Option<Platform>>,
    memory: Arc<Memory>,
    /// Physical px per logical px of the character window's monitor.
    scale: Mutex<f64>,
    link: Mutex<Option<LinkHandle>>,
    link_port: Mutex<Option<u16>>,
}

#[derive(Serialize, Clone)]
struct LinkStatus {
    port: Option<u16>,
    desktop_name: String,
    connected: Option<String>,
    devices: Vec<mochi_link::PairedDevice>,
}

#[derive(Serialize, Clone)]
struct MonitorDto {
    id: u32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    scale: f64,
    primary: bool,
}

#[derive(Deserialize, Clone, Copy)]
struct BodyRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Serialize, Clone)]
struct ProbeResult {
    ok: bool,
    detail: String,
}

// ---------- window / character commands ----------

#[tauri::command]
fn primary_monitor(window: WebviewWindow, state: State<'_, AppState>) -> Result<MonitorDto, String> {
    let m = window.current_monitor().map_err(|e| e.to_string())?.or_else(|| window.primary_monitor().ok().flatten());
    let Some(m) = m else { return Err("no monitor".into()) };
    let p = m.position();
    let s = m.size();
    let scale = m.scale_factor();
    *state.scale.lock().unwrap() = scale;
    Ok(MonitorDto { id: 0, x: p.x as f64 / scale, y: p.y as f64 / scale, w: s.width as f64 / scale, h: s.height as f64 / scale, scale, primary: true })
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn set_settings(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    settings.save(&state.data_dir).map_err(|e| e.to_string())?;
    *state.settings.lock().unwrap() = settings.clone();
    if let Some(h) = state.engine.lock().unwrap().as_ref() {
        h.set_narration(settings.narration);
    }
    let _ = app.emit("mochi://settings", settings);
    Ok(())
}

/// Can we reach the configured brain? Shown as a line in the settings window.
#[tauri::command]
async fn probe_brain(state: State<'_, AppState>) -> Result<ProbeResult, String> {
    let s = state.settings.lock().unwrap().clone();
    Ok(match s.brain {
        BrainKind::Mock => ProbeResult { ok: true, detail: "Demo mode: scripted Gmail on a pretend screen.".into() },
        BrainKind::ClaudeCode => {
            let (prog, lead) = mochi_core::launch::resolve_program(&s.claude.binary);
            let version = match tokio::process::Command::new(&prog).args(&lead).arg("--version").output().await {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
                Ok(o) => return Ok(ProbeResult { ok: false, detail: format!("`{} --version` failed: {}", s.claude.binary, String::from_utf8_lossy(&o.stderr).trim()) }),
                Err(e) => return Ok(ProbeResult { ok: false, detail: format!("Cannot start `{}`: {e}. Install Claude Code (npm i -g @anthropic-ai/claude-code) and sign in once.", s.claude.binary) }),
            };
            // A real one-line call with the chosen model: catches sign-in and model/plan problems.
            let model = claude_model(&s);
            let mut cmd = tokio::process::Command::new(&prog);
            cmd.args(&lead);
            cmd.args(["-p", "Reply with exactly: OK", "--output-format", "json", "--tools", "", "--max-turns", "1", "--no-session-persistence", "--model", &model]);
            cmd.current_dir(&state.data_dir);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            match tokio::time::timeout(Duration::from_secs(60), cmd.output()).await {
                Ok(Ok(o)) => {
                    let out = String::from_utf8_lossy(&o.stdout).to_string();
                    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_default();
                    let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(!o.status.success());
                    let text = v.get("result").and_then(|r| r.as_str()).map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&o.stderr).trim().to_string());
                    if is_error || !o.status.success() {
                        ProbeResult { ok: false, detail: format!("Claude Code {version} starts, but model \"{model}\" failed: {}", text.chars().take(240).collect::<String>()) }
                    } else {
                        ProbeResult { ok: true, detail: format!("Claude Code {version}, model {model}: replied \"{}\".", text.trim().chars().take(40).collect::<String>()) }
                    }
                }
                Ok(Err(e)) => ProbeResult { ok: false, detail: format!("Could not run a test prompt: {e}") },
                Err(_) => ProbeResult { ok: false, detail: "The test prompt timed out after 60 s. Is Claude Code signed in? Run `claude` once in a terminal.".into() },
            }
        }
        BrainKind::Codex => {
            let (prog, lead) = mochi_core::launch::resolve_program(&s.codex.binary);
            let version = match tokio::process::Command::new(&prog).args(&lead).arg("--version").output().await {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
                Ok(o) => return Ok(ProbeResult { ok: false, detail: format!("`{} --version` failed: {}", s.codex.binary, String::from_utf8_lossy(&o.stderr).trim()) }),
                Err(e) => return Ok(ProbeResult { ok: false, detail: format!("Cannot start `{}`: {e}. Install Codex CLI (npm i -g @openai/codex) and run `codex login` once.", s.codex.binary) }),
            };
            let work = state.data_dir.join("codex");
            let _ = std::fs::create_dir_all(&work);
            let mut cmd = tokio::process::Command::new(&prog);
            cmd.args(&lead);
            cmd.args(["exec", "--skip-git-repo-check", "-C"]).arg(&work).args(["-o"]).arg(work.join("probe.txt")).args(["Reply with exactly: OK"]);
            if let Some(m) = s.codex.model.clone().filter(|m| !m.trim().is_empty()) { cmd.args(["-m", &m]); }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            match tokio::time::timeout(Duration::from_secs(90), cmd.output()).await {
                Ok(Ok(o)) => {
                    let reply = std::fs::read_to_string(work.join("probe.txt")).unwrap_or_default();
                    if o.status.success() && !reply.trim().is_empty() {
                        ProbeResult { ok: true, detail: format!("{version}: replied \"{}\".", reply.trim().chars().take(40).collect::<String>()) }
                    } else {
                        let err = String::from_utf8_lossy(&o.stderr);
                        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output");
                        ProbeResult { ok: false, detail: format!("{version} starts, but the test prompt failed: {}", last.chars().take(240).collect::<String>()) }
                    }
                }
                Ok(Err(e)) => ProbeResult { ok: false, detail: format!("Could not run a test prompt: {e}") },
                Err(_) => ProbeResult { ok: false, detail: "The test prompt timed out after 90 s. Is Codex signed in? Run `codex login` in a terminal.".into() },
            }
        }
        BrainKind::Local => {
            let brain = LocalBrain::new(local_config(&s));
            match brain.probe().await {
                Ok(models) if models.iter().any(|m| m == &s.local.model) => ProbeResult { ok: true, detail: format!("Connected. Model {} is available.", s.local.model) },
                Ok(models) if models.is_empty() => ProbeResult { ok: true, detail: "Connected; the server did not list its models.".into() },
                Ok(models) => ProbeResult { ok: false, detail: format!("Connected, but {} is not listed. Available: {}", s.local.model, models.join(", ")) },
                Err(e) => ProbeResult { ok: false, detail: format!("Cannot reach http://{}:{} — {e}", s.local.host, s.local.port) },
            }
        }
    })
}

/// Empty model means "sonnet": the CLI's own default may be a model the account cannot use.
fn claude_model(s: &Settings) -> String {
    s.claude.model.clone().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "sonnet".into())
}

fn local_config(s: &Settings) -> LocalConfig {
    LocalConfig {
        host: s.local.host.clone(),
        port: s.local.port,
        model: s.local.model.clone(),
        api_key: s.local.api_key.clone().filter(|k| !k.is_empty()),
        vision: s.local.vision,
        ..Default::default()
    }
}

// ---------- tasks ----------

/// `scenario` is only used by the mock brain ("gmail", "gmail_lost", "close_unsaved").
#[tauri::command]
async fn start_task(app: AppHandle, task: String, scenario: Option<String>) -> Result<(), String> {
    begin_task(&app, task, scenario).await
}

async fn begin_task(app: &AppHandle, task: String, scenario: Option<String>) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.engine.lock().unwrap().is_some() {
        return Err("Mochi is already busy".into());
    }
    let s = state.settings.lock().unwrap().clone();
    let use_mock = s.brain == BrainKind::Mock || scenario.is_some() || !WindowsDesktop::available();
    let scenario = scenario.unwrap_or_else(|| "gmail".into());

    let (brain, platform): (Arc<dyn Brain>, Platform) = if use_mock {
        let d = Arc::new(if scenario == "close_unsaved" { FakeDesktop::unsaved_doc() } else { FakeDesktop::gmail() });
        (Arc::new(MockBrain::scenario(&scenario)), Platform::Fake(d))
    } else {
        let brain: Arc<dyn Brain> = match s.brain {
            BrainKind::ClaudeCode => Arc::new(ClaudeBrain::new(ClaudeConfig {
                binary: s.claude.binary.clone(),
                model: Some(claude_model(&s)),
                cwd: Some(state.data_dir.clone()),
                ..Default::default()
            })),
            BrainKind::Codex => Arc::new(CodexBrain::new(CodexConfig {
                binary: s.codex.binary.clone(),
                model: s.codex.model.clone().filter(|m| !m.trim().is_empty()),
                work_dir: state.data_dir.join("codex"),
                ..Default::default()
            })),
            BrainKind::Local => Arc::new(LocalBrain::new(local_config(&s))),
            BrainKind::Mock => unreachable!(),
        };
        (brain, Platform::Windows(Arc::new(WindowsDesktop::new())))
    };
    *state.platform.lock().unwrap() = Some(platform.clone());
    let (perception, actuator): (Arc<dyn Perception>, Arc<dyn Actuator>) = match &platform {
        Platform::Fake(d) => (d.clone(), d.clone()),
        Platform::Windows(w) => (w.clone(), w.clone()),
    };

    let (tx, mut rx) = mpsc::channel::<CharacterEvent>(64);
    let config = EngineConfig { log: Some(Arc::new(|m: &str| log(m))), ..Default::default() };
    let (engine, handle) = TaskEngine::new(brain, perception, actuator, SafetyGate::with_defaults(), state.memory.clone(), tx, config);
    handle.set_narration(s.narration);
    *state.engine.lock().unwrap() = Some(handle);

    // Forward engine events to the character window. Screen bounds from the real platform
    // are physical pixels; the window works in logical pixels.
    let fwd = app.clone();
    let scale = if matches!(platform, Platform::Windows(_)) { *state.scale.lock().unwrap() } else { 1.0 };
    let link = state.link.lock().unwrap().clone();
    tauri::async_runtime::spawn(async move {
        let _ = fwd.emit("mochi://event", CharacterEvent::Wake);
        while let Some(ev) = rx.recv().await {
            if let (Some(l), Some(m)) = (&link, mirror(&ev)) {
                l.send(m);
            }
            let ev = match ev {
                CharacterEvent::MoveTo { bounds, monitor_id } if scale != 1.0 => CharacterEvent::MoveTo {
                    bounds: Bounds { x: bounds.x / scale, y: bounds.y / scale, w: bounds.w / scale, h: bounds.h / scale },
                    monitor_id,
                },
                other => other,
            };
            let _ = fwd.emit("mochi://event", ev);
        }
    });
    let done = app.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = engine.run(task).await;
        log(&format!("task finished: {outcome:?}"));
        if let Some(s) = done.try_state::<AppState>() {
            *s.engine.lock().unwrap() = None;
            *s.platform.lock().unwrap() = None;
        }
        let _ = done.emit("mochi://task-finished", format!("{outcome:?}"));
        let link = done.state::<AppState>().link.lock().unwrap().clone();
        if let Some(l) = link {
            tokio::time::sleep(Duration::from_millis(1800)).await;
            l.send(DesktopMessage::State { state: CharacterState::Idle, line: None });
        }
    });
    Ok(())
}

/// What the phone shows for each character event (its face mirrors the desktop).
fn mirror(ev: &CharacterEvent) -> Option<DesktopMessage> {
    use CharacterState as S;
    Some(match ev {
        CharacterEvent::Wake => DesktopMessage::State { state: S::Listening, line: None },
        CharacterEvent::Thinking { on: true } => DesktopMessage::State { state: S::Thinking, line: None },
        CharacterEvent::Thinking { on: false } => DesktopMessage::State { state: S::Working, line: None },
        CharacterEvent::Narrate { text } | CharacterEvent::Say { text } => DesktopMessage::Narrate { text: text.clone(), audio_url: None },
        CharacterEvent::Lost { looking_for } => DesktopMessage::State { state: S::Lost, line: Some(format!("Can't find {looking_for}… show me?")) },
        CharacterEvent::Learning => DesktopMessage::State { state: S::Learning, line: None },
        CharacterEvent::ConfirmRequest { text } => DesktopMessage::ConfirmRequest { text: text.clone() },
        CharacterEvent::ConfirmAnswered { .. } => DesktopMessage::State { state: S::Working, line: None },
        CharacterEvent::Done { summary, kind } => DesktopMessage::Done {
            summary: summary.clone(),
            kind: if matches!(kind, DoneKind::Heart) { "heart".into() } else { "thumbs_up".into() },
        },
        CharacterEvent::Cancel => DesktopMessage::State { state: S::Idle, line: None },
        CharacterEvent::MoveTo { .. } | CharacterEvent::Gesture { .. } => return None,
    })
}

// ---------- phone link ----------

#[tauri::command]
fn link_status(state: State<'_, AppState>) -> LinkStatus {
    let link = state.link.lock().unwrap().clone();
    LinkStatus {
        port: *state.link_port.lock().unwrap(),
        desktop_name: hostname::get().map(|h| h.to_string_lossy().to_string()).unwrap_or_else(|_| "Mochi desktop".into()),
        connected: link.as_ref().and_then(|l| l.connected()).map(|c| c.1),
        devices: link.as_ref().map(|l| l.devices()).unwrap_or_default(),
    }
}

#[tauri::command]
fn pair_answer(state: State<'_, AppState>, session: u64, yes: bool) {
    log(&format!("link: desktop answered session {session}: {}", if yes { "yes" } else { "no" }));
    if let Some(l) = state.link.lock().unwrap().as_ref() {
        l.approve(session, yes);
    }
}

#[tauri::command]
fn forget_device(state: State<'_, AppState>, device_id: String) {
    if let Some(l) = state.link.lock().unwrap().as_ref() {
        l.forget(&device_id);
    }
}

/// Close Mochi for real: stop any task, tell the phone, then exit.
#[tauri::command]
async fn quit_app(app: AppHandle) {
    log("quit requested");
    {
        let st = app.state::<AppState>();
        if let Some(h) = st.engine.lock().unwrap().as_ref() { h.cancel(); }
        if let Some(l) = st.link.lock().unwrap().as_ref() { l.send(DesktopMessage::Sleepy); }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    app.exit(0);
}

/// Start the WebSocket server and route phone events into the app.
fn spawn_link(app: AppHandle, data_dir: PathBuf) {
    tauri::async_runtime::spawn(async move {
        let (tx, mut rx) = mpsc::channel::<LinkEvent>(64);
        let cfg = LinkConfig { devices_path: data_dir.join("devices.json"), ..Default::default() };
        let link = match Link::start(cfg, tx).await {
            Ok(l) => l,
            Err(e) => { log(&format!("phone link not started: {e}")); return; }
        };
        log(&format!("phone link listening on port {}", link.addr.port()));
        {
            let st = app.state::<AppState>();
            *st.link.lock().unwrap() = Some(link.handle.clone());
            *st.link_port.lock().unwrap() = Some(link.addr.port());
        }
        let _keep = link._mdns_keepalive();
        while let Some(ev) = rx.recv().await {
            match ev {
                LinkEvent::PairRequest { session, device_name, code } => {
                    log(&format!("link: pair request from {device_name}, code {code}"));
                    let _ = app.emit("mochi://pair-request", serde_json::json!({ "session": session, "device_name": device_name, "code": code }));
                }
                LinkEvent::PairProgress { .. } => log(&format!("link: {ev:?}")),
                LinkEvent::Paired { .. } | LinkEvent::PairFailed { .. } | LinkEvent::Connected { .. } | LinkEvent::Disconnected { .. } => {
                    log(&format!("link: {ev:?}"));
                    let _ = app.emit("mochi://link", format!("{ev:?}"));
                }
                LinkEvent::Wake => {
                    let busy = app.state::<AppState>().engine.lock().unwrap().is_some();
                    if !busy {
                        let _ = app.emit("mochi://event", CharacterEvent::Wake);
                    }
                }
                LinkEvent::Utterance { text, is_final } => {
                    if is_final && !text.trim().is_empty() {
                        let busy = app.state::<AppState>().engine.lock().unwrap().is_some();
                        let lower = text.trim().to_lowercase();
                        if busy {
                            // While working, only yes/no/stop are meaningful; everything else is ignored.
                            let h = app.state::<AppState>().engine.lock().unwrap().clone();
                            if let Some(h) = h {
                                if ["yes", "yep", "yeah", "go", "send it", "do it", "okay", "ok", "sure"].contains(&lower.as_str()) { h.answer(true).await; }
                                else if ["no", "nope", "stop", "wait", "cancel", "don't", "dont"].contains(&lower.as_str()) { h.answer(false).await; }
                                else if lower.contains("stop") { h.cancel(); }
                            }
                        } else {
                            log(&format!("task from phone: {text:?}"));
                            if let Err(e) = begin_task(&app, text, None).await {
                                log(&format!("task from phone refused: {e}"));
                            }
                        }
                    }
                }
                LinkEvent::UserSpeaking => { let _ = app.emit("mochi://interrupt", ()); }
                LinkEvent::Answer { yes } => {
                    let h = app.state::<AppState>().engine.lock().unwrap().clone();
                    if let Some(h) = h { h.answer(yes).await; }
                }
                LinkEvent::Stop => {
                    if let Some(h) = app.state::<AppState>().engine.lock().unwrap().as_ref() { h.cancel(); }
                }
            }
        }
    });
}

#[tauri::command]
async fn answer(state: State<'_, AppState>, yes: bool) -> Result<(), String> {
    let h = state.engine.lock().unwrap().clone();
    if let Some(h) = h {
        h.answer(yes).await;
    }
    Ok(())
}

/// The user pointed at the thing Mochi could not find. Real platform: the element under
/// the cursor. Mock: the Reply button the scenario was missing.
#[tauri::command]
async fn teach(state: State<'_, AppState>, element_id: Option<u32>) -> Result<(), String> {
    let h = state.engine.lock().unwrap().clone();
    let p = state.platform.lock().unwrap().clone();
    let (Some(h), Some(p)) = (h, p) else { return Ok(()) };
    match p {
        Platform::Fake(d) => {
            let screen = d.snapshot(false).await.map_err(|e| e.to_string())?;
            h.teach(screen, element_id.unwrap_or(mochi_brain_mock::ids::REPLY)).await;
        }
        Platform::Windows(w) => {
            if let Some((screen, id)) = w.element_under_cursor().await.map_err(|e| e.to_string())? {
                h.teach(screen, id).await;
            }
        }
    }
    Ok(())
}

#[tauri::command]
fn set_narration(state: State<'_, AppState>, on: bool) {
    state.settings.lock().unwrap().narration = on;
    if let Some(h) = state.engine.lock().unwrap().as_ref() {
        h.set_narration(on);
    }
}

#[tauri::command]
fn cancel_task(state: State<'_, AppState>) {
    if let Some(h) = state.engine.lock().unwrap().as_ref() {
        h.cancel();
    }
}

// ---------- memory ----------

#[tauri::command]
fn list_targets(state: State<'_, AppState>) -> Result<Vec<LearnedTarget>, String> {
    state.memory.all_targets().map_err(|e| e.to_string())
}

#[tauri::command]
fn forget_target(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.memory.forget(id).map_err(|e| e.to_string())
}

#[tauri::command]
fn wipe_memory(state: State<'_, AppState>) -> Result<(), String> {
    state.memory.wipe().map_err(|e| e.to_string())
}

#[tauri::command]
fn app_setting(state: State<'_, AppState>, app_id: String) -> Result<mochi_core::memory::AppSetting, String> {
    state.memory.app_setting(&app_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_app_setting(state: State<'_, AppState>, setting: mochi_core::memory::AppSetting) -> Result<(), String> {
    state.memory.set_app_setting(&setting).map_err(|e| e.to_string())
}

// ---------- settings window ----------

#[tauri::command]
fn open_settings(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    let r = WebviewWindowBuilder::new(&app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Mochi settings")
        .inner_size(520.0, 680.0)
        .min_inner_size(420.0, 480.0)
        .decorations(true)
        .resizable(true)
        .always_on_top(false)
        .focused(true)
        .center()
        .build();
    match r {
        Ok(_) => { log("settings window created"); Ok(()) }
        Err(e) => { log(&format!("settings window failed: {e}")); Err(e.to_string()) }
    }
}

/// Poll the global cursor at ~60 Hz: emit it for the eyes, and toggle click-through so
/// the window ignores the mouse everywhere except over Mochi (and its menu).
fn spawn_cursor_loop(window: WebviewWindow, body: Arc<Mutex<Option<BodyRect>>>) {
    tauri::async_runtime::spawn(async move {
        let mut over_body = false;
        let mut button_was_down = false;
        let _ = window.set_ignore_cursor_events(true);
        loop {
            tokio::time::sleep(Duration::from_millis(16)).await;
            // Global click edge (release), used by the teaching loop while Mochi is lost.
            let down = mochi_platform_windows::left_button_down();
            if button_was_down && !down && !over_body {
                let _ = window.emit("mochi://global-click", ());
            }
            button_was_down = down;
            let Ok(pos) = window.cursor_position() else { continue };
            let scale = window.scale_factor().unwrap_or(1.0);
            let (x, y) = (pos.x / scale, pos.y / scale);
            let _ = window.emit("mochi://cursor", serde_json::json!({ "x": x, "y": y }));
            let origin = window.outer_position().map(|p| (p.x as f64 / scale, p.y as f64 / scale)).unwrap_or((0.0, 0.0));
            let (lx, ly) = (x - origin.0, y - origin.1);
            let hit = body
                .lock()
                .unwrap()
                .map(|b| lx >= b.x - 6.0 && lx <= b.x + b.w + 6.0 && ly >= b.y - 6.0 && ly <= b.y + b.h + 6.0)
                .unwrap_or(false);
            if hit != over_body {
                over_body = hit;
                let _ = window.set_ignore_cursor_events(!hit);
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir().expect("app data dir");
            std::fs::create_dir_all(&data_dir).ok();
            let _ = LOG_PATH.set(data_dir.join("mochi.log"));
            log(&format!("Mochi {} starting; data dir {}", env!("CARGO_PKG_VERSION"), data_dir.display()));
            let memory = Arc::new(Memory::open(data_dir.join("mochi.sqlite")).expect("open memory db"));
            let mut settings = Settings::load(&data_dir);
            if settings.brain == BrainKind::Mock {
                settings.brain = BrainKind::ClaudeCode; // the demo brain is no longer offered in the UI
            }
            app.manage(AppState {
                data_dir,
                settings: Mutex::new(settings),
                engine: Mutex::new(None),
                platform: Mutex::new(None),
                memory,
                scale: Mutex::new(1.0),
                link: Mutex::new(None),
                link_port: Mutex::new(None),
            });
            spawn_link(app.handle().clone(), app.path().app_data_dir().expect("app data dir"));

            let window = app.get_webview_window("mochi").expect("mochi window");
            if let Ok(Some(m)) = window.current_monitor() {
                let _ = window.set_position(*m.position());
                let _ = window.set_size(*m.size());
            }
            let body: Arc<Mutex<Option<BodyRect>>> = Arc::new(Mutex::new(None));
            let b2 = body.clone();
            app.listen("mochi://body-rect", move |e| {
                if let Ok(r) = serde_json::from_str::<BodyRect>(e.payload()) {
                    *b2.lock().unwrap() = Some(r);
                }
            });
            spawn_cursor_loop(window, body);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            primary_monitor,
            get_settings,
            set_settings,
            probe_brain,
            start_task,
            answer,
            teach,
            set_narration,
            cancel_task,
            list_targets,
            forget_target,
            wipe_memory,
            app_setting,
            set_app_setting,
            open_settings,
            link_status,
            pair_answer,
            forget_device,
            log_js,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running Mochi");
}
