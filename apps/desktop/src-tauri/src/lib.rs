//! Mochi desktop connector (Tauri 2). Phase 1: the character window + the task engine
//! running the mock brain against a fake Gmail screen. Phase 2 swaps `FakeDesktop` for
//! Windows UI Automation / SendInput and `MockBrain` for the Claude Code CLI adapter.

use mochi_brain_mock::{FakeDesktop, MockBrain};
use mochi_core::engine::EngineConfig;
use mochi_core::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Listener, Manager, State, WebviewWindow};
use tokio::sync::mpsc;

#[derive(Default)]
struct AppState {
    engine: Mutex<Option<EngineHandle>>,
    /// Last fake screen used, so `teach` can hand the engine a snapshot + element.
    desktop: Mutex<Option<Arc<FakeDesktop>>>,
    narration: Mutex<bool>,
    memory: Mutex<Option<Arc<Memory>>>,
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

#[tauri::command]
fn primary_monitor(window: WebviewWindow) -> Result<MonitorDto, String> {
    let m = window.current_monitor().map_err(|e| e.to_string())?.or_else(|| window.primary_monitor().ok().flatten());
    let Some(m) = m else { return Err("no monitor".into()) };
    let p = m.position();
    let s = m.size();
    let scale = m.scale_factor();
    Ok(MonitorDto {
        id: 0,
        x: p.x as f64 / scale,
        y: p.y as f64 / scale,
        w: s.width as f64 / scale,
        h: s.height as f64 / scale,
        scale,
        primary: true,
    })
}

#[tauri::command]
async fn start_task(app: AppHandle, state: State<'_, AppState>, scenario: String, task: String) -> Result<(), String> {
    if state.engine.lock().unwrap().is_some() {
        return Err("a task is already running".into());
    }
    let memory = {
        let mut g = state.memory.lock().unwrap();
        if g.is_none() {
            let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            *g = Some(Arc::new(Memory::open(dir.join("mochi.sqlite")).map_err(|e| e.to_string())?));
        }
        g.clone().unwrap()
    };
    let desktop = Arc::new(if scenario == "close_unsaved" { FakeDesktop::unsaved_doc() } else { FakeDesktop::gmail() });
    *state.desktop.lock().unwrap() = Some(desktop.clone());

    let (tx, mut rx) = mpsc::channel::<CharacterEvent>(64);
    let (engine, handle) = TaskEngine::new(
        Arc::new(MockBrain::scenario(&scenario)),
        desktop.clone(),
        desktop,
        SafetyGate::with_defaults(),
        memory,
        tx,
        EngineConfig::default(),
    );
    handle.set_narration(*state.narration.lock().unwrap());
    *state.engine.lock().unwrap() = Some(handle);

    // Forward engine events to the character window (and, in phase 3, the phone).
    let fwd = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = fwd.emit("mochi://event", CharacterEvent::Wake);
        while let Some(ev) = rx.recv().await {
            let _ = fwd.emit("mochi://event", ev);
        }
    });
    let done = app.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = engine.run(task).await;
        tracing_log(&format!("task finished: {outcome:?}"));
        if let Some(s) = done.try_state::<AppState>() {
            *s.engine.lock().unwrap() = None;
        }
    });
    Ok(())
}

#[tauri::command]
async fn answer(state: State<'_, AppState>, yes: bool) -> Result<(), String> {
    let h = state.engine.lock().unwrap().clone();
    if let Some(h) = h {
        h.answer(yes).await;
    }
    Ok(())
}

/// Phase 1: with the fake screen, teaching picks the element the scenario was missing
/// (Reply). Phase 2 reads the real element under the cursor via UI Automation.
#[tauri::command]
async fn teach(state: State<'_, AppState>, element_id: Option<u32>) -> Result<(), String> {
    let h = state.engine.lock().unwrap().clone();
    let d = state.desktop.lock().unwrap().clone();
    if let (Some(h), Some(d)) = (h, d) {
        let screen = d.snapshot(false).await.map_err(|e| e.to_string())?;
        h.teach(screen, element_id.unwrap_or(mochi_brain_mock::ids::REPLY)).await;
    }
    Ok(())
}

#[tauri::command]
fn set_narration(state: State<'_, AppState>, on: bool) {
    *state.narration.lock().unwrap() = on;
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

fn tracing_log(msg: &str) {
    eprintln!("[mochi] {msg}");
}

/// Poll the global cursor at ~60 Hz: emit it for the eyes, and toggle click-through so
/// the window ignores the mouse everywhere except over Mochi's body.
fn spawn_cursor_loop(window: WebviewWindow, body: Arc<Mutex<Option<BodyRect>>>) {
    tauri::async_runtime::spawn(async move {
        let mut over_body = false;
        let _ = window.set_ignore_cursor_events(true);
        loop {
            tokio::time::sleep(Duration::from_millis(16)).await;
            let Ok(pos) = window.cursor_position() else { continue };
            let scale = window.scale_factor().unwrap_or(1.0);
            let (x, y) = (pos.x / scale, pos.y / scale);
            let _ = window.emit("mochi://cursor", serde_json::json!({ "x": x, "y": y }));

            // Body rect arrives in window-local logical px; convert the cursor to the same space.
            let origin = window.outer_position().map(|p| (p.x as f64 / scale, p.y as f64 / scale)).unwrap_or((0.0, 0.0));
            let (lx, ly) = (x - origin.0, y - origin.1);
            let hit = body
                .lock()
                .unwrap()
                .map(|b| lx >= b.x - 6.0 && lx <= b.x + b.w + 6.0 && ly >= b.y - 48.0 && ly <= b.y + b.h + 6.0)
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
        .manage(AppState { narration: Mutex::new(true), ..Default::default() })
        .setup(|app| {
            let window = app.get_webview_window("mochi").expect("mochi window");
            // Cover the monitor the cursor is on; Mochi positions itself inside.
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
        .invoke_handler(tauri::generate_handler![primary_monitor, start_task, answer, teach, set_narration, cancel_task])
        .run(tauri::generate_context!())
        .expect("error while running Mochi");
}
