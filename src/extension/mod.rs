//! Extension API — the contract that all extensions must satisfy.
//!
//! This module defines the [`Extension`] trait, its supporting types
//! ([`ToolDefinition`], [`SlashCommand`], [`CommandResult`], etc.),
//! the hook system, and JSON Schema coercion/validation utilities.
//!
//! Extension *implementations* live in `src/extensions/` and `src/builtin/`.

pub mod coerce;
pub mod hooks;
pub mod traits;
pub mod types;

pub use coerce::{
    ValidationError, coerce_primitive_by_type, coerce_with_json_schema, validate_tool_arguments,
};
pub use hooks::{clear_tool_hooks, register_tool_hooks, run_after_hooks, run_before_hooks};
pub use traits::{Extension, ExtensionDefault, ToolRenderer, is_extension_enabled};
pub use types::{
    AfterHook, AfterToolCallResult, AutocompleteItem, BeforeHook, BeforeToolCallResult, Cancel,
    CommandHandler, CommandResult, HookRegistration, SlashCommand, ToolDefinition,
    ToolRenderContext,
};
