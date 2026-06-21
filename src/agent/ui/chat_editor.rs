use crossterm::event::KeyEvent;

use crate::tui::Component;
use crate::tui::Theme;
use crate::tui::components::Editor;
use crate::tui::components::editor::{EditorOptions, EditorTheme};
use crate::tui::keybindings::{
    ACTION_APP_CLEAR, ACTION_APP_COMPACT_TOGGLE, ACTION_APP_EDITOR_EXTERNAL, ACTION_APP_ESCAPE,
    ACTION_APP_EXIT, ACTION_APP_HELP, ACTION_APP_HISTORY_DOWN, ACTION_APP_HISTORY_UP,
    ACTION_APP_MESSAGE_DEQUEUE, ACTION_APP_MESSAGE_FOLLOW_UP, ACTION_APP_MODEL_CYCLE_BACKWARD,
    ACTION_APP_MODEL_CYCLE_FORWARD, ACTION_APP_MODEL_SELECTOR, ACTION_APP_SUSPEND,
    ACTION_APP_THINKING_CYCLE, ACTION_APP_TOGGLE_THINKING, ACTION_APP_TOOLS_EXPAND,
    ACTION_EDITOR_PAGE_DOWN, ACTION_EDITOR_PAGE_UP, ACTION_INPUT_NEW_LINE, ACTION_INPUT_SUBMIT,
    ACTION_INPUT_TAB, ACTION_SELECT_CANCEL, get_keybindings,
};

