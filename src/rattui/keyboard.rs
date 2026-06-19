use super::app::{App, create_editor, submit_message};
use super::display::DisplayMsg;
use super::model_selector::filter_models;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) fn scroll_up(app: &mut App, lines: usize) {
    app.auto_scroll.set(false);
    let current = app.scroll_offset.get();
    app.scroll_offset.set(current.saturating_sub(lines));
}

pub(crate) fn scroll_down(app: &mut App, lines: usize) {
    if app.auto_scroll.get() {
        return;
    }
    let current = app.scroll_offset.get();
    app.scroll_offset.set(current.saturating_add(lines));
}

pub(crate) fn recall_history(app: &mut App, direction: isize) {
    // Collect user messages from conversation (newest last)
    let user_messages: Vec<&str> = app
        .conversation
        .iter()
        .filter_map(|m| {
            if m.role == crate::types::Role::User && !m.content.is_empty() {
                Some(m.content.as_str())
            } else {
                None
            }
        })
        .collect();

    if user_messages.is_empty() {
        return;
    }

    let len = user_messages.len();
    let current = app.history_index.unwrap_or(len);

    let new_index = if direction < 0 {
        if current == 0 {
            return;
        }
        current.saturating_sub(1)
    } else {
        if current >= len {
            return;
        }
        current + 1
    };

    if new_index >= len {
        app.editor = create_editor(app);
        app.history_index = None;
    } else {
        let mut editor = create_editor(app);
        editor.set_text(user_messages[new_index]);
        app.editor = editor;
        app.history_index = Some(new_index);
    }
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let _shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let _alt = key.modifiers.contains(KeyModifiers::ALT);

    // ── Model selector input mode ──
    if app.show_model_selector {
        handle_model_selector_key(app, key);
        return;
    }

    match key.code {
        // Tab: pass to editor for autocomplete (slash commands, file paths)
        KeyCode::Tab | KeyCode::Char('\t') => {
            if app.show_help {
                app.show_help = false;
                return;
            }
            app.editor
                .handle_key(key.code, key.modifiers.contains(KeyModifiers::CONTROL));
        }
        // Ctrl+C: interrupt streaming, or clear editor (pi: app.interrupt)
        KeyCode::Char('c') if ctrl => {
            if app.show_help {
                app.show_help = false;
            } else if app.is_streaming {
                if let Some(handle) = app.agent_abort.take() {
                    handle.abort();
                }
                app.is_streaming = false;
                app.messages.push(DisplayMsg::Info("Aborted".to_string()));
            } else {
                app.editor = create_editor(app);
                app.history_index = None;
            }
        }
        // Ctrl+D: quit when streaming or editor empty
        KeyCode::Char('d') if ctrl => {
            if app.show_help {
                app.show_help = false;
            } else if app.is_streaming {
                if let Some(handle) = app.agent_abort.take() {
                    handle.abort();
                }
                app.is_streaming = false;
                app.messages.push(DisplayMsg::Info("Aborted".to_string()));
            } else if app.editor.is_empty() {
                app.should_quit = true;
            }
        }
        // Escape: close autocomplete, help, or abort streaming (pi: app.interrupt)
        // Also match Char('\x1b') — some terminals send Esc this way with mouse capture
        KeyCode::Esc | KeyCode::Char('\x1b') => {
            if app.editor.autocomplete_active() {
                app.editor.dismiss_autocomplete();
            } else if app.show_help {
                app.show_help = false;
            } else if app.is_streaming {
                if let Some(handle) = app.agent_abort.take() {
                    handle.abort();
                }
                app.is_streaming = false;
                app.messages.push(DisplayMsg::Info("Aborted".to_string()));
            }
        }
        // Ctrl+T: toggle thinking (pi-style, persisted to settings.json)
        KeyCode::Char('t') if ctrl => {
            app.hide_thinking = !app.hide_thinking;
            if let Ok(mut settings) = crate::settings::Settings::load(&app.cwd) {
                settings.hide_thinking = Some(app.hide_thinking);
                let _ = settings.save();
            }
            app.messages.push(DisplayMsg::Info(format!(
                "Thinking blocks: {}",
                if app.hide_thinking {
                    "hidden"
                } else {
                    "visible"
                }
            )));
        }
        // Ctrl+L: open model selector (pi-style)
        KeyCode::Char('l') if ctrl => {
            app.show_model_selector = true;
            app.model_search.clear();
            app.model_selector_selection = app
                .available_models
                .iter()
                .position(|m| m == &app.model || format!("opencode_go::{m}") == app.model)
                .unwrap_or(0);
        }
        // Ctrl+O: toggle tool output (pi-style, persisted to settings.json)
        KeyCode::Char('o') if ctrl => {
            app.tool_output_collapsed = !app.tool_output_collapsed;
            if let Ok(mut settings) = crate::settings::Settings::load(&app.cwd) {
                settings.collapse_tool_output = Some(app.tool_output_collapsed);
                let _ = settings.save();
            }
            app.messages.push(DisplayMsg::Info(format!(
                "Tool output: {}",
                if app.tool_output_collapsed {
                    "collapsed"
                } else {
                    "expanded"
                }
            )));
        }
        // Ctrl+J: newline (terminal-independent, works on all terminals)
        KeyCode::Char('j') if ctrl => {
            app.editor
                .handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        }
        // F1: show help
        KeyCode::F(1) => {
            app.show_help = !app.show_help;
        }
        // Enter (no modifiers): submit
        KeyCode::Enter => {
            if app.editor.autocomplete_active() {
                app.editor.accept_autocomplete_if_active();
                return;
            }
            let text = app.editor.text();
            let trimmed = text.trim();
            if !trimmed.is_empty() && !app.is_streaming {
                submit_message(app, trimmed.to_string());
            }
        }
        // Arrow up: recall previous message when editor is empty
        KeyCode::Up if app.editor.is_empty() && !app.is_streaming => {
            recall_history(app, -1);
        }
        // Arrow down: recall next message when editor is empty
        KeyCode::Down if app.editor.is_empty() && !app.is_streaming => {
            recall_history(app, 1);
        }
        // PageUp/PageDown → scroll messages
        KeyCode::PageUp => scroll_up(app, 10),
        KeyCode::PageDown => scroll_down(app, 10),
        // Everything else: pass to editor
        _ => {
            app.editor
                .handle_key(key.code, key.modifiers.contains(KeyModifiers::CONTROL));
        }
    }
}

