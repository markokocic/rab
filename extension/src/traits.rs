//! Core traits: [`Extension`], [`ToolRenderer`], and helpers.

use rab_tui::{Component, Theme};
use std::borrow::Cow;

use crate::types::{HookRegistration, SlashCommand, ToolDefinition, ToolRenderContext};

// ── Extension default state ────────────────────────────────────

/// Default state of an extension for the /extensions UI.
/// Controls whether the extension can be toggled and its default enabled state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionDefault {
    /// Always loaded, cannot be toggled via /extensions (builtin).
    Builtin,
    /// Enabled by default, user can toggle via /extensions.
    Enabled,
    /// Disabled by default, user can toggle via /extensions.
    Disabled,
}

// ── Extension trait ────────────────────────────────────────────

pub trait Extension: Send + Sync + std::any::Any {
    fn name(&self) -> Cow<'static, str>;

    /// Downcast to `&dyn Any` for downcasting to concrete types.
    fn as_any(&self) -> &dyn std::any::Any;

    /// How this extension behaves in the /extensions UI.
    fn default_state(&self) -> ExtensionDefault {
        ExtensionDefault::Enabled
    }

    /// Tools this extension provides (LLM-callable), each with its own prompt metadata.
    fn tools(&self) -> Vec<ToolDefinition> {
        vec![]
    }

    /// Slash commands this extension provides (e.g. `/quit`, `/model`).
    fn commands(&self) -> Vec<SlashCommand> {
        vec![]
    }

    /// Skills this extension provides (AgentSkills-compatible).
    fn skills(&self) -> yoagent::skills::SkillSet {
        yoagent::skills::SkillSet::empty()
    }

    /// Called when `/reload` is triggered.
    fn on_reload(&self) {}

    /// Register hooks into a specific tool (including tools owned by other extensions).
    fn tool_hooks(&self) -> Vec<HookRegistration> {
        vec![]
    }

    /// Called before the session is shut down or reloaded.
    fn on_session_shutdown(&self, _reason: &str) {}

    /// Called after the session starts or reloads.
    fn on_session_start(&self, _reason: &str) {}
}

// ── ToolRenderer trait ─────────────────────────────────────────

/// Tool-specific rendering interface (matching pi's renderCall/renderResult pattern).
pub trait ToolRenderer: Send + Sync {
    /// Render the tool call portion as a Component.
    fn render_call(
        &self,
        args: &serde_json::Value,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Box<dyn Component>;

    /// Render the tool result body as a Component.
    fn render_result(
        &self,
        content: &str,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Option<Box<dyn Component>>;

    /// Whether this tool uses `renderShell: "self"` (controls its own framing).
    fn render_self(&self) -> bool {
        false
    }
}
