//! Interactive TUI application — main struct, event loop, and module hub.
//!
//! This module is the central hub for the interactive agent UI. It re-exports
//! all sub-modules so that external users (help.rs, agent/ui/mod.rs) continue
//! to find items under `crate::agent::ui::app::*`.
//!
//! Sub-modules:
//! - [`events`]    — Agent event handler (`handle_agent_event`)
//! - [`helpers`]   — Utility/helper functions (bang parsing, XML, skills)
//! - [`types`]     — Shared types (`OverlayResult`, `PendingLabelChanges`)
//! - [`handlers`]  — Keyboard input handling and event dispatch
//! - [`chat`]      — Chat rendering utilities
//! - [`command_handlers`] — Slash command dispatch and result handling
//! - [`auth`]      — Authentication dialogs and login/logout flows
//! - [`agent`]     — Agent lifecycle (submission, creation, compaction)
//! - [`overlays`]  — Overlay openers (model selector, settings, etc.)
//! - [`app_impl`]  — `App` struct, `AppConfig`, `impl App`, `run()`

pub mod agent;
pub mod auth;
pub mod chat;
pub mod command_handlers;
pub mod events;
pub mod handlers;
pub mod helpers;
pub mod overlays;
pub mod types;

// Re-export everything at the module level for backward compatibility.
// External code can continue to use `crate::agent::ui::app::*`.
pub use agent::*;
pub use auth::*;
pub use chat::*;
pub use command_handlers::*;
pub use events::*;
pub use handlers::*;
pub use helpers::*;
pub use overlays::*;
pub use types::*;

// App struct, AppConfig, and run() live in their own file.
mod app_impl;
pub use app_impl::{App, AppConfig, run};
