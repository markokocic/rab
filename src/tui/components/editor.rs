#![allow(clippy::type_complexity)]

use crate::tui::keybindings::{
    get_keybindings, ACTION_EDITOR_CURSOR_DOWN, ACTION_EDITOR_CURSOR_LEFT,
    ACTION_EDITOR_CURSOR_LINE_END, ACTION_EDITOR_CURSOR_LINE_START,
    ACTION_EDITOR_CURSOR_RIGHT, ACTION_EDITOR_CURSOR_UP, ACTION_EDITOR_CURSOR_WORD_LEFT,
    ACTION_EDITOR_CURSOR_WORD_RIGHT, ACTION_EDITOR_DELETE_CHAR_BACKWARD,
    ACTION_EDITOR_DELETE_CHAR_FORWARD, ACTION_EDITOR_DELETE_TO_LINE_END,
    ACTION_EDITOR_DELETE_TO_LINE_START, ACTION_EDITOR_DELETE_WORD_BACKWARD,
    ACTION_EDITOR_DELETE_WORD_FORWARD, ACTION_EDITOR_PAGE_DOWN, ACTION_EDITOR_PAGE_UP,
    ACTION_EDITOR_UNDO, ACTION_EDITOR_YANK, ACTION_EDITOR_YANK_POP, ACTION_INPUT_NEW_LINE,
    ACTION_INPUT_SUBMIT, ACTION_INPUT_TAB, ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM,
    ACTION_SELECT_DOWN, ACTION_SELECT_UP, ACTION_EDITOR_JUMP_FORWARD,
    ACTION_EDITOR_JUMP_BACKWARD,
};
use crate::tui::autocomplete::AutocompleteProvider;
use crate::tui::component::Component;
use crate::tui::components::select_list::{SelectItem, SelectList, SelectListTheme};
use crate::tui::util::is_whitespace_char;
use crate::tui::focusable::{CURSOR_MARKER, Focusable};
use crate::tui::keys::key_event_to_string;
use crate::tui::kill_ring::KillRing;
use std::collections::HashMap;

use crate::tui::undo_stack::UndoStack;
use crate::tui::util::{visible_width, visual_col_to_byte_offset, wrap_text_with_ansi};
use crate::tui::word_nav::{find_word_backward, find_word_forward};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

/// Theme for the Editor component.
pub struct EditorTheme {
    pub text: Box<dyn Fn(&str) -> String>,
    pub cursor: Box<dyn Fn(&str) -> String>,
    pub border: Box<dyn Fn(&str) -> String>,
    pub scroll_indicator: Box<dyn Fn(&str) -> String>,
    pub autocomplete_selected: Box<dyn Fn(&str) -> String>,
    pub autocomplete_normal: Box<dyn Fn(&str) -> String>,
}

impl Default for EditorTheme {
    fn default() -> Self {
        Self {
            text: Box::new(|s| s.to_string()),
            cursor: Box::new(|s| format!("\x1b[7m{}\x1b[27m", s)),
            border: Box::new(|s| s.to_string()),
            scroll_indicator: Box::new(|s| s.to_string()),
            autocomplete_selected: Box::new(|s| format!("\x1b[7m{}\x1b[27m", s)),
            autocomplete_normal: Box::new(|s| s.to_string()),
        }
    }
}

/// Direction for character jump mode (pi-style).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JumpDirection {
    Forward,
    Backward,
}

pub struct EditorOptions {
    pub padding_x: usize,
    pub max_visible_lines: usize,
}

impl Default for EditorOptions {
    fn default() -> Self {
        Self {
            padding_x: 1,
            max_visible_lines: 10,
        }
    }
}

// ── Editor ─────────────────────────────────────────────────────────

pub struct Editor {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
    padding_x: usize,
    max_visible_lines: usize,
    scroll_offset: usize,
    theme: EditorTheme,
    focused: bool,
    kill_ring: KillRing,
    undo_stack: UndoStack<EditorSnapshot>,
    history: Vec<String>,
    history_index: i32,
    history_draft: Option<EditorSnapshot>,
    preferred_col: Option<usize>,
    last_action: Option<String>,
    pub on_submit: Option<Box<dyn FnMut(String)>>,
    pub on_change: Option<Box<dyn FnMut(&str)>>,
    pub disable_submit: bool,
    pub border_color: Box<dyn Fn(&str) -> String>,

    // Character jump mode (pi-style: await next printable char to jump to)
    jump_mode: Option<JumpDirection>,

    // Pi-style autocomplete provider (handles slash commands, file paths, etc.)
    autocomplete_provider: Option<Box<dyn AutocompleteProvider>>,

    // Pi-style paste markers (large pastes stored, marker inserted in place)
    pastes: HashMap<u32, String>,
    paste_counter: u32,

    // Pi-style autocomplete state (uses SelectList)
    autocomplete_list: Option<SelectList>,
    pub autocomplete_active: bool,
}

#[derive(Debug, Clone)]
struct EditorSnapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

