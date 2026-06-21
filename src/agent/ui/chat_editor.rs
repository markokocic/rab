use crossterm::event::{KeyCode, KeyEvent};

use crate::tui::Component;
use crate::tui::Theme;
use crate::tui::components::Editor;
use crate::tui::components::editor::{EditorOptions, EditorTheme};
use crate::tui::keys::{Key, matches_key};

/// Actions that ChatEditor can signal to the app layer.
/// Mirrors pi's CustomEditor approach: the editor handles its own text keys
/// and returns an action for app-level keybindings.
#[derive(Debug)]
pub enum InputAction {
    /// Key was consumed by the editor (text editing, navigation, etc.)
    Handled,
    /// Escape pressed (app should clear/abort)
    Escape,
    /// Ctrl+C pressed (app should interrupt streaming or clear)
    Interrupt,
    /// Ctrl+D pressed while editor is empty (app should quit)
    Exit,
    /// Ctrl+L pressed (app should open model selector)
    ModelSelector,
    /// Ctrl+T pressed (app should toggle thinking visibility)
    ToggleThinking,
    /// Ctrl+O pressed (app should toggle tool output collapse)
    ToggleCollapse,
    /// F1 pressed (app should show help overlay)
    Help,
    /// Enter pressed with text (app should submit the message)
    Submit(String),
    /// Up/Down arrow to recall history
    RecallHistory(isize),
    /// PageUp pressed (app should scroll up)
    PageUp,
    /// PageDown pressed (app should scroll down)
    PageDown,
}

/// Rab-specific chat editor that wraps the core tui::Editor.
///
/// Mirrors pi's CustomEditor pattern: ChatEditor handles keyboard input and
/// dispatches app-level actions (escape, submit, model selector, etc.) as
/// an InputAction enum, while text-editing keys are delegated to the inner
/// Editor. The app layer matches on InputAction to perform side effects.
pub struct ChatEditor {
    pub editor: Editor,
    /// Available slash command names for autocomplete.
    slash_commands: Vec<String>,
    /// CWD for file-path completion.
    cwd: std::path::PathBuf,
}

impl ChatEditor {
    pub fn new(theme: &dyn Theme, cwd: std::path::PathBuf) -> Self {
        let editor_theme = EditorTheme {
            text: {
                let theme_text = theme.fg("text", "").to_string();
                Box::new(move |s| {
                    if !theme_text.is_empty() && theme_text.starts_with('\x1b') {
                        let prefix = &theme_text[..theme_text.len().saturating_sub(1)];
                        format!("{}m{}", &prefix[2..prefix.len()], s)
                    } else {
                        s.to_string()
                    }
                })
            },
            cursor: Box::new(|s| format!("\x1b[7m{}\x1b[27m", s)),
            border: Box::new(move |s| format!("\x1b[38;2;138;190;183m{}\x1b[39m", s)),
            scroll_indicator: Box::new(move |s| format!("\x1b[38;2;128;128;128m{}\x1b[39m", s)),
            autocomplete_selected: Box::new(|s| {
                format!("\x1b[7m\x1b[38;2;138;190;183m{}\x1b[27m\x1b[39m", s)
            }),
            autocomplete_normal: Box::new(|s| format!("\x1b[38;2;128;128;128m{}\x1b[39m", s)),
        };

        let editor = Editor::new(
            editor_theme,
            EditorOptions {
                padding_x: 1,
                max_visible_lines: 10,
            },
        );

        Self {
            editor,
            slash_commands: Vec::new(),
            cwd,
        }
    }

    /// Set the available slash commands for autocomplete.
    pub fn set_slash_commands(&mut self, commands: Vec<String>) {
        self.slash_commands = commands;
    }

    /// Update the working directory.
    pub fn set_cwd(&mut self, cwd: std::path::PathBuf) {
        self.cwd = cwd;
    }