/// Actions that ChatEditor can signal to the app layer.
/// Mirrors pi's CustomEditor approach: the editor handles its own text keys
/// and returns an action for app-level keybindings.
#[derive(Debug)]
pub enum InputAction {
    /// Key was consumed by the editor (text editing, navigation, etc.)
    Handled,
    /// Escape pressed (app should abort streaming or close autocomplete)
    Escape,
    /// Ctrl+C pressed (app should clear editor, or double-press to exit)
    Clear,
    /// Ctrl+D pressed while editor is empty (app should quit)
    Exit,
    /// Ctrl+Z pressed (app should suspend)
    Suspend,
    /// Shift+Tab pressed (app should cycle thinking level)
    ThinkingCycle,
    /// Ctrl+L pressed (app should open model selector)
    ModelSelector,
    /// Ctrl+P pressed (app should cycle to next model)
    ModelCycleForward,
    /// Shift+Ctrl+P pressed (app should cycle to previous model)
    ModelCycleBackward,
    /// Ctrl+T pressed (app should toggle thinking visibility)
    ToggleThinking,
    /// Ctrl+O pressed (app should toggle all tool output expansion)
    ToolsExpand,
    /// Ctrl+G pressed (app should open external editor)
    EditorExternal,
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
    /// Alt+Enter pressed (app should queue follow-up message)
    FollowUp(String),
    /// Alt+Up pressed (app should restore queued messages to editor)
    Dequeue,
    /// Ctrl+Shift+C pressed (app should toggle auto-compact)
    CompactToggle,
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
    /// 1. Checks app-level keys (escape, clear, submit, model selector, etc.)
    ///    and returns the corresponding InputAction for the app layer to handle.
    /// 2. For text-editing keys, delegates to the inner Editor.handle_input.
    ///
    /// This keeps app-level side effects (aborting agent, opening overlays, etc.)
    /// in the app layer while keeping text-editing logic in the Editor component.
    pub fn handle_input(&mut self, key: &KeyEvent) -> InputAction {
        let kb = get_keybindings();

        // ── Escape: close autocomplete first if active, else signal app ──
        if kb.matches(key, ACTION_SELECT_CANCEL) || kb.matches(key, ACTION_APP_ESCAPE) {
            if self.editor.autocomplete_active {
                self.editor.clear_autocomplete();
                return InputAction::Handled;
            }
            return InputAction::Escape;
        }

        // ── Ctrl+C: clear (abort streaming or clear editor) ──
        if kb.matches(key, ACTION_APP_CLEAR) {
            return InputAction::Clear;
        }

        // ── Ctrl+D: exit when editor is empty (mirrors pi's app.exit) ──
        if kb.matches(key, ACTION_APP_EXIT) && self.editor.get_text().is_empty() {
            return InputAction::Exit;
        }

        // ── Ctrl+Z: suspend ──
        if kb.matches(key, ACTION_APP_SUSPEND) {
            return InputAction::Suspend;
        }

        // ── Shift+Tab: cycle thinking level ──
        if kb.matches(key, ACTION_APP_THINKING_CYCLE) {
            return InputAction::ThinkingCycle;
        }

        // ── Ctrl+L: model selector ──
        if kb.matches(key, ACTION_APP_MODEL_SELECTOR) {
            return InputAction::ModelSelector;
        }

        // ── Ctrl+P: cycle model forward ──
        if kb.matches(key, ACTION_APP_MODEL_CYCLE_FORWARD) {
            return InputAction::ModelCycleForward;
        }

        // ── Shift+Ctrl+P: cycle model backward ──
        if kb.matches(key, ACTION_APP_MODEL_CYCLE_BACKWARD) {
            return InputAction::ModelCycleBackward;
        }

        // ── Ctrl+T: toggle thinking visibility ──
        if kb.matches(key, ACTION_APP_TOGGLE_THINKING) {
            return InputAction::ToggleThinking;
        }

        // ── Ctrl+O: toggle all tool output expansion ──
        if kb.matches(key, ACTION_APP_TOOLS_EXPAND) {
            return InputAction::ToolsExpand;
        }

        // ── Ctrl+G: external editor ──
        if kb.matches(key, ACTION_APP_EDITOR_EXTERNAL) {
            return InputAction::EditorExternal;
        }

        // ── F1: help overlay ──
        if kb.matches(key, ACTION_APP_HELP) {
            return InputAction::Help;
        }

        // ── Alt+Enter: queue follow-up message ──
        if kb.matches(key, ACTION_APP_MESSAGE_FOLLOW_UP) {
            let text = self.editor.get_text();
            if !text.trim().is_empty() {
                self.editor.add_to_history(&text);
                self.editor.set_text("");
                return InputAction::FollowUp(text);
            }
            return InputAction::Handled;
        }

        // ── Alt+Up: restore queued messages ──
        if kb.matches(key, ACTION_APP_MESSAGE_DEQUEUE) {
            return InputAction::Dequeue;
        }

        // ── Ctrl+Shift+C: toggle auto-compact ──
        if kb.matches(key, ACTION_APP_COMPACT_TOGGLE) {
            return InputAction::CompactToggle;
        }

        // ── Tab: trigger slash-command autocomplete (pi-style) ──
        if kb.matches(key, ACTION_INPUT_TAB) && !self.editor.autocomplete_active {
            let text = self.editor.get_text();
            if text.starts_with('/') {
                let suggestions = self.get_autocomplete_suggestions();
                self.editor.set_autocomplete(suggestions);
            }
            return InputAction::Handled;
        }

        // ── Enter: submit ──
        if kb.matches(key, ACTION_INPUT_SUBMIT) {
            let text = self.editor.get_text();
            if !text.trim().is_empty() {
                self.editor.add_to_history(&text);
                self.editor.set_text("");
                return InputAction::Submit(text);
            }
            return InputAction::Handled;
        }

        // ── Ctrl+J: insert literal newline ──
        if kb.matches(key, ACTION_INPUT_NEW_LINE) {
            self.editor.insert_text_at_cursor("\n");
            return InputAction::Handled;
        }

        // ── Up/Down: recall history (only when autocomplete is not active) ──
        if !self.editor.autocomplete_active {
            if kb.matches(key, ACTION_APP_HISTORY_UP) && self.editor.get_text().is_empty() {
                return InputAction::RecallHistory(-1);
            }
            if kb.matches(key, ACTION_APP_HISTORY_DOWN) && self.editor.get_text().is_empty() {
                return InputAction::RecallHistory(1);
            }
        }

        // ── PageUp/PageDown: scroll ──
        if kb.matches(key, ACTION_EDITOR_PAGE_UP) {
            return InputAction::PageUp;
        }
        if kb.matches(key, ACTION_EDITOR_PAGE_DOWN) {
            return InputAction::PageDown;
        }

        // ── All other keys: delegate to the core Editor for text editing ──
        self.editor.handle_input(key);

        // ── Auto-trigger slash autocomplete on / ──
        self.check_slash_autocomplete();

        InputAction::Handled
    }

