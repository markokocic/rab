//! ExtensionsSelector — the /extensions command overlay.
//!
//! Shows all extensions with their tools, commands, and skills listed inline.
//! Builtin extensions are display-only. Other extensions can be toggled
//! enabled/disabled. Save actions persist to global or project settings.

use crate::agent::ExtensionDefault;
use crate::agent::ui::components::settings_list::{SettingItem, SettingsList};
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::util::truncate_to_width;
use crossterm::event::KeyEvent;

/// Info about a single extension for the /extensions UI.
#[derive(Debug, Clone)]
pub struct ExtensionInfo {
    pub name: String,
    pub default_state: ExtensionDefault,
    pub enabled: bool,
    pub tool_names: Vec<String>,
    pub command_names: Vec<String>,
    pub skill_names: Vec<String>,
}

/// Callbacks for the ExtensionsSelector.
pub struct ExtensionsCallbacks {
    pub on_toggle: Box<dyn FnMut(String, bool)>,
    pub on_save_global: Box<dyn FnMut()>,
    pub on_save_project: Box<dyn FnMut()>,
    pub on_cancel: Box<dyn FnMut()>,
}

/// Build SettingItems from extension info (extension rows + save actions only).
fn build_items(extensions: &[ExtensionInfo]) -> Vec<SettingItem> {
    let mut items: Vec<SettingItem> = Vec::new();

    for ext in extensions {
        let is_builtin = ext.default_state == ExtensionDefault::Builtin;

        let (current_value, values) = if is_builtin {
            ("(builtin)".to_string(), None)
        } else {
            let state = if ext.enabled { "enabled" } else { "disabled" };
            (
                state.to_string(),
                Some(vec!["enabled".to_string(), "disabled".to_string()]),
            )
        };

        // Build description with tools, commands, skills (shown when extension is selected)
        let mut desc_parts: Vec<String> = Vec::new();
        if is_builtin {
            desc_parts.push("Always loaded, cannot be disabled".to_string());
        }
        if !ext.tool_names.is_empty() {
            desc_parts.push(format!("Tools: {}", ext.tool_names.join(", ")));
        }
        if !ext.command_names.is_empty() {
            desc_parts.push(format!("Commands: {}", ext.command_names.join(", ")));
        }
        if !ext.skill_names.is_empty() {
            desc_parts.push(format!("Skills: {}", ext.skill_names.join(", ")));
        }
        let description = if desc_parts.is_empty() {
            None
        } else {
            Some(desc_parts.join("\n"))
        };

        items.push(SettingItem {
            id: format!("ext_{}", ext.name),
            label: ext.name.clone(),
            description,
            current_value,
            values,
        });
    }

    // Save actions
    items.push(SettingItem {
        id: "__separator__".into(),
        label: "".into(),
        description: None,
        current_value: String::new(),
        values: None,
    });

    items.push(SettingItem {
        id: "__save_global__".into(),
        label: "Save globally".into(),
        description: Some("Save extension states to ~/.rab/agent/settings.json".into()),
        current_value: "".into(),
        values: Some(vec!["save".into()]),
    });

    items.push(SettingItem {
        id: "__save_project__".into(),
        label: "Save in project".into(),
        description: Some("Save extension states to .rab/settings.json".into()),
        current_value: "".into(),
        values: Some(vec!["save".into()]),
    });

    items
}

/// The /extensions selector component.
pub struct ExtensionsSelector {
    settings_list: SettingsList,
    _extensions: Vec<ExtensionInfo>,
}

impl ExtensionsSelector {
    pub fn new(extensions: Vec<ExtensionInfo>, callbacks: ExtensionsCallbacks) -> Self {
        let items = build_items(&extensions);
        let count = items.len();

        // Destructure callbacks to avoid partial move
        let ExtensionsCallbacks {
            mut on_toggle,
            mut on_save_global,
            mut on_save_project,
            mut on_cancel,
        } = callbacks;

        let settings_list = SettingsList::new(
            items,
            count.min(15),
            {
                Box::new(move |id, new_value| {
                    if id == "__save_global__" {
                        (on_save_global)();
                    } else if id == "__save_project__" {
                        (on_save_project)();
                    } else if let Some(ext_name) = id.strip_prefix("ext_") {
                        let enabled = new_value == "enabled";
                        (on_toggle)(ext_name.to_string(), enabled);
                    }
                })
            },
            {
                Box::new(move || {
                    (on_cancel)();
                })
            },
            true, // search enabled
        );

        Self {
            settings_list,
            _extensions: extensions,
        }
    }

    /// Update the enabled state of an extension in the settings list.
    pub fn update_extension_state(&mut self, name: &str, enabled: bool) {
        let id = format!("ext_{}", name);
        let value = if enabled { "enabled" } else { "disabled" };
        self.settings_list.update_value(&id, value.to_string());
    }
}

impl Component for ExtensionsSelector {
    fn render(&mut self, width: usize) -> Vec<String> {
        // Scope the theme guard so it's dropped before settings_list.render()
        // (which also calls current_theme()). Otherwise the non-reentrant Mutex deadlocks.
        let mut lines: Vec<String> = Vec::new();
        let hint_text: String;
        {
            let theme = current_theme();
            let title = theme.bold_accent("  Extension Configuration");
            lines.push(truncate_to_width(&title, width, "", true));
            lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));
            lines.push(String::new());
            hint_text = format!(
                "  {}",
                theme.dim("Type to search · ↑↓ navigate · Enter/Space: toggle · Esc: close")
            );
        }
        // theme lock is released here

        // Render the settings list contents (descriptions shown when selected)
        let list_lines = self.settings_list.render(width);

        // Filter out the default hint line from SettingsList and replace with our own
        let mut result_lines: Vec<String> = Vec::new();
        for line in &list_lines {
            if line.contains("Type to search") || line.contains("Enter/Space to change") {
                continue;
            }
            result_lines.push(line.clone());
        }
        result_lines.push(String::new());
        result_lines.push(truncate_to_width(&hint_text, width, "", true));

        // Pad to minimum height so overlay doesn't flicker as description length changes
        const MIN_OVERLAY_HEIGHT: usize = 20;
        while result_lines.len() < MIN_OVERLAY_HEIGHT {
            result_lines.push(String::new());
        }

        lines.extend(result_lines);
        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        self.settings_list.handle_input(key)
    }

    fn invalidate(&mut self) {
        self.settings_list.invalidate();
    }
}