impl Editor {
    pub fn new(theme: EditorTheme, options: EditorOptions) -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            padding_x: options.padding_x,
            max_visible_lines: options.max_visible_lines.max(3),
            scroll_offset: 0,
            theme,
            focused: false,
            kill_ring: KillRing::new(),
            undo_stack: UndoStack::new(),
            history: Vec::new(),
            history_index: -1,
            history_draft: None,
            preferred_col: None,
            last_action: None,
            on_submit: None,
            on_change: None,
            disable_submit: false,
            autocomplete_list: None,
            autocomplete_active: false,
            border_color: Box::new(|s| s.to_string()),
            autocomplete_provider: None,
            pastes: HashMap::new(),
            paste_counter: 0,
            jump_mode: None,
        }
    }

    // ── Public API ──

    pub fn get_text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn get_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn get_cursor(&self) -> (usize, usize) {
        (self.cursor_line, self.cursor_col)
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(|s| s.to_string()).collect()
        };
        self.cursor_line = self.lines.len().saturating_sub(1);
        self.cursor_col = self.lines.last().map_or(0, |l| l.len());
        self.scroll_offset = 0;
        self.preferred_col = None;
    }

    pub fn add_to_history(&mut self, text: &str) {
        self.history.push(text.to_string());
        self.history_index = -1;
    }

    pub fn insert_text_at_cursor(&mut self, text: &str) {
        self.exit_history();
        self.last_action = None;
        self.push_undo();
        self.insert_text_internal(text);
    }

    // ── Autocomplete (pi-style: uses SelectList) ──

    /// Set the autocomplete provider (handles slash commands, file paths, etc.).
    pub fn set_autocomplete_provider(&mut self, provider: Box<dyn AutocompleteProvider>) {
        self.autocomplete_provider = Some(provider);
    }

    pub fn set_autocomplete(&mut self, items: Vec<SelectItem>) {
        if items.is_empty() {
            self.autocomplete_active = false;
            self.autocomplete_list = None;
            return;
        }
        // Build SelectListTheme with standalone closures (Box<dyn Fn> is not Clone).
        let theme = SelectListTheme {
            selected_prefix: Box::new(|s| {
                format!("\x1b[7m\x1b[38;2;138;190;183m→ {}\x1b[27m\x1b[39m", s)
            }),
            selected_text: Box::new(|s| {
                format!("\x1b[7m\x1b[38;2;138;190;183m{}\x1b[27m\x1b[39m", s)
            }),
            normal_text: Box::new(|s| format!("\x1b[38;2;128;128;128m{}\x1b[39m", s)),
            description: Box::new(|s| format!("\x1b[38;2;128;128;128m{}\x1b[39m", s)),
            scroll_info: Box::new(|s| format!("\x1b[38;2;128;128;128m{}\x1b[39m", s)),
            no_match: Box::new(|s| s.to_string()),
            hint: Box::new(|s| s.to_string()),
        };
        let mut list = SelectList::new(items, 5, theme, None);
        list.set_selected_index(0);
        self.autocomplete_list = Some(list);
        self.autocomplete_active = true;
    }

    pub fn clear_autocomplete(&mut self) {
        self.autocomplete_active = false;
        self.autocomplete_list = None;
    }

    pub fn autocomplete_selected_value(&self) -> Option<String> {
        self.autocomplete_list
            .as_ref()
            .and_then(|l| l.selected_item())
            .map(|item| item.value.clone())
    }

    pub fn autocomplete_is_empty(&self) -> bool {
        self.autocomplete_list
            .as_ref()
            .is_none_or(|l| l.items().is_empty())
    }

    // ── Undo ──

    // Pi fish-style undo coalescing:
    // - Consecutive word chars coalesce into one undo unit
    // - Space captures state before itself (undo removes space + following word)
    fn maybe_push_undo(&mut self, ch: &str) {
        if is_whitespace_char(ch) || self.last_action.as_deref() != Some("type_word") {
            self.undo_stack.push(&EditorSnapshot {
                lines: self.lines.clone(),
                cursor_line: self.cursor_line,
                cursor_col: self.cursor_col,
            });
        }
        self.last_action = Some("type_word".into());
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(&EditorSnapshot {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        });
    }

    fn undo(&mut self) {
        if let Some(snap) = self.undo_stack.pop() {
            self.lines = snap.lines;
            self.cursor_line = snap.cursor_line;
            self.cursor_col = snap.cursor_col;
            self.preferred_col = None;
        }
    }

    // ── Cursor ──

    fn set_cursor_col(&mut self, col: usize) {
        self.cursor_col = col;
        self.preferred_col = None;
    }

    // ── Text insertion ──

    fn insert_text_internal(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let normalized = text.replace("\r\n", "\n").replace('\t', "    ");
        let inserted_lines: Vec<&str> = normalized.split('\n').collect();
        let current_line = self.lines[self.cursor_line].clone();
        let before = &current_line[..self.cursor_col.min(current_line.len())];
        let after = &current_line[self.cursor_col.min(current_line.len())..];

        if inserted_lines.len() == 1 {
            self.lines[self.cursor_line] = format!("{}{}{}", before, normalized, after);
            self.set_cursor_col(self.cursor_col + normalized.len());
        } else {
            let mut new_lines: Vec<String> = Vec::new();
            new_lines.extend(self.lines[..self.cursor_line].iter().cloned());
            new_lines.push(format!("{}{}", before, inserted_lines[0]));
            for line in &inserted_lines[1..inserted_lines.len() - 1] {
                new_lines.push(line.to_string());
            }
            new_lines.push(format!("{}{}", inserted_lines.last().unwrap_or(&""), after));
            new_lines.extend(self.lines[self.cursor_line + 1..].iter().cloned());
            self.lines = new_lines;
            self.cursor_line += inserted_lines.len() - 1;
            self.set_cursor_col(inserted_lines.last().map_or(0, |l| l.len()));
        }
        self.notify_change();
    }

    fn insert_character(&mut self, ch: &str) {
        self.exit_history();
        self.maybe_push_undo(ch);
        self.insert_text_internal(ch);

        // Pi-style autocomplete auto-trigger
        self.check_autocomplete_trigger(ch);
    }

    /// Check if the just-typed character should trigger autocomplete.
    /// Pi behavior: / at start of line, @ and # at token boundaries,
    /// and letters when already in a slash command context.
    fn check_autocomplete_trigger(&mut self, ch: &str) {
        if self.autocomplete_active {
            return; // Already showing
        }
        let current_line = &self.lines[self.cursor_line];
        let text_before = &current_line[..self.cursor_col.min(current_line.len())];

        // / at the start of the line (or after whitespace)
        if ch == "/" {
            let before_char = text_before.chars().nth_back(1); // char right before /
            if text_before.len() == 1 || before_char.is_none_or(|c| c.is_whitespace()) {
                self.try_trigger_autocomplete();
                return;
            }
        }

        // @ and # at token boundaries
        if ch == "@" || ch == "#" {
            let before_char = text_before.chars().nth_back(1);
            if text_before.len() == 1 || before_char.is_none_or(|c| c.is_whitespace() || c == ' ' || c == '\t') {
                self.try_trigger_autocomplete();
                return;
            }
        }

        // Letters when in a slash command context
        if ch.len() == 1 && ch.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            if text_before.starts_with('/') && !text_before.contains(' ') {
                self.try_trigger_autocomplete();
                return;
            }
            // Also trigger for @ and # contexts
            if text_before.contains('@') || text_before.contains('#') {
                self.try_trigger_autocomplete();
            }
        }
    }

    fn try_trigger_autocomplete(&mut self) {
        let Some(ref provider) = self.autocomplete_provider else {
            return;
        };
        if let Some(suggestions) = provider.get_suggestions(
            &self.lines,
            self.cursor_line,
            self.cursor_col,
            false,
        ) {
            let items: Vec<SelectItem> = suggestions
                .items
                .into_iter()
                .map(|item| {
                    let mut si = SelectItem::new(item.value, item.label);
                    if let Some(desc) = item.description {
                        si = si.with_description(desc);
                    }
                    si
                })
                .collect();
            if !items.is_empty() {
                self.set_autocomplete(items);
            }
        }
    }

    fn add_newline(&mut self) {
        self.exit_history();
        self.last_action = None;
        self.push_undo();
        let line = self.lines[self.cursor_line].clone();
        let before = &line[..self.cursor_col.min(line.len())];
        let after = &line[self.cursor_col.min(line.len())..];
        self.lines[self.cursor_line] = before.to_string();
        self.lines.insert(self.cursor_line + 1, after.to_string());
        self.cursor_line += 1;
        self.set_cursor_col(0);
        self.notify_change();
    }

    // ── Delete ──

    fn backspace(&mut self) {
        self.exit_history();
        self.last_action = None;
        if self.cursor_col > 0 {
            self.push_undo();
            let line = self.lines[self.cursor_line].clone();
            let graphemes: Vec<(usize, &str)> =
                line[..self.cursor_col].grapheme_indices(true).collect();
            if let Some(&(idx, g)) = graphemes.last() {
                self.lines[self.cursor_line].drain(idx..idx + g.len());
                self.set_cursor_col(idx);
            }
        } else if self.cursor_line > 0 {
            self.push_undo();
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            let prev_len = self.lines[self.cursor_line].len();
            self.lines[self.cursor_line].push_str(&current);
            self.set_cursor_col(prev_len);
        }
        self.notify_change();
    }

    fn delete_forward(&mut self) {
        self.exit_history();
        self.last_action = None;
        let line = self.lines[self.cursor_line].clone();
        if self.cursor_col < line.len() {
            self.push_undo();
            let graphemes: Vec<(usize, &str)> = line[self.cursor_col..]
                .grapheme_indices(true)
                .map(|(i, g)| (self.cursor_col + i, g))
                .collect();
            if let Some(&(idx, g)) = graphemes.first() {
                self.lines[self.cursor_line].drain(idx..idx + g.len());
            }
        } else if self.cursor_line + 1 < self.lines.len() {
            self.push_undo();
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
        }
        self.notify_change();
    }

    // ── Kill operations ──

    fn delete_to_line_start(&mut self) {
        self.exit_history();
        let line = self.lines[self.cursor_line].clone();
        if self.cursor_col > 0 {
            self.push_undo();
            let deleted = line[..self.cursor_col].to_string();
            let accumulate = self.last_action.as_deref() == Some("kill");
            self.kill_ring.push(&deleted, true, accumulate);
            self.last_action = Some("kill".into());
            self.lines[self.cursor_line] = line[self.cursor_col..].to_string();
            self.set_cursor_col(0);
        } else if self.cursor_line > 0 {
            self.push_undo();
            let accumulate = self.last_action.as_deref() == Some("kill");
            self.kill_ring.push("\n", true, accumulate);
            self.last_action = Some("kill".into());
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            let prev_len = self.lines[self.cursor_line].len();
            self.lines[self.cursor_line].push_str(&current);
            self.set_cursor_col(prev_len);
        }
        self.notify_change();
    }

    fn delete_to_line_end(&mut self) {
        self.exit_history();
        let line = self.lines[self.cursor_line].clone();
        if self.cursor_col < line.len() {
            self.push_undo();
            let deleted = line[self.cursor_col..].to_string();
            let accumulate = self.last_action.as_deref() == Some("kill");
            self.kill_ring.push(&deleted, false, accumulate);
            self.last_action = Some("kill".into());
            self.lines[self.cursor_line] = line[..self.cursor_col].to_string();
        } else if self.cursor_line + 1 < self.lines.len() {
            self.push_undo();
            let accumulate = self.last_action.as_deref() == Some("kill");
            self.kill_ring.push("\n", false, accumulate);
            self.last_action = Some("kill".into());
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
        }
        self.notify_change();
    }

    fn delete_word_backward(&mut self) {
        self.exit_history();
        let line = self.lines[self.cursor_line].clone();
        if self.cursor_col == 0 {
            return;
        }
        let new_col = find_word_backward(&line, self.cursor_col);
        if new_col < self.cursor_col {
            self.push_undo();
            let deleted = line[new_col..self.cursor_col].to_string();
            let accumulate = self.last_action.as_deref() == Some("kill");
            self.kill_ring.push(&deleted, true, accumulate);
            self.last_action = Some("kill".into());
            self.lines[self.cursor_line].drain(new_col..self.cursor_col);
            self.set_cursor_col(new_col);
            self.notify_change();
        }
    }

    fn delete_word_forward(&mut self) {
        self.exit_history();
        let line = self.lines[self.cursor_line].clone();
        if self.cursor_col >= line.len() {
            return;
        }
        let new_col = find_word_forward(&line, self.cursor_col);
        if new_col > self.cursor_col {
            self.push_undo();
            let deleted = line[self.cursor_col..new_col].to_string();
            let accumulate = self.last_action.as_deref() == Some("kill");
            self.kill_ring.push(&deleted, false, accumulate);
            self.last_action = Some("kill".into());
            self.lines[self.cursor_line].drain(self.cursor_col..new_col);
            self.notify_change();
        }
    }

    // ── Yank ──

    fn yank(&mut self) {
        self.exit_history();
        let text = self.kill_ring.peek().map(|s| s.to_string());
        if let Some(text) = text {
            self.push_undo();
            self.cursor_col += text.len();
            self.lines[self.cursor_line].insert_str(self.cursor_col - text.len(), &text);
            self.notify_change();
        }
    }

    fn yank_pop(&mut self) {
        self.kill_ring.rotate();
        let text = self.kill_ring.peek().map(|s| s.to_string());
        if let Some(snap) = self.undo_stack.pop() {
            self.lines = snap.lines;
            self.cursor_line = snap.cursor_line;
            self.cursor_col = snap.cursor_col;
        }
        if let Some(text) = text {
            self.push_undo();
            self.cursor_col += text.len();
            self.lines[self.cursor_line].insert_str(self.cursor_col - text.len(), &text);
            self.notify_change();
        }
    }

    // ── Cursor movement ──

    fn move_left(&mut self) {
        self.last_action = None;
        if self.cursor_col > 0 {
            let line = &self.lines[self.cursor_line];
            let graphemes: Vec<(usize, &str)> =
                line[..self.cursor_col].grapheme_indices(true).collect();
            if let Some(&(idx, _g)) = graphemes.last() {
                self.set_cursor_col(idx);
            }
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.set_cursor_col(self.lines[self.cursor_line].len());
        }
    }

    fn move_right(&mut self) {
        self.last_action = None;
        let line = &self.lines[self.cursor_line];
        if self.cursor_col < line.len() {
            let mut it = line[self.cursor_col..].grapheme_indices(true);
            if let Some((idx, g)) = it.next() {
                self.set_cursor_col(self.cursor_col + idx + g.len());
            }
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.set_cursor_col(0);
        }
    }

    fn move_up(&mut self) {
        self.move_vertical(-1);
    }

    fn move_down(&mut self) {
        self.move_vertical(1);
    }

    fn move_to_line_start(&mut self) {
        self.last_action = None;
        self.set_cursor_col(0);
    }

    fn move_to_line_end(&mut self) {
        self.last_action = None;
        let len = self.lines[self.cursor_line].len();
        self.set_cursor_col(len);
    }

    fn move_vertical(&mut self, delta: isize) {
        let target_line = if delta < 0 {
            if self.cursor_line == 0 {
                return;
            }
            self.cursor_line - 1
        } else if self.cursor_line + 1 >= self.lines.len() {
            return;
        } else {
            self.cursor_line + 1
        };

        let target_len = self.lines[target_line].len();
        let pref = self.preferred_col.unwrap_or(self.cursor_col);
        self.preferred_col = Some(pref);
        self.cursor_line = target_line;
        self.cursor_col = pref.min(target_len);
    }

    // ── Character jump (pi-style) ──

    fn jump_to_char(&mut self, ch: char, dir: JumpDirection) {
        let line = &self.lines[self.cursor_line].clone();
        match dir {
            JumpDirection::Forward => {
                // Find ch after cursor
                let start = self.cursor_col + 1;
                if start < line.len() {
                    let rest = &line[self.cursor_col + 1..];
                    if let Some(pos) = rest.find(ch) {
                        self.set_cursor_col(self.cursor_col + 1 + pos);
                    }
                }
            }
            JumpDirection::Backward => {
                // Find ch before cursor
                if self.cursor_col > 0 {
                    let before = &line[..self.cursor_col];
                    if let Some(pos) = before.rfind(ch) {
                        self.set_cursor_col(pos);
                    }
                }
            }
        }
    }

    // ── History ──

    fn exit_history(&mut self) {
        self.history_index = -1;
        self.history_draft = None;
        self.undo_stack.clear();
        self.last_action = None;
    }

    fn recall_older(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = if self.history_index < 0 {
            self.history.len() as i32 - 1
        } else {
            self.history_index - 1
        };
        if idx < 0 {
            return;
        }

        // Pi: save draft when first entering history browsing
        if self.history_index < 0 && idx >= 0 {
            self.history_draft = Some(EditorSnapshot {
                lines: self.lines.clone(),
                cursor_line: self.cursor_line,
                cursor_col: self.cursor_col,
            });
        }

        let text = self.history[idx as usize].clone();
        self.set_text(&text);
        self.cursor_col = 0; // pi: cursor at start when going older
        self.history_index = idx;
    }

    fn recall_newer(&mut self) {
        if self.history_index < 0 {
            return;
        }
        let idx = self.history_index + 1;
        if idx >= self.history.len() as i32 {
            // Pi: restore draft instead of clearing to empty
            if let Some(draft) = self.history_draft.take() {
                self.lines = draft.lines;
                self.cursor_line = draft.cursor_line;
                self.cursor_col = draft.cursor_col;
                self.preferred_col = None;
            } else {
                self.set_text("");
            }
            self.history_index = -1;
        } else {
            let text = self.history[idx as usize].clone();
            self.set_text(&text);
            self.history_index = idx;
        }
    }

    // ── Paste markers (pi-style) ──

    /// Handle a paste: for large pastes (>10 lines or >1000 chars),
    /// stores the content and inserts a marker like "[paste #1 +123 lines]".
    /// Small pastes are inserted directly.
    pub fn handle_paste(&mut self, text: &str) {
        let lines: Vec<&str> = text.split('\n').collect();
        let total_chars = text.len();

        if lines.len() > 10 || total_chars > 1000 {
            self.paste_counter += 1;
            let paste_id = self.paste_counter;
            self.pastes.insert(paste_id, text.to_string());

            let marker = if lines.len() > 10 {
                format!("[paste #{} +{} lines]", paste_id, lines.len())
            } else {
                format!("[paste #{} {} chars]", paste_id, total_chars)
            };
            self.insert_text_at_cursor(&marker);
        } else {
            self.insert_text_at_cursor(text);
        }
    }

    /// Expand paste markers in text back to their full content.
    pub fn expand_paste_markers(&self, text: &str) -> String {
        let mut result = text.to_string();
        // Replace markers from highest ID to lowest to avoid ID conflicts
        let mut ids: Vec<u32> = self.pastes.keys().copied().collect();
        ids.sort_unstable_by(|a, b| b.cmp(a)); // descending
        for paste_id in ids {
            if let Some(content) = self.pastes.get(&paste_id) {
                // Simple replacement — find any marker with this ID
                let marker1 = format!("[paste #{} ", paste_id);
                loop {
                    let start = result.find(&marker1);
                    match start {
                        Some(pos) => {
                            let end = result[pos..].find(']').map(|e| pos + e + 1).unwrap_or(result.len());
                            result.replace_range(pos..end, content);
                        }
                        None => break,
                    }
                }
            }
        }
        result
    }

    /// Get text with paste markers expanded.
    /// Use this when you need the full content (e.g., for external editor).
    pub fn get_expanded_text(&self) -> String {
        self.expand_paste_markers(&self.lines.join("\n"))
    }

    /// Check if a string is a paste marker.
    pub fn is_paste_marker(segment: &str) -> bool {
        segment.starts_with("[paste #") && segment.ends_with(']')
    }

    // ── Page scroll ──

    fn page_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(self.max_visible_lines);
    }

    fn page_down(&mut self) {
        self.scroll_offset += self.max_visible_lines;
    }

    // ── Submit ──

    fn submit(&mut self) {
        // Pi: expand paste markers before submitting
        let raw = self.lines.join("\n");
        let result = self.expand_paste_markers(&raw);
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.scroll_offset = 0;
        self.pastes.clear();
        self.paste_counter = 0;
        self.undo_stack.clear();
        self.last_action = None;
        self.preferred_col = None;
        self.exit_history();
        if let Some(ref mut cb) = self.on_submit {
            cb(result);
        }
        self.notify_change();
    }

    // ── Helpers ──

    fn notify_change(&mut self) {
        let text = self.get_text();
        if let Some(ref mut cb) = self.on_change {
            cb(&text);
        }
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty() || (self.lines.len() == 1 && self.lines[0].is_empty())
    }

    fn is_first_visual_line(&self) -> bool {
        self.cursor_line == 0
    }

    fn is_last_visual_line(&self) -> bool {
        self.cursor_line >= self.lines.len().saturating_sub(1)
    }
}

