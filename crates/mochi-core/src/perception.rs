//! Platform ports. The desktop app implements these with Windows UI Automation +
//! SendInput (phase 2); the mock implementation in `mochi-brain-mock` drives phase 1.

use crate::types::{Action, Result, ScreenSnapshot};
use async_trait::async_trait;

#[async_trait]
pub trait Perception: Send + Sync {
    /// Snapshot the active window: trimmed tree, monitors, and a screenshot only if
    /// `want_screenshot` or the tree is too thin to act on.
    async fn snapshot(&self, want_screenshot: bool) -> Result<ScreenSnapshot>;

    /// Element id currently under the cursor, if any (used by the teaching loop).
    async fn element_under_cursor(&self) -> Result<Option<(ScreenSnapshot, u32)>>;
}

#[async_trait]
pub trait Actuator: Send + Sync {
    async fn perform(&self, action: &Action, screen: &ScreenSnapshot) -> Result<()>;
}
