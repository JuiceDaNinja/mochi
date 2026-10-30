//! Windows perception and actuation for Mochi.
//!
//! `WindowsDesktop` implements [`Perception`] (UI Automation tree + GDI screenshot when the
//! tree is thin) and [`Actuator`] (SendInput). Each call runs on a blocking thread with its
//! own COM initialisation, so the async engine never touches COM directly.
//!
//! On non-Windows hosts the same type exists but every call returns an error, which keeps
//! the desktop app compiling everywhere while only Windows is wired for real.

pub mod common;
#[cfg(windows)]
mod win;

use async_trait::async_trait;
use mochi_core::*;

#[derive(Default)]
pub struct WindowsDesktop;

impl WindowsDesktop {
    pub fn new() -> Self {
        Self
    }
    pub fn available() -> bool {
        cfg!(windows)
    }
}

/// Is the left mouse button currently down? Polled by the shell to detect a teaching click
/// while the character window is click-through.
#[cfg(windows)]
pub fn left_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    (unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000) != 0
}
#[cfg(not(windows))]
pub fn left_button_down() -> bool {
    false
}

#[cfg(windows)]
#[async_trait]
impl Perception for WindowsDesktop {
    async fn snapshot(&self, want_screenshot: bool) -> Result<ScreenSnapshot> {
        tokio::task::spawn_blocking(move || win::snapshot(want_screenshot)).await.map_err(|e| MochiError::Perception(e.to_string()))?
    }
    async fn element_under_cursor(&self) -> Result<Option<(ScreenSnapshot, u32)>> {
        tokio::task::spawn_blocking(win::element_under_cursor).await.map_err(|e| MochiError::Perception(e.to_string()))?
    }
}

#[cfg(windows)]
#[async_trait]
impl Actuator for WindowsDesktop {
    async fn perform(&self, action: &Action, screen: &ScreenSnapshot) -> Result<()> {
        let (a, s) = (action.clone(), screen.clone());
        tokio::task::spawn_blocking(move || win::perform(&a, &s)).await.map_err(|e| MochiError::Action(e.to_string()))?
    }
}

#[cfg(not(windows))]
#[async_trait]
impl Perception for WindowsDesktop {
    async fn snapshot(&self, _want_screenshot: bool) -> Result<ScreenSnapshot> {
        Err(MochiError::Perception("screen reading is only implemented on Windows in this build".into()))
    }
    async fn element_under_cursor(&self) -> Result<Option<(ScreenSnapshot, u32)>> {
        Ok(None)
    }
}

#[cfg(not(windows))]
#[async_trait]
impl Actuator for WindowsDesktop {
    async fn perform(&self, _action: &Action, _screen: &ScreenSnapshot) -> Result<()> {
        Err(MochiError::Action("input is only implemented on Windows in this build".into()))
    }
}
