pub mod coerce;
pub mod hooks;
pub mod traits;
pub mod types;

pub use coerce::{
    ValidationError, coerce_primitive_by_type, coerce_with_json_schema, validate_tool_arguments,
};
pub use hooks::{clear_tool_hooks, register_tool_hooks, run_after_hooks, run_before_hooks};
pub use traits::{Extension, ExtensionDefault, ToolRenderer};
pub use types::{
    AfterHook, AfterToolCallResult, AutocompleteItem, BeforeHook, BeforeToolCallResult, Cancel,
    CommandHandler, CommandResult, HookRegistration, SlashCommand, ToolDefinition,
    ToolRenderContext,
};

use crate::settings::Settings;

/// Check whether an extension is currently enabled.
/// Builtin extensions are always enabled. Otherwise, check the settings overrides,
/// falling back to the extension's `default_state()`.
pub fn is_extension_enabled(ext: &dyn Extension, settings: &Settings) -> bool {
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

    fn settings_with_ext(name: &str, enabled: bool) -> Settings {
        let mut s = Settings::default();
        s.extensions_config.states.insert(name.to_string(), enabled);
        s
    }

    #[test]
    fn builtin_always_enabled() {
        let ext = MockExt {
            name: "builtin",
            state: ExtensionDefault::Builtin,
        };
        let s = Settings::default();
        assert!(is_extension_enabled(&ext, &s));
    }

    #[test]
    fn enabled_by_default_when_not_in_settings() {
        let ext = MockExt {
            name: "my-ext",
            state: ExtensionDefault::Enabled,
        };
        let s = Settings::default();
        assert!(is_extension_enabled(&ext, &s));
    }

    #[test]
    fn disabled_by_default_when_not_in_settings() {
        let ext = MockExt {
            name: "my-ext",
            state: ExtensionDefault::Disabled,
        };
        let s = Settings::default();
        assert!(!is_extension_enabled(&ext, &s));
    }

    #[test]
    fn settings_overrides_enabled() {
        let ext = MockExt {
            name: "my-ext",
            state: ExtensionDefault::Disabled,
        };
        let s = settings_with_ext("my-ext", true);
        assert!(is_extension_enabled(&ext, &s));
    }

    #[test]
    fn settings_overrides_disabled() {
        let ext = MockExt {
            name: "my-ext",
            state: ExtensionDefault::Enabled,
        };
        let s = settings_with_ext("my-ext", false);
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
