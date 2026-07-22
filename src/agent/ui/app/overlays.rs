//! Overlay openers — model selector, settings, extensions, scoped models, etc.
//!
//! Extracted from `mod.rs` to reduce file size.

use super::App;
use super::types::OverlayResult;
use crate::agent::ui::theme;
use crate::extension::Extension;
use crate::tui::Component;
use crate::tui::TUI;
use yoagent::types::AgentTool;

/// Open the model selector overlay.
pub fn open_model_selector(app: &mut App, tui: &mut TUI) {
    let current = app.model.clone();

    // Build (provider, model_id, name) tuples from authenticated providers only.
    // This matches pi's behavior of showing only models from configured providers.
    let all_tuples: Vec<(String, String, String)> = app.registry.list_model_provider_tuples();
    let all_models: Vec<(String, String, String)> = all_tuples
        .into_iter()
        .filter(|(provider, _, _)| app.registry.provider_has_auth(provider))
        .collect();

    let scoped_ids = app.scoped_model_ids.clone().unwrap_or_default();

    let signal = app.overlay_result_signal.clone();
    let current_provider = app
        .registry
        .provider_for_model(&current, Some(&app.current_provider))
        .unwrap_or_else(|| "unknown".to_string());
    let current_full_id = format!("{}/{}", current_provider, current);

    let callbacks = crate::agent::ui::model_selector::ModelSelectorCallbacks {
        on_select: Box::new({
            let signal = signal.clone();
            move |full_id: String| {
                *signal.borrow_mut() = Some(OverlayResult::ModelSelected(full_id));
            }
        }),
        on_cancel: Box::new(|| {}), // No-op: overlay is popped by handle_input returning false
    };

    let selector = crate::agent::ui::model_selector::ModelSelector::new(
        all_models,
        scoped_ids,
        current_full_id,
        callbacks,
    );
    tui.show_positioned_overlay(Box::new(selector), crate::tui::OverlayPosition::Bottom);
}

/// Open the settings overlay.
pub fn open_settings(app: &mut App, tui: &mut TUI) {
    use crate::agent::ui::components::settings_selector::{SettingsCallbacks, SettingsSelector};

    let available_themes = theme::get_available_themes();

    let items = SettingsSelector::build_items(
        app.auto_compact,
        app.hide_thinking,
        app.collapse_tool_output,
        &app.thinking_level,
        &app.theme.name,
        &available_themes,
        &app.settings.default_provider,
        &app.model,
        &app.settings.transport,
        &app.settings.steering_mode,
        &app.settings.follow_up_mode,
        &app.settings.quiet_startup,
        &app.settings.collapse_changelog,
        &app.settings.enable_skill_commands,
        &app.settings.enable_install_telemetry,
        &app.settings.double_escape_action,
        &app.settings.tree_filter_mode,
        &app.settings.show_hardware_cursor,
        &app.settings.editor_padding_x,
        &app.settings.output_pad,
        &app.settings.autocomplete_max_visible,
        app.settings.verbose,
        &app.settings.default_project_trust,
        &app.settings.http_idle_timeout_ms,
        &app.settings
            .terminal
            .as_ref()
            .and_then(|t| t.clear_on_shrink),
        &app.settings
            .terminal
            .as_ref()
            .and_then(|t| t.show_terminal_progress),
        &app.settings
            .warnings
            .as_ref()
            .and_then(|w| w.anthropic_extra_usage),
        &app.settings.shell_command_prefix,
        &app.settings.shell_path,
        &app.settings.external_editor,
        &app.settings.http_proxy,
        &app.settings.session_dir,
    );

    let change_signal = app.pending_settings_change.clone();
    let close_signal = app.overlay_result_signal.clone();

    let callbacks = SettingsCallbacks {
        on_change: Box::new({
            let cs = change_signal.clone();
            move |id: String, new_value: String| {
                *cs.borrow_mut() = Some((id, new_value));
            }
        }),
        on_cancel: Box::new({
            let signal = close_signal.clone();
            move || {
                *signal.borrow_mut() = Some(OverlayResult::Dismiss);
            }
        }),
    };

    let selector = SettingsSelector::new(items, callbacks);
    tui.show_positioned_overlay(Box::new(selector), crate::tui::OverlayPosition::Bottom);
}

