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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::NoopTheme;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn make_editor() -> ChatEditor {
        let theme = NoopTheme::default();
        ChatEditor::new(&theme, std::env::temp_dir().into())
    }

    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    fn escape() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    fn tab() -> KeyEvent {
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
    }

    fn up() -> KeyEvent {
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)
    }

    fn down() -> KeyEvent {
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)
    }

    fn page_up() -> KeyEvent {
        KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)
    }

    fn page_down() -> KeyEvent {
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)
    }

    fn f1() -> KeyEvent {
        KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)
    }

    // ── App-level key tests ──

    #[test]
    fn test_escape_closes_autocomplete() {
        let mut ed = make_editor();
        ed.editor.set_text("/");
        ed.set_slash_commands(vec!["help".into()]);
        // Trigger autocomplete
        let suggestions = ed.get_autocomplete_suggestions();
        ed.editor.set_autocomplete(suggestions);
        assert!(
            ed.editor.autocomplete_active,
            "autocomplete should be active"
        );

        let action = ed.handle_input(&escape());
        assert!(matches!(action, InputAction::Handled));
        assert!(!ed.editor.autocomplete_active, "autocomplete should close");
    }

    #[test]
    fn test_escape_no_autocomplete_returns_action() {
        let mut ed = make_editor();
        assert!(!ed.editor.autocomplete_active);
        let action = ed.handle_input(&escape());
        assert!(matches!(action, InputAction::Escape));
    }

    #[test]
    fn test_ctrl_c_returns_interrupt() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('c'));
        assert!(matches!(action, InputAction::Interrupt));
    }

    #[test]
    fn test_ctrl_d_empty_returns_exit() {
        let mut ed = make_editor();
        assert!(ed.editor.get_text().is_empty());
        let action = ed.handle_input(&ctrl('d'));
        assert!(matches!(action, InputAction::Exit));
    }

    #[test]
    fn test_ctrl_d_with_text_returns_handled() {
        let mut ed = make_editor();
        ed.editor.set_text("hello");
        // Move cursor to col 0 (start of text) so Ctrl+D deletes first char
        for _ in 0..5 {
            ed.handle_input(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        assert_eq!(ed.editor.get_cursor(), (0, 0));
        let action = ed.handle_input(&ctrl('d'));
        // Editor handles Ctrl+D as delete_forward
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "ello");
    }

    #[test]
    fn test_ctrl_l_returns_model_selector() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('l'));
        assert!(matches!(action, InputAction::ModelSelector));
    }

    #[test]
    fn test_ctrl_t_returns_toggle_thinking() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('t'));
        assert!(matches!(action, InputAction::ToggleThinking));
    }

    #[test]
    fn test_ctrl_o_returns_toggle_collapse() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('o'));
        assert!(matches!(action, InputAction::ToggleCollapse));
    }

    #[test]
    fn test_f1_returns_help() {
        let mut ed = make_editor();
        let action = ed.handle_input(&f1());
        assert!(matches!(action, InputAction::Help));
    }

    #[test]
    fn test_enter_with_text_submits_and_clears() {
        let mut ed = make_editor();
        ed.editor.set_text("hello world");
        let action = ed.handle_input(&enter());
        match action {
            InputAction::Submit(text) => {
                assert_eq!(text, "hello world");
            }
            other => panic!("Expected Submit, got {:?}", other),
        }
        assert!(
            ed.editor.get_text().is_empty(),
            "editor should clear on submit"
        );
    }

    #[test]
    fn test_enter_with_empty_text_returns_handled() {
        let mut ed = make_editor();
        let action = ed.handle_input(&enter());
        assert!(matches!(action, InputAction::Handled));
    }

    #[test]
    fn test_ctrl_j_inserts_newline() {
        let mut ed = make_editor();
        ed.editor.set_text("hello");
        // Cursor is at end by default (col 5)
        let action = ed.handle_input(&ctrl('j'));
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "hello\n");
    }

    #[test]
    fn test_tab_triggers_slash_autocomplete() {
        let mut ed = make_editor();
        ed.editor.set_text("/he");
        ed.set_slash_commands(vec!["help".into(), "history".into()]);
        assert!(!ed.editor.autocomplete_active);
        let action = ed.handle_input(&tab());
        assert!(matches!(action, InputAction::Handled));
        assert!(
            ed.editor.autocomplete_active,
            "autocomplete should activate"
        );
    }

    #[test]
    fn test_tab_no_slash_does_nothing() {
        let mut ed = make_editor();
        ed.editor.set_text("hello");
        let action = ed.handle_input(&tab());
        // Tab without / is handled by Editor as autocomplete navigation (but none active)
        assert!(matches!(action, InputAction::Handled));
    }

    #[test]
    fn test_tab_when_already_active_accepts_selection() {
        let mut ed = make_editor();
        ed.editor.set_text("/he");
        ed.set_slash_commands(vec!["help".into()]);
        // Manually activate autocomplete with a suggestion
        let suggestions = ed.get_autocomplete_suggestions();
        ed.editor.set_autocomplete(suggestions);
        assert!(ed.editor.autocomplete_active);

        // Tab while active should accept the selection and close autocomplete
        let action = ed.handle_input(&tab());
        assert!(matches!(action, InputAction::Handled));
        // Tab accepts selection and clears autocomplete (as per Editor behavior)
        assert!(!ed.editor.autocomplete_active);
        // The selected value should have been applied to the text
        assert_eq!(ed.editor.get_text(), "/help ");
    }

    #[test]
    fn test_up_when_empty_recalls_history() {
        let mut ed = make_editor();
        ed.editor.add_to_history("previous message");
        let action = ed.handle_input(&up());
        assert!(matches!(action, InputAction::RecallHistory(d) if d == -1));
    }

    #[test]
    fn test_up_when_not_empty_does_not_recall() {
        let mut ed = make_editor();
        ed.editor.set_text("typing...");
        let action = ed.handle_input(&up());
        // Up with text in a multi-line editor is editor navigation
        // (on single line, first press goes to start of line, second goes to history)
        assert!(matches!(action, InputAction::Handled));
    }

    #[test]
    fn test_down_when_empty_recalls_history() {
        let mut ed = make_editor();
        ed.editor.add_to_history("msg");
        // First press Up to enter history mode
        ed.handle_input(&up());
        let action = ed.handle_input(&down());
        assert!(matches!(action, InputAction::RecallHistory(d) if d == 1));
    }

    #[test]
    fn test_page_up_returns_page_up_action() {
        let mut ed = make_editor();
        let action = ed.handle_input(&page_up());
        assert!(matches!(action, InputAction::PageUp));
    }

    #[test]
    fn test_page_down_returns_page_down_action() {
        let mut ed = make_editor();
        let action = ed.handle_input(&page_down());
        assert!(matches!(action, InputAction::PageDown));
    }

    // ── Text editing keys (delegated to Editor) ──

    #[test]
    fn test_printable_char_inserts_text() {
        let mut ed = make_editor();
        let action = ed.handle_input(&char_key('a'));
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "a");
    }

    #[test]
    fn test_backspace_deletes() {
        let mut ed = make_editor();
        ed.editor.set_text("abc");
        // Cursor at end (col 3) by default
        let action = ed.handle_input(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "ab");
    }

    #[test]
    fn test_arrow_left_moves_cursor() {
        let mut ed = make_editor();
        ed.editor.set_text("abc");
        // set_text puts cursor at end (col 3)
        assert_eq!(ed.editor.get_cursor(), (0, 3));
        let action = ed.handle_input(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_cursor(), (0, 2));
    }

    #[test]
    fn test_ctrl_k_deletes_to_line_end() {
        let mut ed = make_editor();
        ed.editor.set_text("hello world");
        // Move cursor to position 6 (after "hello ") using left arrow
        for _ in 0..6 {
            ed.handle_input(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        assert_eq!(ed.editor.get_cursor(), (0, 5));
        let action = ed.handle_input(&ctrl('k'));
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "hello");
    }

    #[test]
    fn test_ctrl_z_delegates_to_editor() {
        let mut ed = make_editor();
        ed.editor.set_text("hello");
        let action = ed.handle_input(&ctrl('z'));
        // Ctrl+Z isn't intercepted by ChatEditor, delegated to Editor
        assert!(matches!(action, InputAction::Handled));
    }

    // ── History integration with ChatEditor ──

    #[test]
    fn test_submit_adds_to_history() {
        let mut ed = make_editor();
        ed.editor.set_text("test");
        let action = ed.handle_input(&enter());
        assert!(matches!(action, InputAction::Submit(_)));
        // History should now contain "test"
        let action2 = ed.handle_input(&up());
        assert!(matches!(action2, InputAction::RecallHistory(d) if d == -1));
    }

    // ── InputAction enum exhaustiveness ──

    #[test]
    fn test_input_action_debug() {
        // Verify all variants are constructible and debuggable
        let variants = vec![
            format!("{:?}", InputAction::Handled),
            format!("{:?}", InputAction::Escape),
            format!("{:?}", InputAction::Interrupt),
            format!("{:?}", InputAction::Exit),
            format!("{:?}", InputAction::ModelSelector),
            format!("{:?}", InputAction::ToggleThinking),
            format!("{:?}", InputAction::ToggleCollapse),
            format!("{:?}", InputAction::Help),
            format!("{:?}", InputAction::Submit("x".into())),
            format!("{:?}", InputAction::RecallHistory(1)),
            format!("{:?}", InputAction::PageUp),
            format!("{:?}", InputAction::PageDown),
        ];
        for v in &variants {
            assert!(!v.is_empty(), "Debug output should not be empty");
        }
    }
}
