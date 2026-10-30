//! Mochi core: everything that does not touch a screen, a speaker or a network card.
//!
//! The desktop app (Tauri) wires platform implementations of [`Perception`] and
//! [`Actuator`] into the [`TaskEngine`], picks a [`Brain`] adapter, and forwards
//! [`CharacterEvent`]s to the character window. Nothing in this crate knows which
//! brain is running, which is the point: Mochi behaves the same with all of them.

pub mod brain;
pub mod engine;
pub mod gate;
pub mod memory;
pub mod perception;
pub mod prompt;
pub mod protocol;
pub mod runtime;
pub mod types;

pub use brain::{Brain, BrainCapabilities, BrainInput};
pub use engine::{ConfirmAnswer, EngineHandle, TaskEngine, TaskOutcome};
pub use gate::{GateRules, GateVerdict, SafetyGate};
pub use memory::{LearnedTarget, Memory};
pub use perception::{Actuator, Perception};
pub use runtime::{CharacterEvent, CharacterState, DoneKind};
pub use types::*;