/// Open the extension configuration overlay.
pub fn open_extensions_selector(app: &mut App, tui: &mut TUI) {
    use crate::agent::ui::components::extensions_selector::{
        ExtensionInfo, ExtensionsCallbacks, ExtensionsSelector,
    };

    // Build ExtensionInfo for each loaded extension
    let extensions: Vec<ExtensionInfo> = app
        .extensions
        .iter()
        .map(|ext| {
            let tools = ext.tools();
            let tool_names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
            let commands = ext.commands();
            let command_names: Vec<String> = commands.iter().map(|c| c.name.clone()).collect();
            let skills = ext.skills();
            let skill_names: Vec<String> = skills.skills().iter().map(|s| s.name.clone()).collect();

            let name = ext.name().to_string();
            let default_state = ext.default_state();

            // Determine current enabled state
            let enabled = crate::extension::is_extension_enabled(ext.as_ref(), &app.settings);

            ExtensionInfo {
                name,
                default_state,
                enabled,
                tool_names,
                command_names,
                skill_names,
            }
        })
        .collect();
    let close_signal = app.overlay_result_signal.clone();

    let callbacks = ExtensionsCallbacks {
        on_toggle: Box::new({
            let app_ptr: *mut App = app as *mut App;
            move |name: String, enabled: bool| {
                let app = unsafe { &mut *app_ptr };
                // Update in-memory settings
                app.settings.set_extension_enabled(&name, enabled);

                // Rebuild system prompt, commands, and skills from current extensions
                refresh_agent_config(app);
            }
        }),
        on_save_global: Box::new({
            let app_ptr: *mut App = app as *mut App;
            move || {
                let app = unsafe { &mut *app_ptr };
                if let Err(e) = app.settings.save() {
                    app.status_text = Some(format!("Failed to save extensions globally: {}", e));
                } else {
                    app.status_text = Some("Extensions saved globally".into());
                }
            }
        }),
        on_save_project: Box::new({
            let app_ptr: *mut App = app as *mut App;
            move || {
                let app = unsafe { &mut *app_ptr };
                if let Err(e) = app.settings.save_to_project(&app.cwd) {
                    app.status_text = Some(format!("Failed to save extensions in project: {}", e));
                } else {
                    app.status_text = Some("Extensions saved in project".into());
                }
            }
        }),
        on_cancel: Box::new({
            let signal = close_signal.clone();
            move || {
                *signal.borrow_mut() = Some(OverlayResult::Dismiss);
            }
        }),
    };

    let selector = ExtensionsSelector::new(extensions, callbacks);
    use crate::tui::overlay::{OverlayAnchor, OverlayOptions, SizeValue};
    tui.show_overlay(
        Box::new(selector),
        OverlayOptions {
            width: Some(SizeValue::Percent(100.0)),
            anchor: Some(OverlayAnchor::BottomLeft),
            offset_y: Some(-5),
            ..Default::default()
        },
    );
}

