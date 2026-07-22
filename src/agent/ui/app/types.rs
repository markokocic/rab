//! Shared types for the interactive UI application.
//!
//! Extracted from `mod.rs` to reduce file size.

use std::cell::RefCell;
use std::rc::Rc;

use crate::agent::ui::components::oauth_selector::AuthType;

/// Pending label changes accumulator (used by tree selector, flushed each frame).
pub type PendingLabelChanges = Rc<RefCell<Vec<(String, Option<String>)>>>;

/// Result from an overlay lifecycle — checked by the main loop after route_input.
#[derive(Debug, Clone)]
pub enum OverlayResult {
    /// User selected a model (provider/id string).
    ModelSelected(String),
    /// User accepted scoped model changes — persist to settings.
    ScopedModelsAccepted(Option<Vec<String>>),
    /// User cancelled — close overlay, no persist.
    ScopedModelsCancelled,
    /// User selected a provider for login.
    LoginProviderSelected(String),
    /// User provided an API key for login.
    LoginApiKeyProvided { provider: String, key: String },
    /// User selected an auth type for login.
    LoginAuthTypeSelected(AuthType),
    /// User selected a provider for logout.
    LogoutProviderSelected(String),
    /// User confirmed session import (carries the resolved path).
    ImportConfirmed(String),
    /// User cancelled session import.
    ImportCancelled,
    /// User selected a tree entry to navigate to.
    TreeNavigateTo(String),
    /// User cancelled tree navigation.
    TreeCancelled,
    /// User chose whether to summarize after tree entry selection.
    /// `custom_instructions` is set when user chose "Summarize with custom prompt".
    TreeSummarizeChoice {
        entry_id: String,
        summarize: bool,
        custom_instructions: Option<String>,
    },
    /// User wants to reopen the tree selector (from summarization prompt), carrying the entry to select.
    TreeReopen(String),
    /// User selected a message to fork from.
    ForkMessageSelected(String),
    /// User cancelled fork message selection.
    ForkCancelled,
    /// Generic dismiss (no action needed, close the overlay).
    Dismiss,
}