// ── Component impl ─────────────────────────────────────────────────

impl Component for Editor {
    fn render(&self, width: usize) -> Vec<String> {
        let max_padding = if width > 1 { (width - 1) / 2 } else { 0 };
        let pad_x = self.padding_x.min(max_padding);
        let content_width = if width > pad_x * 2 {
            width - pad_x * 2
        } else {
            1
        };
        let layout_width = content_width.max(1);

        let horizontal = "─";
        let left_pad = " ".repeat(pad_x);
        let right_pad = " ".repeat(pad_x);
        let mut result: Vec<String> = Vec::new();

        // ── Layout text into visual lines, tracking cursor ──
        let visual_lines =
            layout_text(&self.lines, layout_width, self.cursor_line, self.cursor_col);
        let total_visual = visual_lines.len().max(1);

        // Find cursor visual line index
        let cursor_vis = visual_lines
            .iter()
            .position(|vl| vl.has_cursor)
            .unwrap_or(0);

        // Adjust scroll to keep cursor visible
        let max_vis = self.max_visible_lines.max(1);
        let mut scroll = self.scroll_offset;
        if cursor_vis < scroll {
            scroll = cursor_vis;
        } else if cursor_vis >= scroll + max_vis {
            scroll = cursor_vis - max_vis + 1;
        }
        let max_scroll = total_visual.saturating_sub(max_vis);
        scroll = scroll.min(max_scroll);

        let visible_end = (scroll + max_vis).min(total_visual);

        // ── Top border ──
        if scroll > 0 {
            let indicator = format!("─── ↑ {} more ", scroll);
            let indicator_w = visible_width(&indicator);
            let fill = if indicator_w < width {
                horizontal.repeat(width - indicator_w)
            } else {
                String::new()
            };
            result.push((self.border_color)(&format!("{}{}", indicator, fill)));
        } else {
            result.push((self.border_color)(&horizontal.repeat(width)));
        }

        // ── Content lines ──
        for vl in visual_lines.iter().skip(scroll).take(visible_end - scroll) {
            let text = &vl.text;
            let (display, line_width) = if vl.has_cursor {
                let cursor_pos = vl.cursor_pos.unwrap_or(0);
                let before = &text[..cursor_pos.min(text.len())];
                let after = &text[cursor_pos.min(text.len())..];

                let marker = if self.focused {
                    CURSOR_MARKER.to_string()
                } else {
                    String::new()
                };

                if !after.is_empty() {
                    let after_graphemes: Vec<&str> = after.graphemes(true).collect();
                    let first_g = after_graphemes.first().copied().unwrap_or(" ");
                    let rest = &after[first_g.len()..];
                    let cursor = format!("\x1b[7m{}\x1b[0m", first_g);
                    (
                        format!("{}{}{}{}", before, marker, cursor, rest),
                        visible_width(text),
                    )
                } else {
                    let cursor = "\x1b[7m \x1b[0m";
                    (
                        format!("{}{}{}", before, marker, cursor),
                        visible_width(text) + 1,
                    )
                }
            } else {
                (text.clone(), visible_width(text))
            };

            let padding = if line_width < content_width {
                " ".repeat(content_width - line_width)
            } else {
                String::new()
            };
            result.push(format!("{}{}{}{}", left_pad, display, padding, right_pad));
        }

        // ── Bottom border ──
        let below = total_visual.saturating_sub(visible_end);
        if below > 0 {
            let indicator = format!("─── ↓ {} more ", below);
            let indicator_w = visible_width(&indicator);
            let fill = if indicator_w < width {
                horizontal.repeat(width - indicator_w)
            } else {
                String::new()
            };
            result.push((self.border_color)(&format!("{}{}", indicator, fill)));
        } else {
            result.push((self.border_color)(&horizontal.repeat(width)));
        }

        // ── Autocomplete dropdown (pi-style: renders SelectList below bottom border) ──
        if self.autocomplete_active
            && let Some(ref list) = self.autocomplete_list
        {
            let list_lines = list.render(width);
            result.extend(list_lines);
        }

        result
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        let kb = get_keybindings();

        // ── Character jump mode: await next printable char ──
        if let Some(dir) = self.jump_mode {
            // Cancel on jump hotkey again
            if kb.matches(key, ACTION_EDITOR_JUMP_FORWARD) || kb.matches(key, ACTION_EDITOR_JUMP_BACKWARD) {
                self.jump_mode = None;
                return true;
            }
            if is_printable_plain(key) {
                if let Some(s) = key_event_to_string(key) {
                    let ch = s.chars().next().unwrap_or(' ');
                    self.jump_mode = None;
                    self.jump_to_char(ch, dir);
                    return true;
                }
            }
            // Non-printable cancels jump mode
            self.jump_mode = None;
        }

        // ── Autocomplete: route to SelectList (pi-style) ──
        if let Some(ref mut list) = self.autocomplete_list {
            if kb.matches(key, ACTION_SELECT_CANCEL) {
                self.clear_autocomplete();
                return true;
            }
            if kb.matches(key, ACTION_SELECT_CONFIRM) || kb.matches(key, ACTION_INPUT_TAB) {
                if let Some(val) = list.selected_item().map(|i| i.value.clone()) {
                    self.set_text(&format!("/{} ", val));
                }
                self.clear_autocomplete();
                return true;
            }
            if kb.matches(key, ACTION_SELECT_UP) || kb.matches(key, ACTION_SELECT_DOWN) {
                list.handle_input(key);
                return true;
            }
            self.clear_autocomplete();
        }

        // ── Enter / Submit ──
        if kb.matches(key, ACTION_INPUT_SUBMIT) {
            if self.disable_submit {
                self.add_newline();
                return true;
            }
            let line = &self.lines[self.cursor_line];
            if self.cursor_col > 0 && line.as_bytes().get(self.cursor_col - 1) == Some(&b'\\') {
                self.backspace();
                self.add_newline();
                return true;
            }
            self.submit();
            return true;
        }

        // ── Character jump triggers ──
        if kb.matches(key, ACTION_EDITOR_JUMP_FORWARD) {
            self.jump_mode = Some(JumpDirection::Forward);
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_JUMP_BACKWARD) {
            self.jump_mode = Some(JumpDirection::Backward);
            return true;
        }

        // ── Printable character ──
        if is_printable_plain(key)
            && let Some(s) = key_event_to_string(key)
        {
            self.insert_character(&s);
            return true;
        }

        // ── Basic movement ──
        if kb.matches(key, ACTION_EDITOR_CURSOR_LEFT) {
            self.move_left();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_CURSOR_RIGHT) {
            self.move_right();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_CURSOR_LINE_START) {
            self.move_to_line_start();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_CURSOR_LINE_END) {
            self.move_to_line_end();
            return true;
        }

        // ── Up/Down with history ──
        if kb.matches(key, ACTION_EDITOR_CURSOR_UP) {
            if self.is_first_visual_line()
                && (self.is_empty() || self.history_index >= 0 || self.cursor_col == 0)
            {
                self.recall_older();
            } else if self.is_first_visual_line() {
                self.move_to_line_start();
            } else {
                self.move_up();
            }
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_CURSOR_DOWN) {
            if self.history_index >= 0 && self.is_last_visual_line() {
                self.recall_newer();
            } else if self.is_last_visual_line() {
                self.move_to_line_end();
            } else {
                self.move_down();
            }
            return true;
        }

        // ── Page scroll ──
        if kb.matches(key, ACTION_EDITOR_PAGE_UP) {
            self.page_up();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_PAGE_DOWN) {
            self.page_down();
            return true;
        }

        // ── Word movement ──
        if kb.matches(key, ACTION_EDITOR_CURSOR_WORD_LEFT) {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col > 0 {
                let c = find_word_backward(line, self.cursor_col);
                self.set_cursor_col(c);
            }
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_CURSOR_WORD_RIGHT) {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col < line.len() {
                let c = find_word_forward(line, self.cursor_col);
                self.set_cursor_col(c);
            }
            return true;
        }

        // ── Deletion ──
        if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_BACKWARD) {
            self.backspace();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_FORWARD) {
            self.delete_forward();
            return true;
        }