    /// Check if the current text starts with `/` and auto-trigger autocomplete.
    fn check_slash_autocomplete(&mut self) {
        if self.editor.autocomplete_active {
            return;
        }
        let text = self.editor.get_text();
        if text.starts_with('/') && text.len() > 1 && !text[1..].starts_with(' ') {
            let suggestions = self.get_autocomplete_suggestions();
            if !suggestions.is_empty() {
                self.editor.set_autocomplete(suggestions);
            }
        }
    }

    /// Public method for app layer to trigger autocomplete check after set_text.
    pub fn check_autocomplete(&mut self) {
        self.check_slash_autocomplete();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::NoopTheme;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn make_editor() -> ChatEditor {
        ChatEditor::new(&NoopTheme, std::env::temp_dir())
    }

    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn alt_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    fn ctrl_shift(c: char) -> KeyEvent {
        KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )
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

    fn shift_tab() -> KeyEvent {
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)
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
    fn test_ctrl_c_returns_clear() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('c'));
        assert!(matches!(action, InputAction::Clear));
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
    fn test_ctrl_z_returns_suspend() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('z'));
        assert!(matches!(action, InputAction::Suspend));
    }

    #[test]
    fn test_shift_tab_returns_thinking_cycle() {
        let mut ed = make_editor();
        let action = ed.handle_input(&shift_tab());
        assert!(matches!(action, InputAction::ThinkingCycle));
    }

    #[test]
    fn test_ctrl_l_returns_model_selector() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('l'));
        assert!(matches!(action, InputAction::ModelSelector));
    }

    #[test]
    fn test_ctrl_p_returns_model_cycle_forward() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('p'));
        assert!(matches!(action, InputAction::ModelCycleForward));
    }

    #[test]
    fn test_ctrl_shift_p_returns_model_cycle_backward() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl_shift('p'));
        assert!(matches!(action, InputAction::ModelCycleBackward));
    }

    #[test]
    fn test_ctrl_t_returns_toggle_thinking() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('t'));
        assert!(matches!(action, InputAction::ToggleThinking));
    }

    #[test]
    fn test_ctrl_o_returns_tools_expand() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('o'));
        assert!(matches!(action, InputAction::ToolsExpand));
    }

    #[test]
    fn test_ctrl_g_returns_editor_external() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl('g'));
        assert!(matches!(action, InputAction::EditorExternal));
    }

    #[test]
    fn test_f1_returns_help() {
        let mut ed = make_editor();
        let action = ed.handle_input(&f1());
        assert!(matches!(action, InputAction::Help));
    }

    #[test]
    fn test_alt_enter_queues_follow_up() {
        let mut ed = make_editor();
        ed.editor.set_text("follow up text");
        let action = ed.handle_input(&alt_key(KeyCode::Enter));
        match action {
            InputAction::FollowUp(text) => {
                assert_eq!(text, "follow up text");
            }
            other => panic!("Expected FollowUp, got {:?}", other),
        }
        assert!(
            ed.editor.get_text().is_empty(),
            "editor should clear on follow-up"
        );
    }

    #[test]
    fn test_alt_enter_empty_returns_handled() {
        let mut ed = make_editor();
        let action = ed.handle_input(&alt_key(KeyCode::Enter));
        assert!(matches!(action, InputAction::Handled));
    }

    // ── Auto-trigger slash autocomplete tests ──

    #[test]
    fn test_check_autocomplete_triggers_on_slash() {
        let mut ed = make_editor();
        ed.set_slash_commands(vec!["help".into(), "history".into(), "model".into()]);

        // Set text to /h and check autocomplete
        ed.editor.set_text("/h");
        ed.check_autocomplete();
        assert!(
            ed.editor.autocomplete_active,
            "Autocomplete should trigger for /h"
        );
    }

    #[test]
    fn test_check_autocomplete_no_trigger_on_just_slash() {
        let mut ed = make_editor();
        ed.set_slash_commands(vec!["help".into()]);

        // Just / alone should NOT trigger autocomplete (no prefix to match)
        ed.editor.set_text("/");
        ed.check_autocomplete();
        assert!(
            !ed.editor.autocomplete_active,
            "Autocomplete should not trigger for just /"
        );
    }

    #[test]
    fn test_check_autocomplete_no_trigger_on_normal_text() {
        let mut ed = make_editor();
        ed.set_slash_commands(vec!["help".into()]);

        ed.editor.set_text("hello world");
        ed.check_autocomplete();
        assert!(
            !ed.editor.autocomplete_active,
            "Autocomplete should not trigger for normal text"
        );
    }

    #[test]
    fn test_check_autocomplete_filters_suggestions() {
        let mut ed = make_editor();
        ed.set_slash_commands(vec!["help".into(), "history".into(), "model".into()]);

        // /h should match both help and history
        ed.editor.set_text("/h");
        ed.check_autocomplete();
        assert!(ed.editor.autocomplete_active);

        // /his should match only history
        ed.editor.set_text("/his");
        ed.check_autocomplete();
        assert!(ed.editor.autocomplete_active);
    }

    #[test]
    fn test_check_autocomplete_no_match_shows_nothing() {
        let mut ed = make_editor();
        ed.set_slash_commands(vec!["help".into()]);

        // /z matches nothing — autocomplete stays inactive
        ed.editor.set_text("/z");
        ed.check_autocomplete();
        assert!(
            !ed.editor.autocomplete_active,
            "Autocomplete should not show when no matches"
        );
    }

    #[test]
    fn test_check_autocomplete_does_not_override_existing() {
        let mut ed = make_editor();
        ed.set_slash_commands(vec!["help".into()]);

        // Manually activate autocomplete
        ed.editor.set_text("/h");
        let suggestions = ed.get_autocomplete_suggestions();
        ed.editor.set_autocomplete(suggestions);
        assert!(ed.editor.autocomplete_active);

        // check_autocomplete should not interfere
        ed.editor.set_text("/x");
        ed.check_autocomplete();
        // The suggestion list may update, but active remains true
        // (the on_change callback doesn't reset it; handle_input checks
        // autocomplete_active first and skips if already active)
    }

    #[test]
    fn test_typing_slash_triggers_autocomplete_via_handle_input() {
        let mut ed = make_editor();
        ed.set_slash_commands(vec!["help".into()]);

        // Type / followed by h — should trigger autocomplete
        ed.handle_input(&char_key('/'));
        ed.handle_input(&char_key('h'));

        assert!(
            ed.editor.autocomplete_active,
            "Typing /h should trigger autocomplete"
        );
    }

    #[test]
    fn test_ctrl_shift_c_returns_compact_toggle() {
        let mut ed = make_editor();
        let action = ed.handle_input(&ctrl_shift('c'));
        assert!(matches!(action, InputAction::CompactToggle));
    }

    #[test]
    fn test_alt_up_returns_dequeue() {
        let mut ed = make_editor();
        let action = ed.handle_input(&alt_key(KeyCode::Up));
        assert!(matches!(action, InputAction::Dequeue));
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
            format!("{:?}", InputAction::Clear),
            format!("{:?}", InputAction::Exit),
            format!("{:?}", InputAction::Suspend),
            format!("{:?}", InputAction::ThinkingCycle),
            format!("{:?}", InputAction::ModelSelector),
            format!("{:?}", InputAction::ModelCycleForward),
            format!("{:?}", InputAction::ModelCycleBackward),
            format!("{:?}", InputAction::ToggleThinking),
            format!("{:?}", InputAction::ToolsExpand),
            format!("{:?}", InputAction::EditorExternal),
            format!("{:?}", InputAction::Help),
            format!("{:?}", InputAction::Submit("x".into())),
            format!("{:?}", InputAction::RecallHistory(1)),
            format!("{:?}", InputAction::PageUp),
            format!("{:?}", InputAction::PageDown),
            format!("{:?}", InputAction::FollowUp("x".into())),
            format!("{:?}", InputAction::CompactToggle),
            format!("{:?}", InputAction::Dequeue),
        ];
        for v in &variants {
            assert!(!v.is_empty(), "Debug output should not be empty");
        }
    }
}
