//! Scripted scenarios for building the character before a real brain exists.
//!
//! `MockBrain` replays a scenario one turn per `next_step`, with a small delay so the
//! Thinking bubble and narration pacing feel real. `FakeDesktop` is a Perception +
//! Actuator pair with a pretend Gmail window, laid out on a 1920x1080 primary monitor.

use async_trait::async_trait;
use mochi_core::*;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

pub struct MockBrain {
    turns: Mutex<VecDeque<Vec<StepEvent>>>,
    think_delay: Duration,
}

impl MockBrain {
    pub fn new(scenario: Vec<Vec<StepEvent>>, think_delay: Duration) -> Self {
        Self { turns: Mutex::new(scenario.into()), think_delay }
    }

    /// Picks a scenario by name; unknown names get the Gmail reply.
    pub fn scenario(name: &str) -> Self {
        let s = match name {
            "gmail_lost" => scenarios::gmail_reply_with_lost(),
            "close_unsaved" => scenarios::close_unsaved_doc(),
            "password" => scenarios::password_field(),
            _ => scenarios::gmail_reply(),
        };
        Self::new(s, Duration::from_millis(900))
    }
}

#[async_trait]
impl Brain for MockBrain {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn capabilities(&self) -> BrainCapabilities {
        BrainCapabilities { vision: false, streaming: true }
    }
    async fn next_step(&self, input: BrainInput, out: mpsc::Sender<StepEvent>) -> Result<()> {
        let turn = self.turns.lock().unwrap().pop_front();
        let Some(turn) = turn else {
            out.send(StepEvent::Done { summary: "Nothing left to do".into(), sent_user_words: false }).await.ok();
            return Ok(());
        };
        // Phase 1 realism: if the scenario says Lost but the user already taught us
        // this target, use the remembered element instead, like a real brain would.
        for ev in turn {
            tokio::time::sleep(self.think_delay / 2).await;
            let ev = match ev {
                StepEvent::Lost { looking_for } => match input.screen.remembered.iter().find(|h| h.phrase == looking_for) {
                    Some(hint) => StepEvent::Act { action: Action::ClickElement { element_id: hint.element_id }, irreversible: false },
                    None => StepEvent::Lost { looking_for },
                },
                other => other,
            };
            if out.send(ev).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

/// A pretend Gmail window. Elements use stable ids so scenarios can refer to them.
pub struct FakeDesktop {
    pub screen: Mutex<ScreenSnapshot>,
    pub performed: Mutex<Vec<Action>>,
}

pub mod ids {
    pub const GMAIL_TAB: u32 = 1;
    pub const INBOX_LIST: u32 = 2;
    pub const THREAD_SARA: u32 = 3;
    pub const REPLY: u32 = 4;
    pub const BODY: u32 = 5;
    pub const SEND: u32 = 6;
    pub const CLOSE: u32 = 7;
    pub const PASSWORD: u32 = 8;
}

impl FakeDesktop {
    pub fn gmail() -> Self {
        let el = |id, role: &str, label: &str, x, y, w, h, focused, secure| UiElement {
            id,
            role: role.into(),
            label: label.into(),
            value: String::new(),
            bounds: Bounds { x, y, w, h },
            interactive: true,
            focused,
            secure,
        };
        let screen = ScreenSnapshot {
            window: WindowInfo {
                app_id: "chrome.exe".into(),
                title: "Inbox (3) - Gmail - Google Chrome".into(),
                bounds: Bounds { x: 160.0, y: 60.0, w: 1600.0, h: 960.0 },
                monitor_id: 0,
                unsaved: false,
            },
            monitors: vec![MonitorInfo { id: 0, bounds: Bounds { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }, scale: 1.0, is_primary: true, has_cursor: true }],
            elements: vec![
                el(ids::GMAIL_TAB, "tab", "Inbox (3) - Gmail", 200.0, 70.0, 220.0, 32.0, false, false),
                el(ids::INBOX_LIST, "list", "Inbox", 420.0, 200.0, 1300.0, 600.0, false, false),
                el(ids::THREAD_SARA, "listitem", "Sara Kim — Dinner Friday?", 440.0, 230.0, 1260.0, 48.0, false, false),
                el(ids::REPLY, "button", "Reply", 460.0, 860.0, 90.0, 36.0, false, false),
                el(ids::BODY, "edit", "Message body", 460.0, 560.0, 1200.0, 260.0, true, false),
                el(ids::SEND, "button", "Send", 460.0, 940.0, 90.0, 36.0, false, false),
                el(ids::CLOSE, "button", "Close", 1720.0, 70.0, 28.0, 28.0, false, false),
                el(ids::PASSWORD, "edit", "Password", 900.0, 400.0, 300.0, 36.0, false, true),
            ],
            remembered: vec![],
            screenshot_png_b64: None,
        };
        Self { screen: Mutex::new(screen), performed: Mutex::new(vec![]) }
    }

    pub fn unsaved_doc() -> Self {
        let d = Self::gmail();
        {
            let mut s = d.screen.lock().unwrap();
            s.window.app_id = "notepad.exe".into();
            s.window.title = "*notes.txt - Notepad".into();
            s.window.unsaved = true;
        }
        d
    }
}

#[async_trait]
impl Perception for FakeDesktop {
    async fn snapshot(&self, _want_screenshot: bool) -> Result<ScreenSnapshot> {
        Ok(self.screen.lock().unwrap().clone())
    }
    async fn element_under_cursor(&self) -> Result<Option<(ScreenSnapshot, u32)>> {
        Ok(None)
    }
}

#[async_trait]
impl Actuator for FakeDesktop {
    async fn perform(&self, action: &Action, _screen: &ScreenSnapshot) -> Result<()> {
        tokio::time::sleep(Duration::from_millis(350)).await;
        self.performed.lock().unwrap().push(action.clone());
        // Make focus follow the clicks a little, so Type targets the body after Reply.
        if let Action::ClickElement { element_id } = action {
            let mut s = self.screen.lock().unwrap();
            for e in s.elements.iter_mut() {
                e.focused = e.id == *element_id || (*element_id == ids::REPLY && e.id == ids::BODY);
            }
        }
        Ok(())
    }
}

pub mod scenarios {
    use super::ids::*;
    use mochi_core::{Action, StepEvent};

    fn n(t: &str) -> StepEvent {
        StepEvent::Narrate { text: t.into() }
    }
    fn click(id: u32) -> StepEvent {
        StepEvent::Act { action: Action::ClickElement { element_id: id }, irreversible: false }
    }

    /// The bible's sample scenario, minus the Lost step.
    pub fn gmail_reply() -> Vec<Vec<StepEvent>> {
        vec![
            vec![n("Opening Gmail…"), StepEvent::Act { action: Action::FocusWindow { app_id: "chrome.exe".into(), title_contains: Some("Gmail".into()) }, irreversible: false }],
            vec![n("Looking for Sara…"), click(THREAD_SARA)],
            vec![n("Found it!"), click(REPLY)],
            vec![n("Typing now…"), StepEvent::Act { action: Action::Type { text: "I'll be there at 6!".into() }, irreversible: false }],
            vec![n("Almost there…"), click(SEND)],
            vec![StepEvent::Done { summary: "Reply sent to Sara".into(), sent_user_words: true }],
        ]
    }

    /// Same, but Mochi cannot find Reply and has to be shown.
    pub fn gmail_reply_with_lost() -> Vec<Vec<StepEvent>> {
        vec![
            vec![n("Opening Gmail…"), StepEvent::Act { action: Action::FocusWindow { app_id: "chrome.exe".into(), title_contains: Some("Gmail".into()) }, irreversible: false }],
            vec![n("Looking for Sara…"), click(THREAD_SARA)],
            vec![n("Found it!"), n("Now the reply button…"), StepEvent::Lost { looking_for: "the reply button".into() }],
            vec![n("Typing now…"), StepEvent::Act { action: Action::Type { text: "I'll be there at 6!".into() }, irreversible: false }],
            vec![n("Almost there…"), click(SEND)],
            vec![StepEvent::Done { summary: "Reply sent to Sara".into(), sent_user_words: true }],
        ]
    }

    pub fn close_unsaved_doc() -> Vec<Vec<StepEvent>> {
        vec![
            vec![n("Closing this window…"), StepEvent::Act { action: Action::Key { combo: "alt+f4".into() }, irreversible: false }],
            vec![StepEvent::Done { summary: "Closed Notepad".into(), sent_user_words: false }],
        ]
    }

    pub fn password_field() -> Vec<Vec<StepEvent>> {
        vec![
            vec![n("Clicking the password box…"), click(PASSWORD)],
            vec![n("Typing…"), StepEvent::Act { action: Action::Type { text: "hunter2".into() }, irreversible: false }],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn gmail_scenario_runs_end_to_end_through_the_engine() {
        let (tx, mut rx) = mpsc::channel(64);
        let desk = Arc::new(FakeDesktop::gmail());
        let (engine, handle) = TaskEngine::new(
            Arc::new(MockBrain::new(scenarios::gmail_reply(), Duration::from_millis(1))),
            desk.clone(),
            desk.clone(),
            SafetyGate::with_defaults(),
            Arc::new(Memory::in_memory().unwrap()),
            tx,
            engine::EngineConfig { confirm_timeout: Duration::from_secs(5), ..Default::default() },
        );
        let run = tokio::spawn(engine.run("reply to Sara, say I'll be there at 6".into()));
        let mut confirm_text = None;
        while let Some(ev) = rx.recv().await {
            if let CharacterEvent::ConfirmRequest { text } = &ev {
                confirm_text = Some(text.clone());
                handle.answer(true).await;
            }
            if matches!(ev, CharacterEvent::Done { .. }) {
                break;
            }
        }
        assert_eq!(confirm_text.as_deref(), Some("Send it?"));
        assert!(matches!(run.await.unwrap(), TaskOutcome::Done { .. }));
        assert_eq!(desk.performed.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn password_scenario_is_blocked_by_the_gate() {
        let (tx, mut rx) = mpsc::channel(64);
        let desk = Arc::new(FakeDesktop::gmail());
        let (engine, _handle) = TaskEngine::new(
            Arc::new(MockBrain::new(scenarios::password_field(), Duration::from_millis(1))),
            desk.clone(),
            desk.clone(),
            SafetyGate::with_defaults(),
            Arc::new(Memory::in_memory().unwrap()),
            tx,
            engine::EngineConfig::default(),
        );
        let outcome = engine.run("log me in".into()).await;
        assert!(matches!(outcome, TaskOutcome::Blocked { .. }));
        let mut said = vec![];
        while let Ok(ev) = rx.try_recv() {
            if let CharacterEvent::Say { text } = ev { said.push(text); }
        }
        assert!(said.iter().any(|s| s.contains("password")));
        assert_eq!(desk.performed.lock().unwrap().len(), 1, "the click happened, the typing did not");
    }
}
