use std::collections::HashMap;
use std::sync::OnceLock;

use crossterm::event::KeyEvent;

use crate::tui::keys::match_key_id;

// =============================================================================
// Keybinding action identifiers — matching pi's TUI_KEYBINDINGS
// =============================================================================

// ── Editor actions ──
pub const ACTION_EDITOR_CURSOR_LEFT: &str = "tui.editor.cursorLeft";
pub const ACTION_EDITOR_CURSOR_RIGHT: &str = "tui.editor.cursorRight";
pub const ACTION_EDITOR_CURSOR_UP: &str = "tui.editor.cursorUp";
pub const ACTION_EDITOR_CURSOR_DOWN: &str = "tui.editor.cursorDown";
pub const ACTION_EDITOR_CURSOR_LINE_START: &str = "tui.editor.cursorLineStart";
pub const ACTION_EDITOR_CURSOR_LINE_END: &str = "tui.editor.cursorLineEnd";
pub const ACTION_EDITOR_CURSOR_WORD_LEFT: &str = "tui.editor.cursorWordLeft";
pub const ACTION_EDITOR_CURSOR_WORD_RIGHT: &str = "tui.editor.cursorWordRight";
pub const ACTION_EDITOR_DELETE_CHAR_BACKWARD: &str = "tui.editor.deleteCharBackward";
pub const ACTION_EDITOR_DELETE_CHAR_FORWARD: &str = "tui.editor.deleteCharForward";
pub const ACTION_EDITOR_DELETE_WORD_BACKWARD: &str = "tui.editor.deleteWordBackward";
pub const ACTION_EDITOR_DELETE_WORD_FORWARD: &str = "tui.editor.deleteWordForward";
pub const ACTION_EDITOR_DELETE_TO_LINE_START: &str = "tui.editor.deleteToLineStart";
pub const ACTION_EDITOR_DELETE_TO_LINE_END: &str = "tui.editor.deleteToLineEnd";
pub const ACTION_EDITOR_YANK: &str = "tui.editor.yank";
pub const ACTION_EDITOR_YANK_POP: &str = "tui.editor.yankPop";
pub const ACTION_EDITOR_UNDO: &str = "tui.editor.undo";
pub const ACTION_EDITOR_PAGE_UP: &str = "tui.editor.pageUp";
pub const ACTION_EDITOR_PAGE_DOWN: &str = "tui.editor.pageDown";

// ── Input (single-line) actions ──
pub const ACTION_INPUT_SUBMIT: &str = "tui.input.submit";
pub const ACTION_INPUT_TAB: &str = "tui.input.tab";
pub const ACTION_INPUT_NEW_LINE: &str = "tui.input.newLine";
pub const ACTION_INPUT_COPY: &str = "tui.input.copy";

// ── Select list actions ──
pub const ACTION_SELECT_UP: &str = "tui.select.up";
pub const ACTION_SELECT_DOWN: &str = "tui.select.down";
pub const ACTION_SELECT_CONFIRM: &str = "tui.select.confirm";
pub const ACTION_SELECT_CANCEL: &str = "tui.select.cancel";

// ── Application-level actions (rab-specific, not in pi) ──
pub const ACTION_APP_ESCAPE: &str = "app.escape";
pub const ACTION_APP_INTERRUPT: &str = "app.interrupt";
pub const ACTION_APP_EXIT: &str = "app.exit";
pub const ACTION_APP_MODEL_SELECTOR: &str = "app.modelSelector";
pub const ACTION_APP_TOGGLE_THINKING: &str = "app.toggleThinking";
pub const ACTION_APP_TOGGLE_COLLAPSE: &str = "app.toggleCollapse";
pub const ACTION_APP_HELP: &str = "app.help";
pub const ACTION_APP_HISTORY_UP: &str = "app.historyUp";
pub const ACTION_APP_HISTORY_DOWN: &str = "app.historyDown";

// =============================================================================
// Keybindings
// =============================================================================

/// Mapping from action ID to list of key IDs that trigger it.
#[derive(Debug, Clone)]
pub struct Keybindings {
    bindings: HashMap<String, Vec<String>>,
}

impl Keybindings {
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Create keybindings from default pi-compatible bindings.
    pub fn with_defaults() -> Self {
        let mut kb = Self::new();
        kb.set_defaults();
        kb
    }

