//! Core traits: [`Extension`], [`ToolRenderer`], and helpers.

use crate::tui::{Component, Theme};
use std::borrow::Cow;

use crate::extension::types::{HookRegistration, SlashCommand, ToolDefinition, ToolRenderContext};

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

/// Check whether an extension is currently enabled.
/// Builtin extensions are always enabled. Otherwise, check the settings overrides,
/// falling back to the extension's `default_state()`.
pub fn is_extension_enabled(ext: &dyn Extension, settings: &crate::settings::Settings) -> bool {
    match ext.default_state() {
        ExtensionDefault::Builtin => true,
        _ => {
            if let Some(&enabled) = settings.extensions_config.states.get(ext.name().as_ref()) {
                enabled
            } else {
                ext.default_state() == ExtensionDefault::Enabled
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    struct MockExt {
        name: &'static str,
        state: ExtensionDefault,
    }

    impl Extension for MockExt {
        fn name(&self) -> Cow<'static, str> {
            Cow::Borrowed(self.name)
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn default_state(&self) -> ExtensionDefault {
            self.state
        }
    }

    fn settings_with_ext(name: &str, enabled: bool) -> crate::settings::Settings {
        let mut s = crate::settings::Settings::default();
        s.extensions_config.states.insert(name.to_string(), enabled);
        s
    }

    #[test]
    fn builtin_always_enabled() {
        let ext = MockExt {
            name: "builtin",
            state: ExtensionDefault::Builtin,
        };
        let s = crate::settings::Settings::default();
        assert!(is_extension_enabled(&ext, &s));
    }

    #[test]
    fn enabled_by_default_when_not_in_settings() {
        let ext = MockExt {
            name: "my-ext",
            state: ExtensionDefault::Enabled,
        };
        let s = crate::settings::Settings::default();
        assert!(is_extension_enabled(&ext, &s));
    }

    #[test]
    fn disabled_by_default_when_not_in_settings() {
        let ext = MockExt {
            name: "my-ext",
            state: ExtensionDefault::Disabled,
        };
        let s = crate::settings::Settings::default();
        assert!(!is_extension_enabled(&ext, &s));
    }

    #[test]
    fn settings_overrides_enabled() {
        let ext = MockExt {
            name: "my-ext",
            state: ExtensionDefault::Disabled, // disabled by default
        };
        let s = settings_with_ext("my-ext", true); // but enabled in settings
        assert!(is_extension_enabled(&ext, &s));
    }

    #[test]
    fn settings_overrides_disabled() {
        let ext = MockExt {
            name: "my-ext",
            state: ExtensionDefault::Enabled, // enabled by default
        };
        let s = settings_with_ext("my-ext", false); // but disabled in settings
        assert!(!is_extension_enabled(&ext, &s));
    }

    #[test]
    fn settings_for_different_ext_does_not_affect() {
        let ext = MockExt {
            name: "my-ext",
            state: ExtensionDefault::Enabled,
        };
        let s = settings_with_ext("other-ext", false);
        assert!(is_extension_enabled(&ext, &s));
    }
}