        // ── Kill operations ──
        if kb.matches(key, ACTION_EDITOR_DELETE_WORD_BACKWARD) {
            self.delete_word_backward();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_DELETE_WORD_FORWARD) {
            self.delete_word_forward();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_DELETE_TO_LINE_START) {
            self.delete_to_line_start();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_DELETE_TO_LINE_END) {
            self.delete_to_line_end();
            return true;
        }

        // ── Yank ──
        if kb.matches(key, ACTION_EDITOR_YANK) {
            self.yank();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_YANK_POP) {
            self.yank_pop();
            return true;
        }

        // ── Undo ──
        if kb.matches(key, ACTION_EDITOR_UNDO) {
            self.exit_history();
            self.last_action = None;
            self.undo();
            self.notify_change();
            return true;
        }

        // ── Ctrl+J = newline ──
        if kb.matches(key, ACTION_INPUT_NEW_LINE) {
            self.add_newline();
            return true;
        }

        // ── Escape — let parent handle ──
        if kb.matches(key, ACTION_SELECT_CANCEL) {
            return false;
        }

        false
    }

    fn is_focusable(&self) -> bool {
        true
    }
}

impl Focusable for Editor {
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn focused(&self) -> bool {
        self.focused
    }
}

// ── Visual layout ──────────────────────────────────────────────────