/// Open the scoped-models selector overlay.
pub fn open_scoped_models_selector(app: &mut App, tui: &mut TUI) {
    use crate::agent::ui::components::scoped_models_selector::{
        ModelsCallbacks, ModelsConfig, ScopedModelsSelector,
    };

    // Build (provider, model_id, name) tuples from authenticated providers only.
    let all_tuples: Vec<(String, String, String)> = app.registry.list_model_provider_tuples();
    let all_models: Vec<(String, String, String)> = all_tuples
        .into_iter()
        .filter(|(provider, _, _)| app.registry.provider_has_auth(provider))
        .collect();

    let current_enabled = app.scoped_model_ids.clone();
    let change_signal = app.pending_scoped_ids.clone();
    let close_signal = app.overlay_result_signal.clone();

    let callbacks = ModelsCallbacks {
        on_change: Box::new(move |enabled_ids: Option<Vec<String>>| {
            // Session-only update — does NOT close the overlay.
            *change_signal.borrow_mut() = Some(enabled_ids.unwrap_or_default());
        }),
        on_persist: Box::new({
            let cs = close_signal.clone();
            move |enabled_ids: Option<Vec<String>>| {
                *cs.borrow_mut() = Some(OverlayResult::ScopedModelsAccepted(enabled_ids));
            }
        }),
        on_cancel: Box::new(move || {
            *close_signal.borrow_mut() = Some(OverlayResult::ScopedModelsCancelled);
        }),
    };

    let config = ModelsConfig {
        all_models,
        enabled_model_ids: current_enabled,
    };

    let selector = ScopedModelsSelector::new(config, callbacks);
    tui.show_positioned_overlay(Box::new(selector), crate::tui::OverlayPosition::Bottom);
}

pub fn show_help_overlay(app: &mut App, tui: &mut TUI) {
    let mut overlay = crate::agent::ui::help::HelpOverlay::new(&app.theme);
    overlay.set_commands(app.commands.clone());
    overlay.set_dismiss_signal(app.overlay_result_signal.clone());
    tui.show_overlay(Box::new(overlay), Default::default());
}