    fn set_defaults(&mut self) {
        self.set(ACTION_EDITOR_CURSOR_LEFT, vec!["left".into(), "ctrl+b".into()]);
        self.set(ACTION_EDITOR_CURSOR_RIGHT, vec!["right".into(), "ctrl+f".into()]);
        self.set(ACTION_EDITOR_CURSOR_UP, vec!["up".into()]);
        self.set(ACTION_EDITOR_CURSOR_DOWN, vec!["down".into()]);
        self.set(ACTION_EDITOR_CURSOR_LINE_START, vec!["home".into(), "ctrl+a".into()]);
        self.set(ACTION_EDITOR_CURSOR_LINE_END, vec!["end".into(), "ctrl+e".into()]);
        self.set(ACTION_EDITOR_CURSOR_WORD_LEFT, vec!["ctrl+left".into(), "alt+b".into()]);
        self.set(ACTION_EDITOR_CURSOR_WORD_RIGHT, vec!["ctrl+right".into(), "alt+f".into()]);
        self.set(ACTION_EDITOR_DELETE_CHAR_BACKWARD, vec!["backspace".into(), "ctrl+h".into()]);
        self.set(ACTION_EDITOR_DELETE_CHAR_FORWARD, vec!["delete".into(), "ctrl+d".into()]);
        self.set(ACTION_EDITOR_DELETE_WORD_BACKWARD, vec!["ctrl+w".into()]);
        self.set(ACTION_EDITOR_DELETE_WORD_FORWARD, vec!["alt+d".into()]);
        self.set(ACTION_EDITOR_DELETE_TO_LINE_START, vec!["ctrl+u".into()]);
        self.set(ACTION_EDITOR_DELETE_TO_LINE_END, vec!["ctrl+k".into()]);
        self.set(ACTION_EDITOR_YANK, vec!["ctrl+y".into()]);
        self.set(ACTION_EDITOR_YANK_POP, vec!["alt+y".into()]);
        self.set(ACTION_EDITOR_UNDO, vec!["ctrl+z".into()]);
        self.set(ACTION_EDITOR_PAGE_UP, vec!["pageUp".into()]);
        self.set(ACTION_EDITOR_PAGE_DOWN, vec!["pageDown".into()]);

        self.set(ACTION_INPUT_SUBMIT, vec!["enter".into()]);
        self.set(ACTION_INPUT_TAB, vec!["tab".into()]);
        self.set(ACTION_INPUT_NEW_LINE, vec!["ctrl+j".into()]);
        self.set(ACTION_INPUT_COPY, vec!["ctrl+c".into()]);

        self.set(ACTION_SELECT_UP, vec!["up".into()]);
        self.set(ACTION_SELECT_DOWN, vec!["down".into()]);
        self.set(ACTION_SELECT_CONFIRM, vec!["enter".into()]);
        self.set(ACTION_SELECT_CANCEL, vec!["escape".into()]);

        self.set(ACTION_APP_ESCAPE, vec!["escape".into()]);
        self.set(ACTION_APP_INTERRUPT, vec!["ctrl+c".into()]);
        self.set(ACTION_APP_EXIT, vec!["ctrl+d".into()]);
        self.set(ACTION_APP_MODEL_SELECTOR, vec!["ctrl+l".into()]);
        self.set(ACTION_APP_TOGGLE_THINKING, vec!["ctrl+t".into()]);
        self.set(ACTION_APP_TOGGLE_COLLAPSE, vec!["ctrl+o".into()]);
        self.set(ACTION_APP_HELP, vec!["f1".into()]);
        self.set(ACTION_APP_HISTORY_UP, vec!["up".into()]);
        self.set(ACTION_APP_HISTORY_DOWN, vec!["down".into()]);
    }

    /// Set the key IDs for an action.
    pub fn set(&mut self, action: &str, keys: Vec<String>) {
        self.bindings.insert(action.to_string(), keys);
    }

    /// Merge another keybindings into this one (overwrites existing).
    pub fn merge(&mut self, other: Keybindings) {
        for (action, keys) in other.bindings {
            self.bindings.insert(action, keys);
        }
    }

    /// Check if a key event matches any of the keys bound to an action.
    pub fn matches(&self, event: &KeyEvent, action_id: &str) -> bool {
        if let Some(keys) = self.bindings.get(action_id) {
            for key_id in keys {
                if match_key_id(event, key_id) {
                    return true;
                }
            }
        }
        false
    }

    /// Get the key IDs bound to an action.
    pub fn get_keys(&self, action_id: &str) -> &[String] {
        self.bindings
            .get(action_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Load keybindings from a JSON file.
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let bindings: HashMap<String, Vec<String>> =
            serde_json::from_str(&content).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Self { bindings })
    }

    /// Save keybindings to a JSON file.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let content = serde_json::to_string_pretty(&self.bindings)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, content)
    }
}

impl Default for Keybindings {
    fn default() -> Self {
        Self::with_defaults()
    }
}

// =============================================================================
// Global keybindings accessor
// =============================================================================

static GLOBAL_KEYBINDINGS: OnceLock<Keybindings> = OnceLock::new();

/// Get the global keybindings instance (initialized with defaults on first call).
pub fn get_keybindings() -> &'static Keybindings {
    GLOBAL_KEYBINDINGS.get_or_init(Keybindings::with_defaults)
}

/// Initialize (or replace) the global keybindings.
pub fn init_keybindings(kb: Keybindings) {
    let _ = GLOBAL_KEYBINDINGS.set(kb);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_defaults_loaded() {
        let kb = get_keybindings();
        assert!(kb.matches(
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            ACTION_INPUT_COPY,
        ));
        assert!(!kb.matches(
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            ACTION_INPUT_COPY,
        ));
    }

    #[test]
    fn test_editor_undo() {
        let kb = get_keybindings();
        assert!(kb.matches(
            &KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
            ACTION_EDITOR_UNDO,
        ));
    }

    #[test]
    fn test_select_up_down() {
        let kb = get_keybindings();
        assert!(kb.matches(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), ACTION_SELECT_UP));
        assert!(kb.matches(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), ACTION_SELECT_DOWN));
        assert!(kb.matches(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), ACTION_SELECT_CONFIRM));
        assert!(kb.matches(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), ACTION_SELECT_CANCEL));
    }

    #[test]
    fn test_delete_word_backward() {
        let kb = get_keybindings();
        assert!(kb.matches(
            &KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            ACTION_EDITOR_DELETE_WORD_BACKWARD,
        ));
    }

    #[test]
    fn test_cursor_word_left() {
        let kb = get_keybindings();
        assert!(kb.matches(
            &KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
            ACTION_EDITOR_CURSOR_WORD_LEFT,
        ));
        assert!(kb.matches(
            &KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
            ACTION_EDITOR_CURSOR_WORD_LEFT,
        ));
    }
}
