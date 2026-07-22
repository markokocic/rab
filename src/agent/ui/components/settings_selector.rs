//! SettingsSelector — the main settings menu overlay.
//!
//! Lists all configurable settings and wires changes to App state and persistence.

use crate::agent::ui::components::settings_list::{SettingItem, SettingsList};
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::util::truncate_to_width;
use crossterm::event::KeyEvent;

/// Callbacks for settings changes.
pub struct SettingsCallbacks {
    pub on_change: Box<dyn FnMut(String, String)>,
    pub on_cancel: Box<dyn FnMut()>,
}

/// The settings selector component — wraps a SettingsList with all rab settings.
pub struct SettingsSelector {
    settings_list: SettingsList,
}

impl SettingsSelector {
    /// Build the settings list items from current app state.
    #[allow(clippy::too_many_arguments)]
    pub fn build_items(
        auto_compact: bool,
        hide_thinking: bool,
        collapse_tool_output: bool,
        thinking_level: &Option<String>,
        theme_name: &str,
        available_themes: &[String],
        default_provider: &Option<String>,
        default_model: &str,
        transport: &Option<String>,
        steering_mode: &Option<String>,
        follow_up_mode: &Option<String>,
        quiet_startup: &Option<bool>,
        collapse_changelog: &Option<bool>,
        enable_skill_commands: &Option<bool>,
        enable_install_telemetry: &Option<bool>,
        double_escape_action: &Option<String>,
        tree_filter_mode: &Option<String>,
        show_hardware_cursor: &Option<bool>,
        editor_padding_x: &Option<i32>,
        output_pad: &Option<i32>,
        autocomplete_max_visible: &Option<i32>,
        verbose: bool,
        default_project_trust: &Option<String>,
        // New parameters for missing settings
        http_idle_timeout_ms: &Option<u64>,
        clear_on_shrink: &Option<bool>,
        show_terminal_progress: &Option<bool>,
        anthropic_extra_usage: &Option<bool>,
        shell_command_prefix: &Option<String>,
        shell_path: &Option<String>,
        external_editor: &Option<String>,
        http_proxy: &Option<String>,
        session_dir: &Option<String>,
    ) -> Vec<SettingItem> {
        let mut items: Vec<SettingItem> = Vec::new();

        // ── General ───────────────────────────────────────────────
        items.push(SettingItem {
            id: "autocompact".into(),
            label: "Auto-compact".into(),
            description: Some("Automatically compact context when it gets too large".into()),
            current_value: if auto_compact { "true" } else { "false" }.into(),
            values: Some(vec!["true".into(), "false".into()]),
        });

        items.push(SettingItem {
            id: "hide-thinking".into(),
            label: "Hide thinking".into(),
            description: Some("Hide thinking blocks in assistant responses".into()),
            current_value: bool_str(hide_thinking),
            values: Some(vec!["true".into(), "false".into()]),
        });

        items.push(SettingItem {
            id: "collapse-tool-output".into(),
            label: "Collapse tool output".into(),
            description: Some("Collapse tool output by default".into()),
            current_value: bool_str(collapse_tool_output),
            values: Some(vec!["true".into(), "false".into()]),
        });

        // ── Model / Provider ──────────────────────────────────────
        let provider_display = default_provider
            .as_deref()
            .unwrap_or("(not set)")
            .to_string();
        items.push(SettingItem {
            id: "default-provider".into(),
            label: "Default provider".into(),
            description: Some("Default LLM provider".into()),
            current_value: provider_display,
            values: None,
        });

        items.push(SettingItem {
            id: "default-model".into(),
            label: "Default model".into(),
            description: Some("Default LLM model".into()),
            current_value: default_model.to_string(),
            values: None,
        });

        // ── Thinking ──────────────────────────────────────────────
        let current_thinking = thinking_level.as_deref().unwrap_or("off").to_string();
        items.push(SettingItem {
            id: "thinking-level".into(),
            label: "Thinking level".into(),
            description: Some("Reasoning depth for thinking-capable models".into()),
            current_value: current_thinking,
            values: Some(vec![
                "off".into(),
                "minimal".into(),
                "low".into(),
                "medium".into(),
                "high".into(),
                "max".into(),
            ]),
        });

        // ── Theme ─────────────────────────────────────────────────
        items.push(SettingItem {
            id: "theme".into(),
            label: "Theme".into(),
            description: Some("Color theme for the interface".into()),
            current_value: theme_name.to_string(),
            values: if available_themes.is_empty() {
                Some(vec!["dark".into(), "light".into()])
            } else {
                Some(available_themes.to_vec())
            },
        });

        // ── Transport ─────────────────────────────────────────────
        let current_transport = transport.as_deref().unwrap_or("auto").to_string();
        items.push(SettingItem {
            id: "transport".into(),
            label: "Transport".into(),
            description: Some(
                "Preferred transport for providers that support multiple transports".into(),
            ),
            current_value: current_transport,
            values: Some(vec![
                "sse".into(),
                "websocket".into(),
                "websocket-cached".into(),
                "auto".into(),
            ]),
        });

        // ── Steering / Follow-up ──────────────────────────────────
        let current_steer = steering_mode.as_deref().unwrap_or("all").to_string();
        items.push(SettingItem {
            id: "steering-mode".into(),
            label: "Steering mode".into(),
            description: Some("How steering messages are delivered during streaming".into()),
            current_value: current_steer,
            values: Some(vec!["all".into(), "one-at-a-time".into()]),
        });

        let current_follow = follow_up_mode.as_deref().unwrap_or("all").to_string();
        items.push(SettingItem {
            id: "follow-up-mode".into(),
            label: "Follow-up mode".into(),
            description: Some("How follow-up messages are delivered during streaming".into()),
            current_value: current_follow,
            values: Some(vec!["all".into(), "one-at-a-time".into()]),
        });

        // ── UI / Display ──────────────────────────────────────────
        items.push(SettingItem {
            id: "quiet-startup".into(),
            label: "Quiet startup".into(),
            description: Some("Disable verbose printing at startup".into()),
            current_value: opt_bool_str(quiet_startup),
            values: Some(vec!["true".into(), "false".into()]),
        });

        items.push(SettingItem {
            id: "collapse-changelog".into(),
            label: "Collapse changelog".into(),
            description: Some("Show condensed changelog after updates".into()),
            current_value: opt_bool_str(collapse_changelog),
            values: Some(vec!["true".into(), "false".into()]),
        });

        items.push(SettingItem {
            id: "verbose".into(),
            label: "Verbose".into(),
            description: Some("Enable verbose logging output".into()),
            current_value: bool_str(verbose),
            values: Some(vec!["true".into(), "false".into()]),
        });

        // ── Input / Editor ────────────────────────────────────────
        let current_dea = double_escape_action
            .as_deref()
            .unwrap_or("tree")
            .to_string();
        items.push(SettingItem {
            id: "double-escape-action".into(),
            label: "Double-escape action".into(),
            description: Some("Action when pressing Escape twice with empty editor".into()),
            current_value: current_dea,
            values: Some(vec!["tree".into(), "fork".into(), "none".into()]),
        });

        let current_tfm = tree_filter_mode.as_deref().unwrap_or("default").to_string();
        items.push(SettingItem {
            id: "tree-filter-mode".into(),
            label: "Tree filter mode".into(),
            description: Some("Default filter when opening /tree".into()),
            current_value: current_tfm,
            values: Some(vec![
                "default".into(),
                "no-tools".into(),
                "user-only".into(),
                "labeled-only".into(),
                "all".into(),
            ]),
        });

        let current_hc = opt_bool_str(show_hardware_cursor);
        items.push(SettingItem {
            id: "show-hardware-cursor".into(),
            label: "Show hardware cursor".into(),
            description: Some("Show the terminal cursor for IME support".into()),
            current_value: current_hc,
            values: Some(vec!["true".into(), "false".into()]),
        });

        let current_ep = editor_padding_x
            .map(|v| v.to_string())
            .unwrap_or_else(|| "1".into());
        items.push(SettingItem {
            id: "editor-padding".into(),
            label: "Editor padding".into(),
            description: Some("Horizontal padding for input editor (0-3)".into()),
            current_value: current_ep,
            values: Some(vec!["0".into(), "1".into(), "2".into(), "3".into()]),
        });

        let current_op = output_pad
            .map(|v| v.to_string())
            .unwrap_or_else(|| "0".into());
        items.push(SettingItem {
            id: "output-padding".into(),
            label: "Output padding".into(),
            description: Some("Horizontal padding for messages".into()),
            current_value: current_op,
            values: Some(vec!["0".into(), "1".into()]),
        });

        let current_amv = autocomplete_max_visible
            .map(|v| v.to_string())
            .unwrap_or_else(|| "7".into());
        items.push(SettingItem {
            id: "autocomplete-max-visible".into(),
            label: "Autocomplete max items".into(),
            description: Some("Max visible items in autocomplete dropdown (3-20)".into()),
            current_value: current_amv,
            values: Some(vec![
                "3".into(),
                "5".into(),
                "7".into(),
                "10".into(),
                "15".into(),
                "20".into(),
            ]),
        });

        // ── Features ──────────────────────────────────────────────
        let current_sk = opt_bool_str(enable_skill_commands);
        items.push(SettingItem {
            id: "skill-commands".into(),
            label: "Skill commands".into(),
            description: Some("Register skills as /skill:name commands".into()),
            current_value: current_sk,
            values: Some(vec!["true".into(), "false".into()]),
        });

        let current_tele = opt_bool_str(enable_install_telemetry);
        items.push(SettingItem {
            id: "install-telemetry".into(),
            label: "Install telemetry".into(),
            description: Some("Send anonymous version/update ping after updates".into()),
            current_value: current_tele,
            values: Some(vec!["true".into(), "false".into()]),
        });

        // ── Project trust ─────────────────────────────────────────
        let current_dpt = default_project_trust
            .as_deref()
            .unwrap_or("ask")
            .to_string();
        items.push(SettingItem {
            id: "default-project-trust".into(),
            label: "Default project trust".into(),
            description: Some("Fallback behavior for project trust decisions".into()),
            current_value: current_dpt,
            values: Some(vec!["ask".into(), "always".into(), "never".into()]),
        });

        // ── Network ───────────────────────────────────────────────
        let http_timeout = http_idle_timeout_ms.map_or_else(
            || "(n/a)".to_string(),
            |ms| match ms {
                0 => "disabled".into(),
                30_000 => "30 sec".into(),
                60_000 => "1 min".into(),
                120_000 => "2 min".into(),
                300_000 => "5 min".into(),
                _ => format!("{}ms", ms),
            },
        );
        items.push(SettingItem {
            id: "http-idle-timeout".into(),
            label: "HTTP idle timeout".into(),
            description: Some(
                "Maximum idle gap while waiting for HTTP headers or body chunks".into(),
            ),
            current_value: http_timeout,
            values: Some(vec![
                "30 sec".into(),
                "1 min".into(),
                "2 min".into(),
                "5 min".into(),
                "disabled".into(),
            ]),
        });

        // ── Terminal ──────────────────────────────────────────────
        items.push(SettingItem {
            id: "clear-on-shrink".into(),
            label: "Clear on shrink".into(),
            description: Some("Clear empty rows when content shrinks (may cause flicker)".into()),
            current_value: opt_bool_str(clear_on_shrink),
            values: Some(vec!["true".into(), "false".into()]),
        });

        items.push(SettingItem {
            id: "terminal-progress".into(),
            label: "Terminal progress".into(),
            description: Some("Show OSC 9;4 progress indicators in the terminal tab bar".into()),
            current_value: opt_bool_str(show_terminal_progress),
            values: Some(vec!["true".into(), "false".into()]),
        });

        items.push(SettingItem {
            id: "show-images".into(),
            label: "Show images (n/a)".into(),
            description: Some("Render images inline in terminal".into()),
            current_value: "(n/a)".into(),
            values: None,
        });

        // ── Warnings ──────────────────────────────────────────────
        let current_warn = match anthropic_extra_usage {
            Some(true) => "true",
            Some(false) => "false",
            None => "(n/a)",
        };
        items.push(SettingItem {
            id: "warnings-anthropic-extra-usage".into(),
            label: "Warn: Anthropic extra usage".into(),
            description: Some(
                "Warn when Anthropic subscription auth may use paid extra usage".into(),
            ),
            current_value: current_warn.into(),
            values: Some(vec!["true".into(), "false".into()]),
        });

        // ── Shell / Editor (display-only, n/a) ────────────────────
        items.push(SettingItem {
            id: "shell-command-prefix".into(),
            label: "Shell command prefix".into(),
            description: Some("Prefix prepended to every bash command (n/a in menu)".into()),
            current_value: shell_command_prefix
                .as_deref()
                .map(|s| format!("\"{}\"", s))
                .unwrap_or_else(|| "(n/a)".into()),
            values: None,
        });

        items.push(SettingItem {
            id: "shell-path".into(),
            label: "Shell path".into(),
            description: Some("Path to the shell executable (n/a in menu)".into()),
            current_value: shell_path.as_deref().unwrap_or("(n/a)").to_string(),
            values: None,
        });

        items.push(SettingItem {
            id: "external-editor".into(),
            label: "External editor".into(),
            description: Some("External editor command (n/a in menu)".into()),
            current_value: external_editor.as_deref().unwrap_or("(n/a)").to_string(),
            values: None,
        });

        items.push(SettingItem {
            id: "http-proxy".into(),
            label: "HTTP proxy".into(),
            description: Some("HTTP proxy URL (n/a in menu)".into()),
            current_value: http_proxy.as_deref().unwrap_or("(n/a)").to_string(),
            values: None,
        });

        items.push(SettingItem {
            id: "session-dir".into(),
            label: "Session directory".into(),
            description: Some("Custom session storage directory (n/a in menu)".into()),
            current_value: session_dir.as_deref().unwrap_or("(n/a)").to_string(),
            values: None,
        });

        items
    }