/// Refresh all agent-facing configuration (system prompt, tools, commands,
/// skills, autocomplete) from the current extension enablement state.
///
/// This is the single method for rebuilding everything that depends on which
/// extensions are enabled. Called by:
/// - `/extensions` toggle (extension enablement changed)
/// - `/reload` handler (files on disk may have changed)
pub fn refresh_agent_config(app: &mut App) {
    fn is_enabled(ext: &dyn Extension, settings: &crate::settings::Settings) -> bool {
        crate::extension::is_extension_enabled(ext, settings)
    }

    // ── Collect tools from enabled extensions ────────────────────
    let enabled_exts: Vec<&dyn Extension> = app
        .extensions
        .iter()
        .filter(|ext| is_enabled(ext.as_ref(), &app.settings))
        .map(|b| b.as_ref())
        .collect();

    let all_tools: Vec<crate::extension::ToolDefinition> =
        enabled_exts.iter().flat_map(|ext| ext.tools()).collect();
    let tool_snippets: Vec<crate::agent::ToolSnippet> = all_tools
        .iter()
        .map(|twm| crate::agent::ToolSnippet {
            name: twm.name().to_string(),
            description: twm.snippet.to_string(),
        })
        .collect();

    let tool_guidelines: Vec<String> = all_tools
        .iter()
        .flat_map(|twm| twm.guidelines.iter().copied())
        .map(|s| s.to_string())
        .collect();

    let has_read_tool = tool_snippets.iter().any(|t| t.name == "read");

    // ── Re-register hooks from enabled extensions ────────────────
    use crate::extension::HookRegistration;
    let enabled_hooks: Vec<HookRegistration> = enabled_exts
        .iter()
        .flat_map(|ext| ext.tool_hooks())
        .collect();
    crate::extension::clear_tool_hooks();
    crate::extension::register_tool_hooks(&enabled_hooks);

    // ── Collect skills: disk-loaded + enabled extensions ─────────
    let mut all_skills = yoagent::skills::SkillSet::load(&app.skill_dirs).unwrap_or_default();
    for ext in &enabled_exts {
        all_skills.merge(ext.skills());
    }
    app.skills = all_skills.skills().to_vec();

    // ── Rebuild commands list and editor autocomplete ────────────
    app.commands = enabled_exts
        .iter()
        .flat_map(|e| e.commands())
        .map(|c| (c.name, c.description))
        .collect();
    for skill in &app.skills {
        app.commands
            .push((format!("skill:{}", skill.name), skill.description.clone()));
    }
    for template in &app.prompt_templates {
        app.commands
            .push((template.name.clone(), template.description.clone()));
    }

    // Rebuild editor autocomplete
    {
        use crate::tui::autocomplete::AutocompleteItem as AutoAutocompleteItem;
        use crate::tui::autocomplete::SlashCommand as AutoSlashCommand;

        let mut auto_commands: Vec<AutoSlashCommand> = enabled_exts
            .iter()
            .flat_map(|e| e.commands())
            .map(|cmd| {
                let handler = cmd.handler;
                AutoSlashCommand {
                    name: cmd.name,
                    description: Some(cmd.description),
                    argument_hint: None,
                    argument_completions: None,
                    get_argument_completions: Some(std::sync::Arc::new(
                        move |prefix: &str| -> Vec<AutoAutocompleteItem> {
                            handler
                                .argument_completions(prefix)
                                .into_iter()
                                .map(|item| AutoAutocompleteItem {
                                    value: item.value,
                                    label: item.label,
                                    description: item.description,
                                })
                                .collect()
                        },
                    )),
                }
            })
            .collect();

        // Re-register /skill:name commands
        for skill in &app.skills {
            let cmd_name = format!("skill:{}", skill.name);
            auto_commands.push(AutoSlashCommand {
                name: cmd_name,
                description: Some(skill.description.clone()),
                argument_hint: None,
                argument_completions: None,
                get_argument_completions: None,
            });
        }

        // Re-register prompt template commands
        for template in &app.prompt_templates {
            auto_commands.push(AutoSlashCommand {
                name: template.name.clone(),
                description: Some(template.description.clone()),
                argument_hint: template.argument_hint.clone(),
                argument_completions: None,
                get_argument_completions: None,
            });
        }

        app.editor.borrow_mut().set_slash_commands(auto_commands);
    }

    // ── Reload context files and system prompts from disk ────────
    let context_files = crate::agent::context_files::load_context_files(&app.cwd, &app.agent_dir);

    let custom_system_md = {
        let project_path = app.cwd.join(".rab").join("SYSTEM.md");
        if project_path.exists() {
            std::fs::read_to_string(&project_path).ok()
        } else {
            let global_path = app.agent_dir.join("SYSTEM.md");
            if global_path.exists() {
                std::fs::read_to_string(&global_path).ok()
            } else {
                None
            }
        }
    };
    let append_system_md = {
        let project_path = app.cwd.join(".rab").join("APPEND_SYSTEM.md");
        if project_path.exists() {
            std::fs::read_to_string(&project_path).ok()
        } else {
            let global_path = app.agent_dir.join("APPEND_SYSTEM.md");
            if global_path.exists() {
                std::fs::read_to_string(&global_path).ok()
            } else {
                None
            }
        }
    };

    // Update header context file display names (same format as startup)
    let context_file_list: Vec<String> = context_files
        .iter()
        .map(|cf| crate::cli::args::format_context_path(&cf.path, &app.cwd))
        .collect();
    app.context_files = context_file_list;

    // ── Build system prompt ──────────────────────────────────────
    let new_system_prompt = crate::agent::SystemPromptBuilder::new()
        .tool_snippets(tool_snippets)
        .guidelines(tool_guidelines)
        .context_files(context_files)
        .custom_prompt(custom_system_md)
        .append_prompt(append_system_md)
        .skills(all_skills)
        .has_read_tool(has_read_tool)
        .cwd(&app.cwd)
        .build();
    app.system_prompt = new_system_prompt;
}