    /// Check if the current input should trigger autocomplete.
    pub fn get_autocomplete_suggestions(
        &self,
    ) -> Vec<crate::tui::components::select_list::SelectItem> {
        let text = self.editor.get_text();

        if text.starts_with('/') {
            let cmd_part = text.trim_start_matches('/');
            let matches: Vec<_> = self
                .slash_commands
                .iter()
                .filter(|c| c.starts_with(cmd_part))
                .map(|c| crate::tui::components::select_list::SelectItem::new(c.clone(), c.clone()))
                .collect();
            return matches;
        }

        Vec::new()
    }

    /// Handle keyboard input. Mirrors pi's CustomEditor.handleInput:
    ///
    /// 1. Checks app-level keys (escape, interrupt, submit, model selector, etc.)
    ///    and returns the corresponding InputAction for the app layer to handle.
    /// 2. For text-editing keys, delegates to the inner Editor.handle_input.
    ///
    /// This keeps app-level side effects (aborting agent, opening overlays, etc.)
    /// in the app layer while keeping text-editing logic in the Editor component.
    pub fn handle_input(&mut self, key: &KeyEvent) -> InputAction {
        // ── Escape: close autocomplete first if active, else signal app ──
        if matches_key(key, &Key::Escape) {
            if self.editor.autocomplete_active {
                self.editor.clear_autocomplete();
                return InputAction::Handled;
            }
            return InputAction::Escape;
        }

        // ── Ctrl+C: interrupt or clear ──
        if matches_key(key, &Key::Ctrl('c')) {
            return InputAction::Interrupt;
        }

        // ── Ctrl+D: exit when editor is empty (mirrors pi's app.exit) ──
        if matches_key(key, &Key::Ctrl('d')) && self.editor.get_text().is_empty() {
            return InputAction::Exit;
        }

        // ── Ctrl+L: model selector ──
        if matches_key(key, &Key::Ctrl('l')) {
            return InputAction::ModelSelector;
        }

        // ── Ctrl+T: toggle thinking visibility ──
        if matches_key(key, &Key::Ctrl('t')) {
            return InputAction::ToggleThinking;
        }

        // ── Ctrl+O: toggle tool output collapse ──
        if matches_key(key, &Key::Ctrl('o')) {
            return InputAction::ToggleCollapse;
        }

        // ── F1: help overlay ──
        if key.code == KeyCode::F(1) {
            return InputAction::Help;
        }

        // ── Tab: trigger slash-command autocomplete (pi-style) ──
        if matches_key(key, &Key::Tab) && !self.editor.autocomplete_active {
            let text = self.editor.get_text();
            if text.starts_with('/') {
                let suggestions = self.get_autocomplete_suggestions();
                self.editor.set_autocomplete(suggestions);
            }
            return InputAction::Handled;
        }

        // ── Enter: submit ──
        if matches_key(key, &Key::Enter) {
            let text = self.editor.get_text();
            if !text.trim().is_empty() {
                self.editor.add_to_history(&text);
                self.editor.set_text("");
                return InputAction::Submit(text);
            }
            return InputAction::Handled;
        }

        // ── Ctrl+J: insert literal newline ──
        if matches_key(key, &Key::Ctrl('j')) {
            self.editor.insert_text_at_cursor("\n");
            return InputAction::Handled;
        }

        // ── Up/Down: recall history (only when autocomplete is not active) ──
        if !self.editor.autocomplete_active {
            if matches_key(key, &Key::Up) && self.editor.get_text().is_empty() {
                return InputAction::RecallHistory(-1);
            }
            if matches_key(key, &Key::Down) && self.editor.get_text().is_empty() {
                return InputAction::RecallHistory(1);
            }
        }

        // ── PageUp/PageDown: scroll ──
        if matches_key(key, &Key::PageUp) {
            return InputAction::PageUp;
        }
        if matches_key(key, &Key::PageDown) {
            return InputAction::PageDown;
        }

        // ── All other keys: delegate to the core Editor for text editing ──
        self.editor.handle_input(key);
        InputAction::Handled
    }
}