    /// Create a new SettingsSelector with the given items and callbacks.
    pub fn new(items: Vec<SettingItem>, callbacks: SettingsCallbacks) -> Self {
        let count = items.len();
        let settings_list = SettingsList::new(
            items,
            count.min(12),
            callbacks.on_change,
            callbacks.on_cancel,
            true,
        );

        Self { settings_list }
    }
}

impl Component for SettingsSelector {
    fn render(&mut self, width: usize) -> Vec<String> {
        // Scope theme guard so it's dropped before settings_list.render()
        // which also calls current_theme(). Otherwise the non-reentrant mutex
        // deadlocks.
        let mut lines: Vec<String> = Vec::new();
        {
            let theme = current_theme();
            let title = theme.bold_accent("  Settings");
            lines.push(truncate_to_width(&title, width, "", true));
            lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));
        }
        lines.push(String::new());

        // Settings list
        lines.extend(self.settings_list.render(width));

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        self.settings_list.handle_input(key)
    }

    fn invalidate(&mut self) {
        self.settings_list.invalidate();
    }
}

// ── Helpers ─────────────────────────────────────────────────────

fn bool_str(v: bool) -> String {
    if v { "true" } else { "false" }.into()
}

fn opt_bool_str(v: &Option<bool>) -> String {
    match v {
        Some(true) => "true".into(),
        Some(false) => "false".into(),
        None => "(n/a)".into(),
    }
}
