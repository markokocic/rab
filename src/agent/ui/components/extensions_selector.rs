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
    pub command_count: usize,
    pub skill_names: Vec<String>,
}

/// Callbacks for the ExtensionsSelector.
pub struct ExtensionsCallbacks {
    pub on_toggle: Box<dyn FnMut(String, bool)>,
    pub on_save_global: Box<dyn FnMut()>,
    pub on_save_project: Box<dyn FnMut()>,
    pub on_cancel: Box<dyn FnMut()>,
}

/// Build SettingItems from extension info.
fn build_items(extensions: &[ExtensionInfo]) -> Vec<SettingItem> {
    let mut items: Vec<SettingItem> = Vec::new();

    for ext in extensions {
        let is_builtin = ext.default_state == ExtensionDefault::Builtin;

        // Extension row
        let (current_value, values) = if is_builtin {
            ("(builtin)".to_string(), None)
        } else {
            let state = if ext.enabled { "enabled" } else { "disabled" };
            (
                state.to_string(),
                Some(vec!["enabled".to_string(), "disabled".to_string()]),
            )
        };
        items.push(SettingItem {
            id: format!("ext_{}", ext.name),
            label: ext.name.clone(),
            description: None,
            current_value,
            values,
        });

        // Tools detail line
        if !ext.tool_names.is_empty() {
            let tools_str = format!("    Tools: {}", ext.tool_names.join(", "));
            items.push(SettingItem {
                id: format!("{}_tools", ext.name),
                label: tools_str,
                description: None,
                current_value: String::new(),
                values: None,
            });
        }

        // Commands detail line
        if ext.command_count > 0 {
            let label = if ext.command_count == 1 {
                "    Commands: 1 command".to_string()
            } else {
                format!("    Commands: {} commands", ext.command_count)
            };
            items.push(SettingItem {
                id: format!("{}_cmds", ext.name),
                label,
                description: None,
                current_value: String::new(),
                values: None,
            });
        }

        // Skills detail line
        if !ext.skill_names.is_empty() {
            let skills_str = format!("    Skills: {}", ext.skill_names.join(", "));
            items.push(SettingItem {
                id: format!("{}_skills", ext.name),
                label: skills_str,
                description: None,
                current_value: String::new(),
                values: None,
            });
        }
    }

    // Separator + save actions
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
        let theme = current_theme();
        let mut lines: Vec<String> = Vec::new();

        let title = theme.bold_accent("  Extension Configuration");
        lines.push(truncate_to_width(&title, width, "", true));
        lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));
        lines.push(String::new());

        // Render the settings list contents
        let list_lines = self.settings_list.render(width);

        // Filter out the default hint line from SettingsList and replace with our own
        // (The settings list renders hints at the bottom; we want our own hint line)
        let our_hint = format!(
            "  {}",
            theme.dim("Type to search · ↑↓ navigate · Enter/Space: toggle · Esc: close")
        );

        // Replace the last non-empty line (the hint) with our custom hint
        let mut result_lines: Vec<String> = Vec::new();
        for line in &list_lines {
            // Skip the default hint line
            if line.contains("Type to search") || line.contains("Enter/Space to change") {
                continue;
            }
            result_lines.push(line.clone());
        }
        result_lines.push(String::new());
        result_lines.push(truncate_to_width(&our_hint, width, "", true));

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
