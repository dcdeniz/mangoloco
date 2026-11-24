//! Mango Loco AI: everything that talks to Claude.
//!
//! * [`client`]: blocking Messages API client (streaming, retries, typed errors).
//! * [`context`]: compact text rendering of a project/script for the model.
//! * [`assistant`]: the chat co-editor, which edits the project by calling tools.
//! * [`tools`]: one-shot AI tools that return proposals the user can accept.
//!
//! Nothing here trusts model output: every index, range and command that comes
//! back from the model is validated against the current project before it is
//! applied, and the API key is never logged or included in errors.

mod agent_tools;
pub mod assistant;
pub mod client;
pub mod context;
pub mod tools;

pub use assistant::{Assistant, ChatMessage, Event, Role, TurnResult};
pub use client::{AiError, Client, DEFAULT_MODEL, Effort, FAST_MODEL, Usage};
