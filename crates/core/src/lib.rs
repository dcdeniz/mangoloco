//! Mango Loco editing engine.
//!
//! UI-independent: everything here can be driven from tests, the command-line
//! tool, the assistant, or the desktop app through [`editor::Editor`].

pub mod captions;
pub mod detect;
pub mod editor;
pub mod model;
pub mod render;
pub mod script;
pub mod timeline_export;
pub mod tools;
pub mod transcript_export;

pub use editor::{Command, Editor};
pub use model::*;