/// Show a summarization choice prompt after tree entry selection (matching pi's showExtensionSelector).
/// Shows "No summary", "Summarize", and "Summarize with custom prompt" options.
pub fn show_summarization_prompt(app: &mut App, tui: &mut TUI, _entry_id: &str) {
    use crate::tui::keybindings::{
        ACTION_EDITOR_DELETE_CHAR_BACKWARD, ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM,
        ACTION_SELECT_DOWN, ACTION_SELECT_UP, get_keybindings,
    };
    use crossterm::event::KeyEvent;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct SummarizationPrompt {
        selected_index: usize,
        items: [&'static str; 3],
        signal: Rc<RefCell<Option<OverlayResult>>>,
        entry_id: String,
        edit_mode: bool,
        edit_text: String,
    }

    impl Component for SummarizationPrompt {
        fn render(&mut self, width: usize) -> Vec<String> {
            let theme = crate::agent::ui::theme::current_theme();
            let mut lines = Vec::new();

            lines.push(theme.fg("muted", &"─".repeat(width.saturating_sub(2))));
            lines.push(String::new());
            lines.push(format!("  {}", theme.bold("Summarize branch?")));
            lines.push(String::new());

            if self.edit_mode {
                // Show editor for custom summarization instructions
                lines.push(format!(
                    "  {}",
                    theme.fg("muted", "Custom summarization instructions (Enter to submit, Shift/Ctrl+Enter for newline):")
                ));
                lines.push(String::new());
                // Render multi-line text content
                if self.edit_text.is_empty() {
                    lines.push(format!(
                        "  {}",
                        theme.fg("muted", "<type here, Enter for newline>")
                    ));
                } else {
                    for line in self.edit_text.lines() {
                        lines.push(format!("  {}", line));
                    }
                }
                lines.push(String::new());
                lines.push(format!(
                    "  {}",
                    theme.fg(
                        "muted",
                        "Enter: submit \u{00b7} Shift/Ctrl+Enter: newline \u{00b7} Esc: back"
                    )
                ));
            } else {
                for (i, item) in self.items.iter().enumerate() {
                    let prefix = if i == self.selected_index {
                        theme.fg("accent", "\u{203a} ")
                    } else {
                        "  ".to_string()
                    };
                    let text = if i == self.selected_index {
                        theme.fg("accent", item)
                    } else {
                        theme.text_color(item)
                    };
                    lines.push(format!("{}{}", prefix, text));
                }
                lines.push(String::new());
                lines.push(theme.fg(
                    "muted",
                    "  \u{2191}/\u{2193} navigate \u{00b7} Enter select \u{00b7} Esc back to tree",
                ));
            }

            lines
        }

        fn handle_input(&mut self, key: &KeyEvent) -> bool {
            let kb = get_keybindings();

            if self.edit_mode {
                if key.code == crossterm::event::KeyCode::Esc {
                    self.edit_mode = false;
                    return true;
                }
                // Enter submits
                if key.code == crossterm::event::KeyCode::Enter
                    && !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::SHIFT)
                    && !key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    && !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL)
                {
                    let instructions = self.edit_text.trim().to_string();
                    let ci = if instructions.is_empty() {
                        None
                    } else {
                        Some(instructions)
                    };
                    *self.signal.borrow_mut() = Some(OverlayResult::TreeSummarizeChoice {
                        entry_id: self.entry_id.clone(),
                        summarize: true,
                        custom_instructions: ci,
                    });
                    return true;
                }
                // Shift+Enter, Ctrl+Enter, or Ctrl+J inserts newline
                if (key.code == crossterm::event::KeyCode::Enter
                    && (key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::SHIFT)
                        || key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)))
                    || (key.code == crossterm::event::KeyCode::Char('j')
                        && key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL))
                {
                    self.edit_text.push('\n');
                    return true;
                }
                if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_BACKWARD) {
                    self.edit_text.pop();
                    return true;
                }
                if let crossterm::event::KeyCode::Char(c) = key.code
                    && !c.is_control()
                {
                    self.edit_text.push(c);
                    return true;
                }
                return true;
            }

            if kb.matches(key, ACTION_SELECT_UP) {
                self.selected_index = if self.selected_index == 0 {
                    self.items.len() - 1
                } else {
                    self.selected_index - 1
                };
                return true;
            }

            if kb.matches(key, ACTION_SELECT_DOWN) {
                self.selected_index = if self.selected_index >= self.items.len() - 1 {
                    0
                } else {
                    self.selected_index + 1
                };
                return true;
            }

            if kb.matches(key, ACTION_SELECT_CONFIRM) {
                match self.selected_index {
                    0 => {
                        *self.signal.borrow_mut() = Some(OverlayResult::TreeSummarizeChoice {
                            entry_id: self.entry_id.clone(),
                            summarize: false,
                            custom_instructions: None,
                        });
                    }
                    1 => {
                        *self.signal.borrow_mut() = Some(OverlayResult::TreeSummarizeChoice {
                            entry_id: self.entry_id.clone(),
                            summarize: true,
                            custom_instructions: None,
                        });
                    }
                    2 => {
                        self.edit_mode = true;
                        self.edit_text.clear();
                        return true;
                    }
                    _ => {}
                }
                return true;
            }

            if kb.matches(key, ACTION_SELECT_CANCEL) {
                *self.signal.borrow_mut() = Some(OverlayResult::TreeReopen(self.entry_id.clone()));
                return true;
            }

            false
        }

        fn invalidate(&mut self) {}
    }

    let entry_id = _entry_id.to_string();
    let prompt = SummarizationPrompt {
        selected_index: 0,
        items: ["No summary", "Summarize", "Summarize with custom prompt"],
        signal: app.overlay_result_signal.clone(),
        entry_id,
        edit_mode: false,
        edit_text: String::new(),
    };

    tui.show_positioned_overlay(Box::new(prompt), crate::tui::OverlayPosition::Bottom);
}