pub(crate) fn handle_model_selector_key(app: &mut App, key: KeyEvent) {
    let filtered = filter_models(&app.available_models, &app.model_search);
    let max_index = filtered.len().saturating_sub(1);

    match key.code {
        KeyCode::Esc | KeyCode::Char('\x1b') => {
            app.show_model_selector = false;
            app.model_search.clear();
        }
        KeyCode::Enter => {
            if app.model_selector_selection < filtered.len() {
                let selected = filtered[app.model_selector_selection].to_string();
                app.show_model_selector = false;
                app.model_search.clear();
                app.model = selected.clone();
                if let Ok(mut settings) = crate::settings::Settings::load(&app.cwd) {
                    settings.default_model = Some(selected.clone());
                    let _ = settings.save();
                }
                app.messages.push(DisplayMsg::Info(format!(
                    "Model: {}",
                    selected.replace("opencode_go::", "")
                )));
            }
        }
        KeyCode::Up => {
            if !filtered.is_empty() {
                app.model_selector_selection = if app.model_selector_selection == 0 {
                    max_index
                } else {
                    app.model_selector_selection - 1
                };
            }
        }
        KeyCode::Down => {
            if !filtered.is_empty() {
                app.model_selector_selection = if app.model_selector_selection >= max_index {
                    0
                } else {
                    app.model_selector_selection + 1
                };
            }
        }
        // Tab cycles to top/bottom
        KeyCode::Tab => {
            if !filtered.is_empty() {
                app.model_selector_selection = 0;
            }
        }
        // Home / End jump to first / last
        KeyCode::Home => {
            if !filtered.is_empty() {
                app.model_selector_selection = 0;
            }
        }
        KeyCode::End => {
            app.model_selector_selection = max_index;
        }
        // Backspace: delete last char from search
        KeyCode::Backspace => {
            app.model_search.pop();
            app.model_selector_selection = app.model_selector_selection.min(max_index);
        }
        // Char: add to search, reset selection
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.model_search.push(c);
            app.model_selector_selection = 0;
        }
        _ => {}
    }
}

pub(crate) fn parse_bang_command(input: &str) -> Option<(&str, bool)> {
    if let Some(rest) = input.strip_prefix("!!") {
        let cmd = rest.trim();
        if cmd.is_empty() {
            None
        } else {
            Some((cmd, true))
        }
    } else if let Some(rest) = input.strip_prefix('!') {
        let cmd = rest.trim();
        if cmd.is_empty() {
            None
        } else {
            Some((cmd, false))
        }
    } else {
        None
    }
}