struct VisualLine {
    text: String,
    has_cursor: bool,
    cursor_pos: Option<usize>,
}

/// Layout text into visual lines, marking which line contains the cursor.
fn layout_text(
    lines: &[String],
    max_width: usize,
    cursor_line: usize,
    cursor_col: usize,
) -> Vec<VisualLine> {
    let mut result: Vec<VisualLine> = Vec::new();

    if lines.is_empty() || (lines.len() == 1 && lines[0].is_empty()) {
        result.push(VisualLine {
            text: String::new(),
            has_cursor: true,
            cursor_pos: Some(0),
        });
        return result;
    }

    let mut _col_offset = 0;

    for (line_idx, line) in lines.iter().enumerate() {
        let is_cursor_line = line_idx == cursor_line;
        let line_w = visible_width(line);
        _col_offset = 0;

        if line_w <= max_width {
            // Line fits entirely
            result.push(VisualLine {
                text: line.clone(),
                has_cursor: is_cursor_line,
                cursor_pos: if is_cursor_line {
                    Some(cursor_col.min(line.len()))
                } else {
                    None
                },
            });
        } else {
            // Word-wrap the line, tracking cursor position by visual column.
            // We cannot use byte-pos accumulation because wrap_text_with_ansi may
            // trim trailing whitespace or add ANSI codes, making chunk byte lengths
            // diverge from the original line's substrings.
            let wrapped = wrap_text_with_ansi(line, max_width);

            // Compute cursor's visual column position in the original line.
            let cursor_vis = if is_cursor_line {
                visible_width(&line[..cursor_col.min(line.len())])
            } else {
                0
            };

            let mut vis_offset: usize = 0;
            for (chunk_idx, chunk) in wrapped.iter().enumerate() {
                let chunk_vis = visible_width(chunk);
                let chunk_vis_end = vis_offset + chunk_vis;

                let cursor_in_chunk = is_cursor_line
                    && cursor_vis >= vis_offset
                    && (cursor_vis < chunk_vis_end || chunk_idx == wrapped.len() - 1);

                let cursor_pos = if cursor_in_chunk {
                    let local_vis = cursor_vis.saturating_sub(vis_offset);
                    // Convert visual offset within chunk to byte offset
                    Some(visual_col_to_byte_offset(chunk, local_vis))
                } else {
                    None
                };

                result.push(VisualLine {
                    text: chunk.clone(),
                    has_cursor: cursor_in_chunk && cursor_pos.is_some(),
                    cursor_pos,
                });

                vis_offset = chunk_vis_end;
            }
        }
    }

    result
}

