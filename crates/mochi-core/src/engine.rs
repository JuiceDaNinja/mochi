//! The task execution loop: snapshot → brain → gate → execute, one action per cycle.
//! Emits [`CharacterEvent`]s for the runtime and waits on the user for confirms and teaching.

use crate::brain::{is_terminal, trim_narration, Brain, BrainInput};
use crate::gate::{GateVerdict, SafetyGate};
use crate::memory::Memory;
use crate::perception::{Actuator, Perception};
use crate::runtime::{lines, CharacterEvent, DoneKind, Gesture};
use crate::types::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAnswer {
    Yes,
    No,
}

/// The user pointed at something while Mochi was Lost.
#[derive(Debug, Clone)]
pub struct TeachSignal {
    pub screen: ScreenSnapshot,
    pub element_id: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskOutcome {
    Done { summary: String },
    /// User said no, or the confirm timed out.
    Declined { description: String },
    /// Gate hard limit or app not allowed.
    Blocked { reason: String },
    /// Lost and nobody helped, or too many cycles.
    GaveUp { reason: String },
    Cancelled,
    Failed { error: String },
}

/// What the app holds to talk to a running task.
#[derive(Clone)]
pub struct EngineHandle {
    confirm_tx: mpsc::Sender<ConfirmAnswer>,
    teach_tx: mpsc::Sender<TeachSignal>,
    cancel_tx: watch::Sender<bool>,
    narration_on: Arc<AtomicBool>,
}

impl EngineHandle {
    pub async fn answer(&self, yes: bool) {
        let _ = self.confirm_tx.send(if yes { ConfirmAnswer::Yes } else { ConfirmAnswer::No }).await;
    }
    pub async fn teach(&self, screen: ScreenSnapshot, element_id: u32) {
        let _ = self.teach_tx.send(TeachSignal { screen, element_id }).await;
    }
    pub fn cancel(&self) {
        let _ = self.cancel_tx.send(true);
    }
    pub fn set_narration(&self, on: bool) {
        self.narration_on.store(on, Ordering::Relaxed);
    }
    pub fn narration(&self) -> bool {
        self.narration_on.load(Ordering::Relaxed)
    }
}

pub struct EngineConfig {
    pub confirm_timeout: Duration,
    pub lost_timeout: Duration,
    pub max_cycles: usize,
    pub screenshot_when_tree_smaller_than: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            confirm_timeout: Duration::from_secs(crate::protocol::CONFIRM_TIMEOUT_SECS),
            lost_timeout: Duration::from_secs(120),
            max_cycles: 40,
            screenshot_when_tree_smaller_than: 5,
        }
    }
}

pub struct TaskEngine {
    brain: Arc<dyn Brain>,
    perception: Arc<dyn Perception>,
    actuator: Arc<dyn Actuator>,
    gate: SafetyGate,
    memory: Arc<Memory>,
    events: mpsc::Sender<CharacterEvent>,
    config: EngineConfig,
    confirm_rx: mpsc::Receiver<ConfirmAnswer>,
    teach_rx: mpsc::Receiver<TeachSignal>,
    cancel_rx: watch::Receiver<bool>,
    narration_on: Arc<AtomicBool>,
}

impl TaskEngine {
    pub fn new(
        brain: Arc<dyn Brain>,
        perception: Arc<dyn Perception>,
        actuator: Arc<dyn Actuator>,
        gate: SafetyGate,
        memory: Arc<Memory>,
        events: mpsc::Sender<CharacterEvent>,
        config: EngineConfig,
    ) -> (Self, EngineHandle) {
        let (confirm_tx, confirm_rx) = mpsc::channel(4);
        let (teach_tx, teach_rx) = mpsc::channel(4);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let narration_on = Arc::new(AtomicBool::new(true));
        let handle = EngineHandle { confirm_tx, teach_tx, cancel_tx, narration_on: narration_on.clone() };
        let engine = Self {
            brain,
            perception,
            actuator,
            gate,
            memory,
            events,
            config,
            confirm_rx,
            teach_rx,
            cancel_rx,
            narration_on,
        };
        (engine, handle)
    }