/// Apply a setting change from the settings menu.
/// Updates app state and persists to settings.
pub fn apply_settings_change(app: &mut App, id: &str, new_value: &str) {
    match id {
        "autocompact" => {
            app.auto_compact = new_value == "true";
            app.footer.borrow_mut().set_auto_compact(app.auto_compact);
            if let Some(ref mut s) = app.session {
                s.set_auto_compact(app.auto_compact);
            }
            app.settings.set_auto_compact(Some(app.auto_compact));
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save auto-compact: {}", e));
            }
        }
        "hide-thinking" => {
            app.hide_thinking = new_value == "true";
            app.propagate_hide_thinking();
            app.settings.set_hide_thinking(Some(app.hide_thinking));
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save hide thinking: {}", e));
            }
        }
        "collapse-tool-output" => {
            app.collapse_tool_output = new_value == "true";
            app.tools_expanded = !app.collapse_tool_output;
            app.header.borrow_mut().set_expanded(app.tools_expanded);
            {
                let mut chat = app.chat_container.borrow_mut();
                for child in chat.children_mut().iter_mut() {
                    child.set_expanded(app.tools_expanded);
                }
            }
            app.settings
                .set_collapse_tool_output(Some(app.collapse_tool_output));
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save tool output setting: {}", e));
            }
        }
        "thinking-level" => {
            let level = if new_value == "off" {
                None
            } else {
                Some(new_value.to_string())
            };
            app.thinking_level = level.clone();
            app.footer.borrow_mut().set_thinking_level(level.clone());
            app.settings.set_default_thinking_level(level);
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save thinking level: {}", e));
            }
        }
        "theme" => {
            if theme::set_theme(new_value).is_ok() {
                app.theme = theme::current_theme().clone();
                // Persist to settings + save
                app.settings.theme = Some(new_value.to_string());
                app.settings.mark_modified("theme");
                if let Err(e) = app.settings.save() {
                    app.status_text = Some(format!("Failed to save theme: {}", e));
                }
                app.status_text = Some(format!("Theme: {}", new_value));
            }
        }
        "transport" => {
            app.settings.transport = Some(new_value.to_string());
            app.settings.mark_modified("transport");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save transport: {}", e));
            }
        }
        "steering-mode" => {
            app.settings.steering_mode = Some(new_value.to_string());
            app.settings.mark_modified("steeringMode");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save steering mode: {}", e));
            }
        }
        "follow-up-mode" => {
            app.settings.follow_up_mode = Some(new_value.to_string());
            app.settings.mark_modified("followUpMode");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save follow-up mode: {}", e));
            }
        }
        "quiet-startup" => {
            app.settings.quiet_startup = Some(new_value == "true");
            app.settings.mark_modified("quietStartup");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save quiet startup: {}", e));
            }
        }
        "collapse-changelog" => {
            app.settings.collapse_changelog = Some(new_value == "true");
            app.settings.mark_modified("collapseChangelog");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save collapse changelog: {}", e));
            }
        }
        "verbose" => {
            app.settings.verbose = new_value == "true";
            app.settings.mark_modified("verbose");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save verbose: {}", e));
            }
        }
        "double-escape-action" => {
            app.settings.double_escape_action = Some(new_value.to_string());
            app.settings.mark_modified("doubleEscapeAction");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save double-escape action: {}", e));
            }
        }
        "tree-filter-mode" => {
            app.settings.tree_filter_mode = Some(new_value.to_string());
            app.settings.mark_modified("treeFilterMode");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save tree filter mode: {}", e));
            }
        }
        "show-hardware-cursor" => {
            app.settings.show_hardware_cursor = Some(new_value == "true");
            app.settings.mark_modified("showHardwareCursor");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save hardware cursor: {}", e));
            }
        }
        "editor-padding" => {
            if let Ok(v) = new_value.parse::<i32>() {
                app.settings.editor_padding_x = Some(v);
                app.settings.mark_modified("editorPaddingX");
                if let Err(e) = app.settings.save() {
                    app.status_text = Some(format!("Failed to save editor padding: {}", e));
                }
            }
        }
        "output-padding" => {
            if let Ok(v) = new_value.parse::<i32>() {
                app.settings.output_pad = Some(v);
                app.settings.mark_modified("outputPad");
                if let Err(e) = app.settings.save() {
                    app.status_text = Some(format!("Failed to save output padding: {}", e));
                }
            }
        }
        "autocomplete-max-visible" => {
            if let Ok(v) = new_value.parse::<i32>() {
                app.settings.autocomplete_max_visible = Some(v);
                app.settings.mark_modified("autocompleteMaxVisible");
                if let Err(e) = app.settings.save() {
                    app.status_text = Some(format!("Failed to save autocomplete max: {}", e));
                }
            }
        }
        "skill-commands" => {
            app.settings.enable_skill_commands = Some(new_value == "true");
            app.settings.mark_modified("enableSkillCommands");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save skill commands: {}", e));
            }
        }
        "install-telemetry" => {
            app.settings.enable_install_telemetry = Some(new_value == "true");
            app.settings.mark_modified("enableInstallTelemetry");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save install telemetry: {}", e));
            }
        }
        "default-project-trust" => {
            app.settings.default_project_trust = Some(new_value.to_string());
            app.settings.mark_modified("defaultProjectTrust");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save project trust: {}", e));
            }
        }
        "http-idle-timeout" => {
            let ms = match new_value {
                "30 sec" => 30_000u64,
                "1 min" => 60_000,
                "2 min" => 120_000,
                "5 min" => 300_000,
                "disabled" => 0,
                _ => 30_000,
            };
            app.settings.http_idle_timeout_ms = Some(ms);
            app.settings.mark_modified("httpIdleTimeoutMs");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save HTTP idle timeout: {}", e));
            }
        }
        "clear-on-shrink" => {
            let val = new_value == "true";
            let terminal = app.settings.terminal.get_or_insert_with(Default::default);
            terminal.clear_on_shrink = Some(val);
            app.settings
                .mark_nested_modified("terminal", "clearOnShrink");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save clear on shrink: {}", e));
            }
        }
        "terminal-progress" => {
            let val = new_value == "true";
            let terminal = app.settings.terminal.get_or_insert_with(Default::default);
            terminal.show_terminal_progress = Some(val);
            app.settings
                .mark_nested_modified("terminal", "showTerminalProgress");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save terminal progress: {}", e));
            }
        }
        "warnings-anthropic-extra-usage" => {
            let val = new_value == "true";
            let warnings = app.settings.warnings.get_or_insert_with(Default::default);
            warnings.anthropic_extra_usage = Some(val);
            app.settings
                .mark_nested_modified("warnings", "anthropicExtraUsage");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save warning setting: {}", e));
            }
        }
        _ => {}
    }
}
