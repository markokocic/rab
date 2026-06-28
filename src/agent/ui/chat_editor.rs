use std::collections::HashMap;

use crossterm::event::KeyEvent;

use crate::agent::ui::theme::ThemeKey;
use crate::tui::Component;
use crate::tui::Theme;
use crate::tui::autocomplete::{CombinedAutocompleteProvider, SlashCommand};
use crate::tui::components::Editor;
use crate::tui::components::editor::EditorOptions;
use crate::tui::keybindings::{
    ACTION_APP_CLEAR, ACTION_APP_COMPACT_TOGGLE, ACTION_APP_EDITOR_EXTERNAL, ACTION_APP_ESCAPE,
    ACTION_APP_EXIT, ACTION_APP_HELP, ACTION_APP_MESSAGE_DEQUEUE, ACTION_APP_MESSAGE_FOLLOW_UP,
    ACTION_APP_MODEL_CYCLE_BACKWARD, ACTION_APP_MODEL_CYCLE_FORWARD, ACTION_APP_MODEL_SELECTOR,
    ACTION_APP_THINKING_CYCLE, ACTION_APP_TOGGLE_THINKING, ACTION_APP_TOOLS_EXPAND,
    ACTION_INPUT_SUBMIT, ACTION_SELECT_CANCEL, get_keybindings,
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
    /// Alt+Enter pressed (app should queue follow-up message)
    FollowUp(String),
    /// Alt+Up pressed (app should restore queued message back to editor)
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
///
/// Key differences from pi's CustomEditor:
/// - Text-editing keys (Ctrl+Z undo, Ctrl+J newline, Up/Down history, Tab,
///   PageUp/PageDown, etc.) are delegated entirely to the inner Editor,
///   matching pi's editor-centric handling.
/// - Only app-level keybindings (interrupt, exit, model selector, help, etc.)
///   are intercepted here.
#[allow(clippy::type_complexity)]
pub struct ChatEditor {
    pub editor: Editor,
    /// Working directory for file path completion in autocomplete provider.
    cwd: std::path::PathBuf,
    /// Slash commands for the autocomplete provider.
    slash_commands: Vec<SlashCommand>,
    /// Extension-registered shortcuts (pi-style). Checked before built-in handling.
    on_extension_shortcut: Option<Box<dyn FnMut(&KeyEvent) -> bool + Send>>,
    /// Dynamically registered app action handlers (pi-style).
    action_handlers: HashMap<String, Box<dyn FnMut() + Send>>,
}

impl ChatEditor {
    pub fn new(_theme: &dyn Theme, cwd: std::path::PathBuf) -> Self {
        let editor = Editor::new(EditorOptions { padding_x: 0 });

        Self {
            editor,
            cwd,
            slash_commands: Vec::new(),
            on_extension_shortcut: None,
            action_handlers: HashMap::new(),
        }
    }

    /// Build and set the autocomplete provider from the current slash commands and cwd.
    fn rebuild_autocomplete_provider(&mut self) {
        let provider = CombinedAutocompleteProvider::new(
            self.slash_commands.clone(),
            self.cwd.to_string_lossy().to_string(),
        );
        self.editor.set_autocomplete_provider(Box::new(provider));
    }

    /// Set the available slash commands for autocomplete.
    pub fn set_slash_commands(&mut self, commands: Vec<SlashCommand>) {
        self.slash_commands = commands;
        self.rebuild_autocomplete_provider();
    }

    /// Set a handler for extension-registered shortcuts (pi-style).
    /// The handler receives the key event and returns true if the key was handled.
    pub fn set_extension_shortcut_handler(
        &mut self,
        handler: Box<dyn FnMut(&KeyEvent) -> bool + Send>,
    ) {
        self.on_extension_shortcut = Some(handler);
    }

    /// Register a dynamic app action handler (pi-style).
    /// When the keybinding for `action` is pressed, `handler` is called.
    /// Does NOT override built-in actions (Escape, Ctrl+C, Ctrl+D, Enter).
    pub fn on_action(&mut self, action: &str, handler: Box<dyn FnMut() + Send>) {
        self.action_handlers.insert(action.to_string(), handler);
    }

    /// After programmatic set_text, trigger autocomplete check (pi-style).
    pub fn check_autocomplete(&mut self) {
        // The inner Editor's autocomplete provider auto-triggers on typing,
        // but after set_text we force a check so slash commands show immediately.
        if !self.editor.autocomplete_active {
            self.editor.try_trigger_autocomplete();
        }
    }

    /// Update the working directory.
    pub fn set_cwd(&mut self, cwd: std::path::PathBuf) {
        self.cwd = cwd;
        self.rebuild_autocomplete_provider();
    }

    /// Handle keyboard input. Mirrors pi's CustomEditor.handleInput:
    ///
    /// 1. Checks app-level keys (escape, clear, submit, model selector, etc.)
    ///    and returns the corresponding InputAction for the app layer to handle.
    /// 2. All other keys are delegated to the inner Editor for text editing,
    ///    including Ctrl+Z (undo), Ctrl+J (newline), Up/Down (history),
    ///    Tab (autocomplete), PageUp/PageDown (scroll), etc.
    ///
    /// This keeps app-level side effects (aborting agent, opening overlays, etc.)
    /// in the app layer while keeping text-editing logic in the Editor component.
    /// Update editor border color based on thinking level or bash mode.
    /// Matches pi's `updateEditorBorderColor()`.
    /// - Bash mode (text starts with `!`): uses `bashMode` color
    /// - Otherwise: uses thinking level color (`thinkingOff`..`thinkingXhigh`)
    pub fn update_border_color(
        &mut self,
        thinking_level: Option<&str>,
        theme: &dyn crate::tui::Theme,
    ) {
        let text = self.editor.get_text();
        if text.trim_start().starts_with('!') {
            let ansi = theme.fg_key(ThemeKey::BashMode, "").to_string();
            // Extract just the ANSI prefix (before any text).
            // Use find('m') to get only the color-set code, not the trailing reset.
            // theme.fg() returns "\x1b[...m\x1b[39m"; we want "\x1b[...m" only.
            let prefix = if ansi.starts_with('\x1b') {
                let end = ansi.find('m').unwrap_or(ansi.len());
                ansi[..end + 1].to_string()
            } else {
                ansi
            };
            self.editor.border_color = crate::tui::Style::new().fg(prefix);
        } else {
            let level = thinking_level.unwrap_or("off");
            let color_name = match level {
                "off" => "thinkingOff",
                "minimal" => "thinkingMinimal",
                "low" => "thinkingLow",
                "medium" => "thinkingMedium",
                "high" => "thinkingHigh",
                "xhigh" | "max" => "thinkingXhigh",
                _ => "thinkingOff",
            };
            let ansi = theme.fg(color_name, "").to_string();
            let prefix = if ansi.starts_with('\x1b') {
                let end = ansi.find('m').unwrap_or(ansi.len());
                ansi[..end + 1].to_string()
            } else {
                ansi
            };
            self.editor.border_color = crate::tui::Style::new().fg(prefix);
        }
    }

    pub fn handle_input(&mut self, key: &KeyEvent) -> InputAction {
        let kb = get_keybindings();

        // ═══════════════════════════════════════════════════════════════════
        // 1. Extension shortcuts (pi-style: checked before built-in handling)
        // ═══════════════════════════════════════════════════════════════════
        if let Some(ref mut handler) = self.on_extension_shortcut
            && handler(key)
        {
            return InputAction::Handled;
        }

        // ═══════════════════════════════════════════════════════════════════
        // 2. Built-in app-level actions (hardcoded, matching pi's CustomEditor)
        // ═══════════════════════════════════════════════════════════════════

        // ── Escape: close autocomplete first if active, else signal app ──
        // Mirrors pi: if autocomplete is active, let Editor handle it (cancels autocomplete).
        if kb.matches(key, ACTION_SELECT_CANCEL) || kb.matches(key, ACTION_APP_ESCAPE) {
            if self.editor.autocomplete_active {
                self.editor.handle_input(key);
                return InputAction::Handled;
            }
            return InputAction::Escape;
        }

        // ── Ctrl+C: clear (abort streaming or clear editor) ──
        if kb.matches(key, ACTION_APP_CLEAR) {
            return InputAction::Clear;
        }

        // ── Ctrl+D: exit when editor is empty, else let Editor handle as delete-forward ──
        if kb.matches(key, ACTION_APP_EXIT) {
            if self.editor.get_text().is_empty() {
                return InputAction::Exit;
            }
            // Fall through so the Editor handles Ctrl+D as deleteCharForward
            self.editor.handle_input(key);
            return InputAction::Handled;
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

        // ── Alt+Up: restore queued message back to editor (dequeue) ──
        if kb.matches(key, ACTION_APP_MESSAGE_DEQUEUE) {
            return InputAction::Dequeue;
        }

        // ── Ctrl+Shift+C: toggle auto-compact ──
        if kb.matches(key, ACTION_APP_COMPACT_TOGGLE) {
            return InputAction::CompactToggle;
        }

        // ═══════════════════════════════════════════════════════════════════
        // 3. Dynamically registered app actions (pi-style actionHandlers)
        // ═══════════════════════════════════════════════════════════════════
        // Matches pi: checked after built-ins, before Editor delegation.
        // Excludes app.interrupt and app.exit which are handled above.
        for (action, handler) in &mut self.action_handlers {
            if action != "app.interrupt" && action != "app.exit" && kb.matches(key, action) {
                handler();
                return InputAction::Handled;
            }
        }

        // ═══════════════════════════════════════════════════════════════════
        // 4. Enter: let Editor handle submit (pi-style)
        // ═══════════════════════════════════════════════════════════════════
        // The Editor's handle_input processes Enter via submit(), which:
        //   1. Expands paste markers
        //   2. Stores the text in last_submitted_text
        //   3. Clears editor state (pastes, undo, history browsing)
        //   4. Calls on_submit callback
        //   5. Sets just_submitted flag
        //
        // We check just_submitted after handle_input to detect submission.
        // For slash command completion via Enter, the completion is applied
        // inside handle_input before submit() is called, so last_submitted_text
        // contains the completed command text.
        if kb.matches(key, ACTION_INPUT_SUBMIT) {
            self.editor.just_submitted = false;
            self.editor.handle_input(key);
            if self.editor.just_submitted {
                // Editor processed the submit - use last_submitted_text (captured
                // before clearing) so slash command completions are included.
                let text = self.editor.last_submitted_text.clone();
                let has_content = !text.trim().is_empty();
                if has_content {
                    self.editor.add_to_history(&text);
                }
                return InputAction::Submit(text);
            }
            return InputAction::Handled;
        }

        // ═══════════════════════════════════════════════════════════════════
        // 5. All other keys: delegate to the core Editor for text editing
        // ═══════════════════════════════════════════════════════════════════
        // This includes:
        //   - Ctrl+Z → undo (ACTION_EDITOR_UNDO)
        //   - Ctrl+J → newline (ACTION_INPUT_NEW_LINE)
        //   - Up/Down → cursor + history (ACTION_EDITOR_CURSOR_UP/DOWN)
        //   - Tab → autocomplete (ACTION_INPUT_TAB)
        //   - PageUp/PageDown → scroll (ACTION_EDITOR_PAGE_UP/DOWN)
        //   - All printable chars, movement, deletion, kill/yank, etc.
        self.editor.just_submitted = false;
        self.editor.handle_input(key);

        InputAction::Handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::autocomplete::SlashCommand;
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

    fn up() -> KeyEvent {
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)
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
        ed.set_slash_commands(vec![SlashCommand {
            name: "help".into(),
            description: None,
            argument_hint: None,
            argument_completions: None,
            get_argument_completions: None,
        }]);
        ed.editor.set_text("/");
        // Trigger autocomplete via the provider (Tab is handled by Editor now)
        ed.editor.handle_input(&ctrl('l')); // not helpful, just press a key
        // Manually trigger autocomplete by typing a letter
        ed.editor.set_text("/h");
        // The inner Editor should have triggered autocomplete via the provider
        // when /h was typed and the auto-trigger ran on 'h'
        // But try_trigger_autocomplete is called from insert_character -> check_autocomplete_trigger
        // which only fires on certain chars. Let's just set autocomplete directly.
        let suggestions = vec![crate::tui::components::select_list::SelectItem::new(
            "help", "help",
        )];
        ed.editor.set_autocomplete(suggestions);
        assert!(
            ed.editor.autocomplete_active,
            "autocomplete should be active"
        );

        // Escape should close it - now handled by Editor (fallthrough)
        let _action = ed.handle_input(&escape());
        assert!(matches!(_action, InputAction::Handled));
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
    fn test_ctrl_d_with_text_deletes_forward() {
        let mut ed = make_editor();
        ed.editor.set_text("hello");
        // Cursor is at end by default, move to start
        for _ in 0..5 {
            ed.editor
                .handle_input(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        assert_eq!(ed.editor.get_cursor(), (0, 0));
        let action = ed.handle_input(&ctrl('d'));
        // Should be Handled (Editor handles Ctrl+D as delete_forward)
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "ello");
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
    fn test_enter_with_empty_text_returns_submit_empty() {
        let mut ed = make_editor();
        let action = ed.handle_input(&enter());
        // Pi: Editor.submitValue() always calls onSubmit, even with empty text
        match action {
            InputAction::Submit(text) => {
                assert_eq!(text, "", "empty submit should return empty string");
            }
            other => panic!("Expected Submit(\"\"), got {:?}", other),
        }
    }

    // ── Pi-compat: text editing keys fall through to Editor ──

    #[test]
    fn test_ctrl_z_delegates_to_editor_undo() {
        let mut ed = make_editor();
        ed.editor.set_text("hello");
        // Move cursor to end
        ed.editor.set_text("hello world");
        // Type more, then undo
        ed.editor.handle_input(&char_key('!'));
        assert_eq!(ed.editor.get_text(), "hello world!");
        // Ctrl+Z should undo via Editor (no longer intercepted as Suspend)
        let action = ed.handle_input(&ctrl('z'));
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "hello world");
    }

    #[test]
    fn test_ctrl_j_inserts_newline_via_editor() {
        let mut ed = make_editor();
        ed.editor.set_text("hello");
        // Ctrl+J should add newline via Editor's add_newline()
        let action = ed.handle_input(&ctrl('j'));
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "hello\n");
    }

    #[test]
    fn test_up_down_history_via_editor() {
        let mut ed = make_editor();
        // Add history entries like pi does
        ed.editor.add_to_history("first");
        ed.editor.add_to_history("second");
        assert!(ed.editor.get_text().is_empty());

        // Up should recall via the Editor's internal history (not app-level)
        let action = ed.handle_input(&up());
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "second");
    }

    #[test]
    fn test_page_keys_delegated_to_editor() {
        let mut ed = make_editor();
        // PageUp/PageDown should be handled by Editor, not intercepted
        let action = ed.handle_input(&page_up());
        assert!(matches!(action, InputAction::Handled));
        let action = ed.handle_input(&page_down());
        assert!(matches!(action, InputAction::Handled));
    }

    #[test]
    fn test_tab_delegated_to_editor() {
        let mut ed = make_editor();
        // Set some text with a slash command prefix
        ed.set_slash_commands(vec![
            SlashCommand {
                name: "help".into(),
                description: None,
                argument_hint: None,
                argument_completions: None,
                get_argument_completions: None,
            },
            SlashCommand {
                name: "history".into(),
                description: None,
                argument_hint: None,
                argument_completions: None,
                get_argument_completions: None,
            },
        ]);
        ed.editor.set_text("/h");

        // Tab should be handled by Editor (trigger autocomplete provider)
        let _action = ed.handle_input(&ctrl(' ')); // Not Tab, but another way...
        // Just verify Tab doesn't crash
        let tab_key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let _action = ed.handle_input(&tab_key);
        assert!(matches!(_action, InputAction::Handled));
    }

    // ── Printable chars ──

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
        let action = ed.handle_input(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "ab");
    }

    #[test]
    fn test_arrow_left_moves_cursor() {
        let mut ed = make_editor();
        ed.editor.set_text("abc");
        assert_eq!(ed.editor.get_cursor(), (0, 3));
        let action = ed.handle_input(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_cursor(), (0, 2));
    }

    #[test]
    fn test_ctrl_k_deletes_to_line_end() {
        let mut ed = make_editor();
        ed.editor.set_text("hello world");
        // Move cursor after "hello "
        for _ in 0..6 {
            ed.editor
                .handle_input(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        assert_eq!(ed.editor.get_cursor(), (0, 5));
        let action = ed.handle_input(&ctrl('k'));
        assert!(matches!(action, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "hello");
    }

    // ── History integration ──

    #[test]
    fn test_submit_adds_to_history() {
        let mut ed = make_editor();
        ed.editor.set_text("test");
        let action = ed.handle_input(&enter());
        assert!(matches!(action, InputAction::Submit(_)));
        // History should now contain "test" - verify by pressing Up
        let action2 = ed.handle_input(&up());
        assert!(matches!(action2, InputAction::Handled));
        assert_eq!(ed.editor.get_text(), "test");
    }

    // ── InputAction enum exhaustiveness ──

    #[test]
    fn test_input_action_debug() {
        let variants = vec![
            format!("{:?}", InputAction::Handled),
            format!("{:?}", InputAction::Escape),
            format!("{:?}", InputAction::Clear),
            format!("{:?}", InputAction::Exit),
            format!("{:?}", InputAction::ThinkingCycle),
            format!("{:?}", InputAction::ModelSelector),
            format!("{:?}", InputAction::ModelCycleForward),
            format!("{:?}", InputAction::ModelCycleBackward),
            format!("{:?}", InputAction::ToggleThinking),
            format!("{:?}", InputAction::ToolsExpand),
            format!("{:?}", InputAction::EditorExternal),
            format!("{:?}", InputAction::Help),
            format!("{:?}", InputAction::Submit("x".into())),
            format!("{:?}", InputAction::FollowUp("x".into())),
            format!("{:?}", InputAction::CompactToggle),
            format!("{:?}", InputAction::Dequeue),
        ];
        for v in &variants {
            assert!(!v.is_empty(), "Debug output should not be empty");
        }
    }
}