    pub async fn run(mut self, task: String) -> TaskOutcome {
        let outcome = self.run_inner(&task).await;
        self.brain.end_task().await;
        if !matches!(outcome, TaskOutcome::Done { .. }) {
            self.emit(CharacterEvent::Cancel).await;
        }
        outcome
    }

    async fn run_inner(&mut self, task: &str) -> TaskOutcome {
        self.emit(CharacterEvent::Say { text: pick(lines::START) }).await;
        let mut history: Vec<Step> = vec![];
        let mut said_remembered = false;

        for _cycle in 0..self.config.max_cycles {
            if self.cancelled() {
                return TaskOutcome::Cancelled;
            }

            // 1. Snapshot (tree first, screenshot only when thin and the brain can see).
            let mut screen = match self.perception.snapshot(false).await {
                Ok(s) => s,
                Err(e) => return TaskOutcome::Failed { error: e.to_string() },
            };
            if self.brain.capabilities().vision && screen.elements.len() < self.config.screenshot_when_tree_smaller_than {
                if let Ok(s) = self.perception.snapshot(true).await {
                    screen = s;
                }
            }

            // 2. Allow list.
            let setting = self.memory.app_setting(&screen.window.app_id).unwrap_or_else(|_| crate::memory::AppSetting {
                app_id: screen.window.app_id.clone(),
                narration_on: true,
                extra_irreversible_labels: vec![],
                allowed: true,
            });
            if !setting.allowed {
                let reason = "Mochi isn't allowed in this app. You can turn it on in settings.".to_string();
                self.emit(CharacterEvent::Say { text: reason.clone() }).await;
                return TaskOutcome::Blocked { reason };
            }
            self.gate.add_app_labels(&screen.window.app_id, &setting.extra_irreversible_labels);
            let narrate_here = self.narration_on.load(Ordering::Relaxed) && setting.narration_on;

            // 3. Memory hints.
            let resolved = self.memory.resolve(&screen).unwrap_or_default();
            screen.remembered = resolved.iter().map(|(t, id)| RememberedHint { phrase: t.user_phrase.clone(), element_id: *id }).collect();
            let memory_rows = resolved.iter().map(|(t, _)| t.clone()).collect();

            // 4. Ask the brain; speak narration as it streams.
            self.emit(CharacterEvent::Thinking { on: true }).await;
            let (tx, mut rx) = mpsc::channel(8);
            let input = BrainInput { task: task.to_string(), screen: screen.clone(), memory: memory_rows, history: history.clone() };
            let brain = self.brain.clone();
            let call = tokio::spawn(async move { brain.next_step(input, tx).await });
            let mut terminal: Option<StepEvent> = None;
            loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Some(StepEvent::Narrate { text }) => {
                            if narrate_here {
                                self.emit(CharacterEvent::Narrate { text: trim_narration(&text) }).await;
                            }
                        }
                        Some(ev) if is_terminal(&ev) => { terminal = Some(ev); break; }
                        Some(_) => {}
                        None => break,
                    },
                    _ = self.cancel_rx.changed() => { call.abort(); self.emit(CharacterEvent::Thinking { on: false }).await; return TaskOutcome::Cancelled; }
                }
            }
            self.emit(CharacterEvent::Thinking { on: false }).await;
            let terminal = match terminal {
                Some(t) => t,
                None => match call.await {
                    Ok(Err(e)) => {
                        self.emit(CharacterEvent::Say { text: pick(lines::SLEEPY) }).await;
                        return TaskOutcome::Failed { error: e.to_string() };
                    }
                    _ => return TaskOutcome::Failed { error: "brain ended its turn without an action".into() },
                },
            };

            // 5. Handle the terminal event.
            match terminal {
                StepEvent::Done { summary, sent_user_words } => {
                    let kind = if sent_user_words { DoneKind::Heart } else { DoneKind::ThumbsUp };
                    self.emit(CharacterEvent::Done { kind, summary: summary.clone() }).await;
                    return TaskOutcome::Done { summary };
                }
                StepEvent::Lost { looking_for } => {
                    self.emit(CharacterEvent::Lost { looking_for: looking_for.clone() }).await;
                    match self.wait_for_teaching().await {
                        Some(sig) => {
                            if let Some(el) = sig.screen.element(sig.element_id).cloned() {
                                let _ = self.memory.learn(&sig.screen, &el, &looking_for);
                                self.emit(CharacterEvent::Learning).await;
                                history.push(Step {
                                    action: Action::ClickPoint { x: el.bounds.center().0, y: el.bounds.center().1 },
                                    ok: true,
                                    note: format!("user showed me \"{looking_for}\": element labelled \"{}\" ({})", el.label, el.role),
                                });
                            }
                        }
                        None => {
                            if self.cancelled() {
                                return TaskOutcome::Cancelled;
                            }
                            self.emit(CharacterEvent::Say { text: pick(lines::TIMEOUT) }).await;
                            return TaskOutcome::GaveUp { reason: format!("could not find {looking_for}") };
                        }
                    }
                }
                StepEvent::Ask { question } => {
                    self.emit(CharacterEvent::ConfirmRequest { text: question.clone() }).await;
                    match self.wait_for_answer().await {
                        Some(ConfirmAnswer::Yes) => {
                            self.emit(CharacterEvent::ConfirmAnswered { yes: true }).await;
                            history.push(Step { action: Action::Key { combo: String::new() }, ok: true, note: format!("user answered yes to: {question}") });
                        }
                        _ => {
                            self.emit(CharacterEvent::ConfirmAnswered { yes: false }).await;
                            return TaskOutcome::Declined { description: question };
                        }
                    }
                }
                StepEvent::Act { action, irreversible } => {
                    // Remembered target used? Say so the first time.
                    if let Action::ClickElement { element_id } = &action {
                        if let Some((t, _)) = resolved.iter().find(|(_, id)| id == element_id) {
                            if let Some(el) = screen.element(*element_id) {
                                let _ = self.memory.record_hit(t.id, &screen, el);
                            }
                            if !said_remembered && narrate_here {
                                self.emit(CharacterEvent::Say { text: pick(lines::REMEMBERED) }).await;
                                said_remembered = true;
                            }
                        }
                    }

                    let target = target_bounds(&action, &screen);
                    self.emit(CharacterEvent::MoveTo { bounds: target, monitor_id: screen.window.monitor_id }).await;

                    match self.gate.classify(&action, &screen, irreversible) {
                        GateVerdict::Blocked { reason } => {
                            let _ = self.memory.log_gate(&screen.window.app_id, &label_of(&action, &screen), "blocked", "blocked");
                            self.emit(CharacterEvent::Say { text: reason.clone() }).await;
                            return TaskOutcome::Blocked { reason };
                        }
                        GateVerdict::Irreversible { description, rule } => {
                            self.emit(CharacterEvent::ConfirmRequest { text: description.clone() }).await;
                            let answer = self.wait_for_answer().await;
                            let label = label_of(&action, &screen);
                            match answer {
                                Some(ConfirmAnswer::Yes) => {
                                    let _ = self.memory.log_gate(&screen.window.app_id, &label, &rule, "yes");
                                    self.emit(CharacterEvent::ConfirmAnswered { yes: true }).await;
                                    self.emit(CharacterEvent::Say { text: pick(lines::AFTER_YES) }).await;
                                }
                                Some(ConfirmAnswer::No) => {
                                    let _ = self.memory.log_gate(&screen.window.app_id, &label, &rule, "no");
                                    self.emit(CharacterEvent::ConfirmAnswered { yes: false }).await;
                                    self.emit(CharacterEvent::Say { text: pick(lines::AFTER_NO) }).await;
                                    return TaskOutcome::Declined { description };
                                }
                                None => {
                                    if self.cancelled() {
                                        return TaskOutcome::Cancelled;
                                    }
                                    let _ = self.memory.log_gate(&screen.window.app_id, &label, &rule, "timeout");
                                    self.emit(CharacterEvent::ConfirmAnswered { yes: false }).await;
                                    self.emit(CharacterEvent::Say { text: pick(lines::TIMEOUT) }).await;
                                    return TaskOutcome::Declined { description };
                                }
                            }
                        }
                        GateVerdict::Safe => {}
                    }

                    if let Some(g) = gesture_for(&action) {
                        self.emit(CharacterEvent::Gesture { gesture: g }).await;
                    }
                    let result = self.actuator.perform(&action, &screen).await;
                    let (ok, note) = match &result {
                        Ok(()) => (true, String::new()),
                        Err(e) => (false, e.to_string()),
                    };
                    history.push(Step { action, ok, note });
                }
                StepEvent::Narrate { .. } => unreachable!("narration is not terminal"),
            }
        }
        self.emit(CharacterEvent::Say { text: "This one's too long for me… sorry.".into() }).await;
        TaskOutcome::GaveUp { reason: "max cycles reached".into() }
    }

    async fn wait_for_answer(&mut self) -> Option<ConfirmAnswer> {
        // Drain stale answers so an old "yes" never releases a new action.
        while self.confirm_rx.try_recv().is_ok() {}
        tokio::select! {
            a = self.confirm_rx.recv() => a,
            _ = tokio::time::sleep(self.config.confirm_timeout) => None,
            _ = self.cancel_rx.changed() => None,
        }
    }

    async fn wait_for_teaching(&mut self) -> Option<TeachSignal> {
        while self.teach_rx.try_recv().is_ok() {}
        tokio::select! {
            t = self.teach_rx.recv() => t,
            _ = tokio::time::sleep(self.config.lost_timeout) => None,
            _ = self.cancel_rx.changed() => None,
        }
    }

    fn cancelled(&self) -> bool {
        *self.cancel_rx.borrow()
    }

    async fn emit(&self, ev: CharacterEvent) {
        let _ = self.events.send(ev).await;
    }
}