fn is_printable_plain(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_))
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && key.code != KeyCode::Enter
        && key.code != KeyCode::Tab
        && key.code != KeyCode::Backspace
        && key.code != KeyCode::Delete
        && key.code != KeyCode::Esc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_editor() {
        let editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        assert_eq!(editor.get_text(), "");
    }

    #[test]
    fn test_set_text() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("hello world");
        assert_eq!(editor.get_text(), "hello world");
    }

    #[test]
    fn test_insert_and_move() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.insert_character("h");
        editor.insert_character("i");
        assert_eq!(editor.get_text(), "hi");
        editor.move_left();
        assert_eq!(editor.cursor_col, 1);
        editor.move_right();
        assert_eq!(editor.cursor_col, 2);
    }

    #[test]
    fn test_backspace() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("hello");
        editor.backspace();
        assert_eq!(editor.get_text(), "hell");
    }

    #[test]
    fn test_multiline() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("line1\nline2");
        assert_eq!(editor.get_lines().len(), 2);
    }

    #[test]
    fn test_undo() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.push_undo();
        editor.insert_text_internal("a");
        editor.push_undo();
        editor.insert_text_internal("b");
        assert_eq!(editor.get_text(), "ab");
        editor.undo();
        assert_eq!(editor.get_text(), "a");
        editor.undo();
        assert_eq!(editor.get_text(), "");
    }

    #[test]
    fn test_submit_clears() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("hello");
        let result = editor.lines.join("\n");
        editor.lines = vec![String::new()];
        editor.cursor_line = 0;
        editor.cursor_col = 0;
        assert_eq!(result, "hello");
        assert_eq!(editor.get_text(), "");
    }

    #[test]
    fn test_render_borders() {
        let editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        let lines = editor.render(80);
        assert!(lines.len() >= 3);
        assert!(lines[0].contains('─'));
        assert!(lines.last().unwrap().contains('─'));
    }

    #[test]
    fn test_scroll_indicator() {
        let mut editor = Editor::new(
            EditorTheme::default(),
            EditorOptions {
                padding_x: 1,
                max_visible_lines: 2,
            },
        );
        editor.set_text("line1\nline2\nline3\nline4");
        editor.scroll_offset = 1;
        let lines = editor.render(80);
        assert!(lines[0].contains("↑"));
    }

    #[test]
    fn test_newline() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("hello");
        editor.add_newline();
        assert_eq!(editor.get_text(), "hello\n");
        editor.insert_character("w");
        assert_eq!(editor.get_text(), "hello\nw");
    }

    #[test]
    fn test_cursor_in_layout() {
        let editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        // Empty editor — cursor should be in visual line 0
        let vl = layout_text(&editor.lines, 80, editor.cursor_line, editor.cursor_col);
        assert!(vl[0].has_cursor);
        assert_eq!(vl[0].cursor_pos, Some(0));
    }

    #[test]
    fn test_cursor_in_layout_with_text() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("abc");
        editor.cursor_col = 1;
        let vl = layout_text(&editor.lines, 80, editor.cursor_line, editor.cursor_col);
        assert!(vl[0].has_cursor);
        assert_eq!(vl[0].cursor_pos, Some(1));
    }

    // ── Ported from pi-tui editor.test.ts ──

    fn up_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)
    }
    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn enter_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    #[test]
    fn test_history_empty_up_does_nothing() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.handle_input(&up_key());
        assert_eq!(editor.get_text(), "");
    }

    #[test]
    fn test_history_up_shows_most_recent() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.add_to_history("first");
        editor.add_to_history("second");
        editor.handle_input(&up_key());
        assert_eq!(editor.get_text(), "second");
    }

    #[test]
    fn test_history_cycles() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.add_to_history("first");
        editor.add_to_history("second");
        editor.add_to_history("third");
        editor.handle_input(&up_key());
        assert_eq!(editor.get_text(), "third");
        editor.handle_input(&up_key());
        assert_eq!(editor.get_text(), "second");
        editor.handle_input(&up_key());
        assert_eq!(editor.get_text(), "first");
        editor.handle_input(&up_key()); // stays at oldest
        assert_eq!(editor.get_text(), "first");
    }

    #[test]
    fn test_history_exits_on_type() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.add_to_history("old");
        editor.handle_input(&up_key());
        assert_eq!(editor.get_text(), "old");
        editor.handle_input(&char_key('x'));
        assert_eq!(editor.get_text(), "xold");
    }

    #[test]
    fn test_backslash_enter_newline() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.handle_input(&char_key('\\'));
        assert_eq!(editor.get_text(), "\\");
        editor.handle_input(&enter_key());
        assert_eq!(editor.get_text(), "\n");
    }

    #[test]
    fn test_move_cursor_over_emoji() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("a😀b");
        editor.cursor_col = 0;
        editor.move_right();
        assert_eq!(editor.cursor_col, 1);
        editor.move_right();
        assert_eq!(editor.cursor_col, 5);
        editor.move_right();
        assert_eq!(editor.cursor_col, 6);
    }

    #[test]
    fn test_backspace_emoji() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("a😀b");
        editor.cursor_col = 6;
        editor.backspace();
        assert_eq!(editor.get_text(), "a😀");
        editor.backspace();
        assert_eq!(editor.get_text(), "a");
    }

    #[test]
    fn test_render_cursor_visible() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.focused = true;
        editor.insert_character("x");
        let lines = editor.render(40);
        let content = &lines[1];
        assert!(content.contains("\x1b[7m"), "Cursor inverse not found");
    }

    #[test]
    fn test_render_borders_always_present() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        let lines = editor.render(80);
        assert_eq!(lines.len(), 3, "Empty editor should have 3 lines");
        assert!(lines[0].contains('─'), "Top border missing");
        assert!(lines[2].contains('─'), "Bottom border missing");

        editor.insert_character("/");
        let lines = editor.render(80);
        assert_eq!(lines.len(), 3, "After typing / should still have 3 lines");
        assert!(lines[0].contains('─'), "Top border missing after /");
        assert!(lines[2].contains('─'), "Bottom border missing after /");

        editor.set_text("hello world this is text");
        let lines = editor.render(40);
        assert!(lines.len() >= 3, "Wrapped text: {}", lines.len());
        assert!(lines[0].contains('─'), "Top border");
        assert!(lines.last().unwrap().contains('─'), "Bottom border");
    }

    #[test]
    fn test_content_width_respected() {
        let mut editor = Editor::new(
            EditorTheme::default(),
            EditorOptions {
                padding_x: 1,
                max_visible_lines: 10,
            },
        );
        editor.set_text("hello world this is a test");
        let lines = editor.render(20);
        for line in &lines {
            let vw = crate::tui::util::visible_width(line);
            assert!(vw <= 20, "Width {} > 20: {:?}", vw, line);
        }
    }

    // ── Wrap/duplication tests ───────────────────────────────────

    #[test]
    fn test_no_duplicate_chunks_from_wrapping() {
        // wrap_text_with_ansi must not produce duplicate chunks from a single input.
        // The same content may appear in multiple chunks if the source has it
        // multiple times, but it must not appear MORE times than in the original.
        let texts = [
            "hello world this is a test of the wrapping system",
            "a b c d e f g h i j k l m n o p q r s t u v w x y z",
            "short",
            "",
            "abc abc abc abc abc abc abc abc",
            "  leading and trailing spaces  ",
            "hello   world   extra   spaces",
        ];
        for text in &texts {
            for width in [1, 2, 3, 5, 8, 12, 20, 40] {
                let wrapped = crate::tui::util::wrap_text_with_ansi(text, width);

                // Total visible content of wrapped chunks must not exceed original
                let total_vis_wrapped: usize = wrapped.iter().map(|c| visible_width(c)).sum();
                let total_vis_original = visible_width(text);
                assert!(
                    total_vis_wrapped <= total_vis_original,
                    "Width={}: wrapped visible {} > original visible {} for {:?}",
                    width,
                    total_vis_wrapped,
                    total_vis_original,
                    text
                );

                // No non-empty chunk should appear more times than it occurs
                // as a substring in the original text.
                for a in &wrapped {
                    if a.is_empty() {
                        continue;
                    }
                    let count_in_wrapped = wrapped.iter().filter(|c| *c == a).count();
                    let count_in_original = text.matches(a.as_str()).count();
                    assert!(
                        count_in_wrapped <= count_in_original || count_in_original == 0,
                        "Width={}: chunk '{}' appears {}x in wrapped but {}x in original for {:?}",
                        width,
                        a,
                        count_in_wrapped,
                        count_in_original,
                        text
                    );
                }
            }
        }
    }

    #[test]
    fn test_cursor_in_wrapped_text_first_chunk() {
        // Cursor at position within first wrapped chunk
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        let text = "hello world this is a test";
        editor.set_text(text);
        // cursor at position 3 ('l' in "hello")
        editor.cursor_col = 3;
        let vl = layout_text(&editor.lines, 10, editor.cursor_line, editor.cursor_col);
        assert!(vl.len() > 1, "Text should wrap into multiple visual lines");
        assert!(
            vl[0].has_cursor,
            "Cursor at col 3 should be in first visual line"
        );
        if let Some(pos) = vl[0].cursor_pos {
            assert_eq!(pos, 3, "Cursor byte offset in first chunk should be 3");
        }
    }

    #[test]
    fn test_cursor_in_wrapped_text_middle_chunk() {
        // Cursor at position within the middle chunk
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        let text = "hello world this is a test";
        editor.set_text(text);
        // "hello world this" = 16 chars, cursor at col 16 = end of "hello world this"
        // which should be the last byte of chunk 0 ("hello worl" at width 10)
        // Actually at width 10, chunk 0 might be "hello worl", chunk 1 "d this is", chunk 2 " a test"
        editor.cursor_col = 16;
        let vl = layout_text(&editor.lines, 10, editor.cursor_line, editor.cursor_col);
        assert!(vl.len() > 1, "Text should wrap");
        let cursor_vl = vl.iter().position(|v| v.has_cursor);
        assert!(
            cursor_vl.is_some(),
            "Cursor should be found in some visual line"
        );
    }

    #[test]
    fn test_cursor_last_chunk_on_boundary() {
        // Cursor at last byte of text — should be in the last visual line
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        let text = "hello world this is a test";
        editor.set_text(text);
        editor.cursor_col = text.len();
        let vl = layout_text(&editor.lines, 10, editor.cursor_line, editor.cursor_col);
        assert!(
            vl.last().is_some_and(|v| v.has_cursor),
            "Cursor at end should be in last visual line"
        );
    }

    #[test]
    fn test_layout_text_each_chunk_unique() {
        // layout_text should never produce VisualLines with identical text
        // from a single logical line's wrapping.
        let text = "hello world this is a test of the wrapping system";
        let vl = layout_text(&[text.to_string()], 12, 0, 0);
        let chunk_texts: Vec<&str> = vl.iter().map(|v| v.text.as_str()).collect();
        for i in 0..chunk_texts.len() {
            for j in (i + 1)..chunk_texts.len() {
                if chunk_texts[i] == chunk_texts[j] {
                    // Same text is OK if the text is empty (edge case)
                    if !chunk_texts[i].is_empty() {
                        panic!(
                            "Duplicate chunk text at positions {} and {}: '{}'",
                            i, j, chunk_texts[i]
                        );
                    }
                }
            }
        }
    }

    // ── visual_col_to_byte_offset tests ──────────────────────────

    #[test]
    fn test_visual_col_to_byte_offset_ascii() {
        let text = "hello";
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 0), 0);
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 3), 3);
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 5), 5);
    }

    #[test]
    fn test_visual_col_to_byte_offset_cjk() {
        let text = "世界hello";
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 0), 0);
        // "世" is width 2, 2 bytes
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 2), 3);
        // "世界" is width 4, 6 bytes
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 4), 6);
    }

    #[test]
    fn test_visual_col_to_byte_offset_ansi() {
        // "\x1b[31m" = 5 bytes, "hello" = 5 bytes, "\x1b[0m" = 4 bytes = 14 total
        let text = "\x1b[31mhello\x1b[0m";
        // visible width is 5 ("hello"), ANSI codes are invisible
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 0), 5); // "h" at byte 5
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 1), 6); // "e" at byte 6
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 2), 7); // first "l" at byte 7
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 3), 8); // second "l" at byte 8
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 4), 9); // "o" at byte 9
        assert_eq!(crate::tui::util::visual_col_to_byte_offset(text, 5), 14); // end at byte 14
    }

    #[test]
    fn test_visual_col_to_byte_offset_empty() {
        assert_eq!(crate::tui::util::visual_col_to_byte_offset("", 0), 0);
        assert_eq!(crate::tui::util::visual_col_to_byte_offset("", 5), 0);
    }

    #[test]
    fn test_visual_col_to_byte_offset_zero_col() {
        // Plain ASCII: first visible char is at byte 0
        assert_eq!(crate::tui::util::visual_col_to_byte_offset("abc", 0), 0);
        // ANSI-prefixed: first visible char is after the ANSI code
        assert_eq!(
            crate::tui::util::visual_col_to_byte_offset("\x1b[31mabc", 0),
            5
        );
    }

    // ── Paste marker tests ──

    #[test]
    fn test_large_paste_creates_marker() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        let large = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11";
        editor.handle_paste(large);
        let text = editor.get_text();
        assert!(text.contains("[paste #"), "Should contain paste marker");
        assert!(!text.contains("line1"), "Should not contain original content");
        assert_eq!(editor.pastes.len(), 1, "Should store one paste");
    }

    #[test]
    fn test_small_paste_no_marker() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.handle_paste("hello");
        let text = editor.get_text();
        assert!(!text.contains("[paste #"), "Small paste should not create marker");
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_expand_paste_markers() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.handle_paste("line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11");
        let expanded = editor.get_expanded_text();
        assert!(expanded.contains("line1"), "Expanded text should contain original content");
        assert!(!expanded.contains("[paste #"), "Expanded text should not contain markers");
    }

    #[test]
    fn test_submit_expands_markers() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.handle_paste("line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11");
        let large_content = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11";
        // Manually call the submit logic to verify expansion
        let raw = editor.lines.join("\n");
        let expanded = editor.expand_paste_markers(&raw);
        assert_eq!(expanded, large_content, "Submit should expand to original content");
    }

    #[test]
    fn test_is_paste_marker() {
        assert!(Editor::is_paste_marker("[paste #1 +5 lines]"));
        assert!(Editor::is_paste_marker("[paste #123 456 chars]"));
        assert!(!Editor::is_paste_marker("normal text"));
        assert!(!Editor::is_paste_marker(""));
    }

    #[test]
    fn test_get_expanded_text() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.handle_paste("line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11");
        let expanded = editor.get_expanded_text();
        assert!(expanded.contains("line1"), "get_expanded_text should expand markers");
        assert!(expanded.starts_with("line1"), "Should start with original content");
    }
}