fn target_bounds(action: &Action, screen: &ScreenSnapshot) -> Bounds {
    let focused = || screen.elements.iter().find(|e| e.focused).map(|e| e.bounds).unwrap_or(screen.window.bounds);
    match action {
        Action::ClickElement { element_id } | Action::Scroll { element_id, .. } => {
            screen.element(*element_id).map(|e| e.bounds).unwrap_or(screen.window.bounds)
        }
        Action::ClickPoint { x, y } => Bounds { x: x - 8.0, y: y - 8.0, w: 16.0, h: 16.0 },
        Action::Type { .. } | Action::Key { .. } => focused(),
        Action::FocusWindow { .. } => screen.window.bounds,
    }
}

fn gesture_for(action: &Action) -> Option<Gesture> {
    match action {
        Action::ClickElement { .. } | Action::ClickPoint { .. } | Action::Scroll { .. } => Some(Gesture::Click),
        Action::Type { .. } | Action::Key { .. } => Some(Gesture::Type),
        Action::FocusWindow { .. } => None,
    }
}

fn label_of(action: &Action, screen: &ScreenSnapshot) -> String {
    match action {
        Action::ClickElement { element_id } => screen.element(*element_id).map(|e| e.label.clone()).unwrap_or_default(),
        Action::Key { combo } => combo.clone(),
        Action::Type { .. } => "type".into(),
        _ => String::new(),
    }
}

/// Deterministic-enough variety without pulling in a RNG crate.
pub fn pick(options: &[&str]) -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    options[n % options.len()].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// A brain that plays a fixed script, one terminal event per call.
    struct ScriptBrain {
        turns: Mutex<std::collections::VecDeque<Vec<StepEvent>>>,
    }
    #[async_trait]
    impl Brain for ScriptBrain {
        fn name(&self) -> &'static str { "script" }
        fn capabilities(&self) -> crate::brain::BrainCapabilities { crate::brain::BrainCapabilities { vision: false, streaming: true } }
        async fn next_step(&self, _input: BrainInput, out: mpsc::Sender<StepEvent>) -> Result<()> {
            let turn = self.turns.lock().unwrap().pop_front().unwrap_or_default();
            for ev in turn {
                out.send(ev).await.ok();
            }
            Ok(())
        }
    }

    struct FakeScreen(ScreenSnapshot);
    #[async_trait]
    impl Perception for FakeScreen {
        async fn snapshot(&self, _: bool) -> Result<ScreenSnapshot> { Ok(self.0.clone()) }
        async fn element_under_cursor(&self) -> Result<Option<(ScreenSnapshot, u32)>> { Ok(None) }
    }

    struct Recorder(Mutex<Vec<Action>>);
    #[async_trait]
    impl Actuator for Recorder {
        async fn perform(&self, action: &Action, _: &ScreenSnapshot) -> Result<()> {
            self.0.lock().unwrap().push(action.clone());
            Ok(())
        }
    }

    fn gmail() -> ScreenSnapshot {
        let el = |id, label: &str, focused| UiElement {
            id, role: "button".into(), label: label.into(), value: "".into(),
            bounds: Bounds { x: 100.0 * id as f64, y: 100.0, w: 80.0, h: 30.0 },
            interactive: true, focused, secure: false,
        };
        ScreenSnapshot {
            window: WindowInfo { app_id: "chrome.exe".into(), title: "Inbox - Gmail - Google Chrome".into(), bounds: Bounds { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }, monitor_id: 0, unsaved: false },
            monitors: vec![],
            elements: vec![el(1, "Reply", false), el(2, "Message body", true), el(3, "Send", false)],
            remembered: vec![],
            screenshot_png_b64: None,
        }
    }

    fn build(turns: Vec<Vec<StepEvent>>, timeout: Duration) -> (TaskEngine, EngineHandle, mpsc::Receiver<CharacterEvent>, Arc<Recorder>, Arc<Memory>) {
        let (tx, rx) = mpsc::channel(64);
        let rec = Arc::new(Recorder(Mutex::new(vec![])));
        let mem = Arc::new(Memory::in_memory().unwrap());
        let (engine, handle) = TaskEngine::new(
            Arc::new(ScriptBrain { turns: Mutex::new(turns.into()) }),
            Arc::new(FakeScreen(gmail())),
            rec.clone(),
            SafetyGate::with_defaults(),
            mem.clone(),
            tx,
            EngineConfig { confirm_timeout: timeout, lost_timeout: timeout, ..Default::default() },
        );
        (engine, handle, rx, rec, mem)
    }

    fn kinds(evs: &[CharacterEvent]) -> Vec<String> {
        evs.iter().map(|e| serde_json::to_value(e).unwrap()["type"].as_str().unwrap().to_string()).collect()
    }

    #[tokio::test]
    async fn gmail_reply_confirms_send_and_ends_with_a_heart() {
        let turns = vec![
            vec![StepEvent::Narrate { text: "Looking for the reply button".into() }, StepEvent::Act { action: Action::ClickElement { element_id: 1 }, irreversible: false }],
            vec![StepEvent::Narrate { text: "Typing now".into() }, StepEvent::Act { action: Action::Type { text: "I'll be there at 6".into() }, irreversible: false }],
            vec![StepEvent::Act { action: Action::ClickElement { element_id: 3 }, irreversible: false }],
            vec![StepEvent::Done { summary: "Reply sent".into(), sent_user_words: true }],
        ];
        let (engine, handle, mut rx, rec, _) = build(turns, Duration::from_secs(5));
        let run = tokio::spawn(engine.run("reply to Sara".into()));

        // Wait for the confirm request, then say yes.
        let mut seen = vec![];
        loop {
            let ev = rx.recv().await.unwrap();
            let is_confirm = matches!(ev, CharacterEvent::ConfirmRequest { .. });
            seen.push(ev);
            if is_confirm { break; }
        }
        assert!(matches!(seen.last(), Some(CharacterEvent::ConfirmRequest { text }) if text == "Send it?"));
        handle.answer(true).await;

        let outcome = run.await.unwrap();
        assert_eq!(outcome, TaskOutcome::Done { summary: "Reply sent".into() });
        while let Ok(ev) = rx.try_recv() { seen.push(ev); }

        let k = kinds(&seen);
        assert_eq!(k[0], "say", "starts with a start line");
        assert!(k.contains(&"narrate".to_string()));
        assert!(matches!(seen.last(), Some(CharacterEvent::Done { kind: DoneKind::Heart, .. })));
        assert_eq!(rec.0.lock().unwrap().len(), 3, "reply, type, send all executed");
        // Gestures: click, type, click
        let gestures: Vec<_> = seen.iter().filter_map(|e| if let CharacterEvent::Gesture { gesture } = e { Some(*gesture) } else { None }).collect();
        assert_eq!(gestures, vec![Gesture::Click, Gesture::Type, Gesture::Click]);
    }

    #[tokio::test]
    async fn a_no_stops_the_send_and_nothing_is_clicked() {
        let turns = vec![vec![StepEvent::Act { action: Action::ClickElement { element_id: 3 }, irreversible: false }]];
        let (engine, handle, mut rx, rec, mem) = build(turns, Duration::from_secs(5));
        let run = tokio::spawn(engine.run("send it".into()));
        loop {
            if matches!(rx.recv().await.unwrap(), CharacterEvent::ConfirmRequest { .. }) { break; }
        }
        handle.answer(false).await;
        assert_eq!(run.await.unwrap(), TaskOutcome::Declined { description: "Send it?".into() });
        assert!(rec.0.lock().unwrap().is_empty());
        assert_eq!(mem.gate_log_count("no").unwrap(), 1);
    }

    #[tokio::test]
    async fn confirm_timeout_counts_as_no() {
        let turns = vec![vec![StepEvent::Act { action: Action::ClickElement { element_id: 3 }, irreversible: false }]];
        let (engine, _handle, mut rx, rec, _) = build(turns, Duration::from_millis(50));
        let outcome = engine.run("send it".into()).await;
        assert!(matches!(outcome, TaskOutcome::Declined { .. }));
        assert!(rec.0.lock().unwrap().is_empty());
        let mut evs = vec![];
        while let Ok(e) = rx.try_recv() { evs.push(e); }
        assert!(evs.iter().any(|e| matches!(e, CharacterEvent::ConfirmAnswered { yes: false })));
    }

    #[tokio::test]
    async fn lost_then_taught_is_remembered_and_the_task_continues() {
        let turns = vec![
            vec![StepEvent::Lost { looking_for: "the reply button".into() }],
            vec![StepEvent::Act { action: Action::ClickElement { element_id: 1 }, irreversible: false }],
            vec![StepEvent::Done { summary: "opened reply".into(), sent_user_words: false }],
        ];
        let (engine, handle, mut rx, _rec, mem) = build(turns, Duration::from_secs(5));
        let run = tokio::spawn(engine.run("reply".into()));
        loop {
            if matches!(rx.recv().await.unwrap(), CharacterEvent::Lost { .. }) { break; }
        }
        handle.teach(gmail(), 1).await;
        assert_eq!(run.await.unwrap(), TaskOutcome::Done { summary: "opened reply".into() });

        let mut evs = vec![];
        while let Ok(e) = rx.try_recv() { evs.push(e); }
        let k = kinds(&evs);
        assert!(k.contains(&"learning".to_string()));
        assert!(evs.iter().any(|e| matches!(e, CharacterEvent::Say { text } if text == "I remember this one!")), "remembered target used on the next cycle");
        assert!(matches!(evs.last(), Some(CharacterEvent::Done { kind: DoneKind::ThumbsUp, .. })));
        let t = mem.all_targets().unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].user_phrase, "the reply button");
        assert_eq!(t[0].hits, 1);
    }

    #[tokio::test]
    async fn cancel_while_waiting_for_confirm() {
        let turns = vec![vec![StepEvent::Act { action: Action::ClickElement { element_id: 3 }, irreversible: false }]];
        let (engine, handle, mut rx, rec, _) = build(turns, Duration::from_secs(30));
        let run = tokio::spawn(engine.run("send".into()));
        loop {
            if matches!(rx.recv().await.unwrap(), CharacterEvent::ConfirmRequest { .. }) { break; }
        }
        handle.cancel();
        assert_eq!(run.await.unwrap(), TaskOutcome::Cancelled);
        assert!(rec.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn narration_toggle_silences_narrate_but_not_confirm() {
        let turns = vec![
            vec![StepEvent::Narrate { text: "Clicking send".into() }, StepEvent::Act { action: Action::ClickElement { element_id: 3 }, irreversible: false }],
        ];
        let (engine, handle, mut rx, _, _) = build(turns, Duration::from_millis(50));
        handle.set_narration(false);
        let _ = engine.run("send".into()).await;
        let mut evs = vec![];
        while let Ok(e) = rx.try_recv() { evs.push(e); }
        assert!(!evs.iter().any(|e| matches!(e, CharacterEvent::Narrate { .. })));
        assert!(evs.iter().any(|e| matches!(e, CharacterEvent::ConfirmRequest { .. })));
    }
}
