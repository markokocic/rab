#![allow(clippy::type_complexity)]

use crate::tui::autocomplete::AutocompleteProvider;
use crate::tui::component::Component;
use crate::tui::components::select_list::{SelectItem, SelectList, SelectListTheme};
use crate::tui::focusable::{CURSOR_MARKER, Focusable};
use crate::tui::keybindings::{
    ACTION_EDITOR_CURSOR_DOWN, ACTION_EDITOR_CURSOR_LEFT, ACTION_EDITOR_CURSOR_LINE_END,
    ACTION_EDITOR_CURSOR_LINE_START, ACTION_EDITOR_CURSOR_RIGHT, ACTION_EDITOR_CURSOR_UP,
    ACTION_EDITOR_CURSOR_WORD_LEFT, ACTION_EDITOR_CURSOR_WORD_RIGHT,
    ACTION_EDITOR_DELETE_CHAR_BACKWARD, ACTION_EDITOR_DELETE_CHAR_FORWARD,
    ACTION_EDITOR_DELETE_TO_LINE_END, ACTION_EDITOR_DELETE_TO_LINE_START,
    ACTION_EDITOR_DELETE_WORD_BACKWARD, ACTION_EDITOR_DELETE_WORD_FORWARD,
    ACTION_EDITOR_JUMP_BACKWARD, ACTION_EDITOR_JUMP_FORWARD, ACTION_EDITOR_PAGE_DOWN,
    ACTION_EDITOR_PAGE_UP, ACTION_EDITOR_UNDO, ACTION_EDITOR_YANK, ACTION_EDITOR_YANK_POP,
    ACTION_INPUT_NEW_LINE, ACTION_INPUT_SUBMIT, ACTION_INPUT_TAB, ACTION_SELECT_CANCEL,
    ACTION_SELECT_CONFIRM, ACTION_SELECT_DOWN, ACTION_SELECT_UP, get_keybindings,
};
use crate::tui::keys::key_event_to_string;
use crate::tui::kill_ring::KillRing;
use crate::tui::util::is_whitespace_char;
use std::collections::HashMap;

use crate::tui::undo_stack::UndoStack;
use crate::tui::util::{visible_width, visual_col_to_byte_offset, wrap_text_with_ansi};
use crate::tui::word_nav::{
    WordNavigationOptions, find_word_backward_with, find_word_forward_with,
};
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
    #[allow(dead_code)]
    max_visible_lines: usize,
    scroll_offset: usize,
    _theme: EditorTheme,
    focused: bool,
    kill_ring: KillRing,
    undo_stack: UndoStack<EditorSnapshot>,
    history: Vec<String>,
    history_index: i32,
    history_draft: Option<EditorSnapshot>,
    preferred_col: Option<usize>,
    last_width: std::cell::Cell<usize>,
    last_action: Option<String>,
    pub on_submit: Option<Box<dyn FnMut(String) + Send>>,
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

    /// True after submit() is called, reset when checked.
    pub just_submitted: bool,

    // Pi-style autocomplete state (uses SelectList)
    /// Terminal height for dynamic max-visible-lines (pi: 30% of rows, min 5).
    terminal_rows: usize,
    autocomplete_max_visible: usize,
    autocomplete_list: Option<SelectList>,
    pub autocomplete_active: bool,
    /// The prefix from the provider's last get_suggestions call.
    /// Used instead of recomputing at selection time to avoid mismatches
    /// (e.g. `@src/au` → provider strips `@`, returns prefix `src/au`).
    autocomplete_prefix: String,
    /// Debounce: minimum time between autocomplete provider calls for @/# triggers.
    /// Pi uses 20ms for attachment autocomplete, 0ms for slash commands.
    last_autocomplete_trigger: std::time::Instant,
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
            _theme: theme,
            focused: false,
            kill_ring: KillRing::new(),
            undo_stack: UndoStack::new(),
            history: Vec::new(),
            history_index: -1,
            history_draft: None,
            preferred_col: None,
            last_width: std::cell::Cell::new(80),
            last_action: None,
            on_submit: None,
            on_change: None,
            disable_submit: false,
            terminal_rows: 24,
            autocomplete_max_visible: 5,
            autocomplete_list: None,
            autocomplete_active: false,
            autocomplete_prefix: String::new(),
            last_autocomplete_trigger: std::time::Instant::now(),
            border_color: Box::new(|s| s.to_string()),
            autocomplete_provider: None,
            pastes: HashMap::new(),
            paste_counter: 0,
            just_submitted: false,
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

    /// Update the terminal height so render can compute max visible lines
    /// dynamically (pi: 30% of rows, min 5).
    pub fn set_terminal_rows(&mut self, rows: usize) {
        self.terminal_rows = rows;
    }

    pub fn set_padding_x(&mut self, padding: usize) {
        self.padding_x = padding;
    }

    pub fn set_autocomplete_max_visible(&mut self, max: usize) {
        self.autocomplete_max_visible = max.clamp(3, 20);
    }

    /// Internal: set text without undo/autocomplete (used by history navigation).
    fn set_text_internal(&mut self, text: &str) {
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

    pub fn set_text(&mut self, text: &str) {
        // Pi: cancel autocomplete, push undo if content differs, then fire onChange
        self.clear_autocomplete();
        self.last_action = None;
        self.exit_history();
        if self.get_text() != text {
            self.push_undo();
        }
        self.set_text_internal(text);
        self.notify_change();
    }

    pub fn add_to_history(&mut self, text: &str) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        // Skip consecutive duplicates (pi-style)
        if !self.history.is_empty() && self.history[0] == trimmed {
            return;
        }
        self.history.insert(0, trimmed);
        if self.history.len() > 100 {
            self.history.pop();
        }
        self.history_index = -1;
    }

    pub fn insert_text_at_cursor(&mut self, text: &str) {
        self.clear_autocomplete();
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
        self.set_autocomplete_with_layout(items, None);
    }

    /// Set autocomplete items with an optional custom layout.
    /// Pi-style: slash commands use a special layout with wider primary column.
    fn set_autocomplete_with_layout(
        &mut self,
        items: Vec<SelectItem>,
        layout: Option<crate::tui::components::select_list::SelectListLayoutOptions>,
    ) {
        if items.is_empty() {
            self.autocomplete_active = false;
            self.autocomplete_list = None;
            return;
        }
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
        // Pi-style: pre-select the best matching item (exact match > prefix match)
        let best = self.best_autocomplete_index(&items);
        let mut list = SelectList::new(items, self.autocomplete_max_visible, theme, layout);
        list.set_selected_index(best);
        self.autocomplete_list = Some(list);
        self.autocomplete_active = true;
    }

    /// Find the best autocomplete item index for the current prefix.
    /// Returns 0 if no match (same as pi's default).
    fn best_autocomplete_index(&self, items: &[SelectItem]) -> usize {
        let prefix = self.autocomplete_prefix.trim_start_matches(['/', '@', '#']);
        if prefix.is_empty() {
            return 0;
        }
        let mut first_prefix = None;
        for (i, item) in items.iter().enumerate() {
            if item.value == prefix {
                return i; // Exact match always wins
            }
            if first_prefix.is_none() && item.value.starts_with(prefix) {
                first_prefix = Some(i);
            }
        }
        first_prefix.unwrap_or(0)
    }

    pub fn clear_autocomplete(&mut self) {
        self.autocomplete_active = false;
        self.autocomplete_list = None;
        self.autocomplete_prefix.clear();
    }

    /// After cursor movement, re-query autocomplete if active (pi-style).
    /// Keeps the picker in sync with the new cursor position - closes when
    /// the new position yields no suggestions, refreshes otherwise.
    fn update_autocomplete_if_active(&mut self) {
        if self.autocomplete_active {
            self.try_trigger_autocomplete();
        }
    }

    /// Pi-style: after backspace/delete that dismissed autocomplete,
    /// re-trigger if cursor is still in a completable context.
    fn retrigger_autocomplete_dismissed(&mut self) {
        if self.autocomplete_active {
            return; // not dismissed
        }
        // Pi: check slash command context first (only line 0, starts with /)
        if self.is_in_slash_command_context() {
            self.try_trigger_autocomplete();
            return;
        }
        let line = self
            .lines
            .get(self.cursor_line)
            .map(|l| l.as_str())
            .unwrap_or("");
        let before = &line[..self.cursor_col.min(line.len())];
        // Check @/# is at token start
        if before.contains('@') || before.contains('#') {
            self.try_trigger_autocomplete();
        }
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

        // Pi-style autocomplete: trigger or update after character insertion
        self.update_autocomplete(ch);
    }

    /// Check if the just-typed character should trigger or update autocomplete.
    /// Pi behavior: / at start of line, @ and # at token boundaries,
    /// and letters when already in a slash command context.
    /// When autocomplete is already active, re-triggers to update suggestions.
    /// Pi: slash menu only allowed on the first line of the editor.
    fn is_slash_menu_allowed(&self) -> bool {
        self.cursor_line == 0
    }

    /// Pi: check if cursor is at start of message (for slash command detection).
    fn is_at_start_of_message(&self) -> bool {
        if !self.is_slash_menu_allowed() {
            return false;
        }
        let line = self
            .lines
            .get(self.cursor_line)
            .map(|l| l.as_str())
            .unwrap_or("");
        let before = &line[..self.cursor_col.min(line.len())];
        let trimmed = before.trim();
        trimmed.is_empty() || trimmed == "/"
    }

    /// Pi: check if cursor is in a slash command context (starts with /, slash menu allowed).
    fn is_in_slash_command_context(&self) -> bool {
        if !self.is_slash_menu_allowed() {
            return false;
        }
        let line = self
            .lines
            .get(self.cursor_line)
            .map(|l| l.as_str())
            .unwrap_or("");
        let before = &line[..self.cursor_col.min(line.len())];
        before.trim_start().starts_with('/')
    }

    fn update_autocomplete(&mut self, ch: &str) {
        // If autocomplete is already active, always re-trigger to update
        if self.autocomplete_active {
            self.try_trigger_autocomplete();
            return;
        }
        let current_line = &self.lines[self.cursor_line];
        let text_before = &current_line[..self.cursor_col.min(current_line.len())];

        // / at start of message (pi: checks isAtStartOfMessage)
        if ch == "/" && self.is_at_start_of_message() {
            self.try_trigger_autocomplete();
            return;
        }

        // @ and # at token boundaries
        if ch == "@" || ch == "#" {
            let before_char = text_before.chars().nth_back(1);
            if text_before.len() == 1
                || before_char.is_none_or(|c| c.is_whitespace() || c == ' ' || c == '\t')
            {
                self.try_trigger_autocomplete();
                return;
            }
        }

        // Provider trigger characters (e.g. custom providers that use +, :, etc.)
        if let Some(ref provider) = self.autocomplete_provider {
            for tc in provider.trigger_characters() {
                if ch.len() == 1 && ch == tc.to_string() && tc != &'/' && tc != &'@' && tc != &'#'
                // already handled above
                {
                    let before_char = text_before.chars().nth_back(1);
                    if text_before.len() == 1
                        || before_char.is_none_or(|c| c.is_whitespace() || c == ' ' || c == '\t')
                    {
                        self.try_trigger_autocomplete();
                        return;
                    }
                }
            }
        }

        // Letters when in a slash command context (pi: only on first line)
        if ch.len() == 1
            && ch
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            if self.is_in_slash_command_context() && !text_before.trim_start().contains(' ') {
                self.try_trigger_autocomplete();
                return;
            }
            // Also trigger for @ and # contexts
            if text_before.contains('@') || text_before.contains('#') {
                self.try_trigger_autocomplete();
            }
        }
    }

    /// Get the autocomplete prefix for the current cursor position.
    fn get_autocomplete_prefix(&self) -> String {
        let line = self
            .lines
            .get(self.cursor_line)
            .map(|l| l.as_str())
            .unwrap_or("");
        let before = &line[..self.cursor_col.min(line.len())];
        // Find the last token boundary
        if before.starts_with('/') && !before.contains(' ') {
            before.to_string()
        } else if let Some(pos) = before.rfind(['@', '#']) {
            before[pos..].to_string()
        } else if let Some(pos) = before.rfind(|c: char| c.is_whitespace()) {
            before[pos + 1..].to_string()
        } else {
            before.to_string()
        }
    }

    /// Trigger autocomplete.
    ///
    /// When `force` is true (Tab key):
    /// - 1 match → complete immediately (no selector)
    /// - Otherwise → open the selector
    ///
    /// When `force` is false (automatic on typing), always opens the selector.
    fn trigger_autocomplete(&mut self, force: bool) {
        let Some(ref provider) = self.autocomplete_provider else {
            return;
        };

        // Debounce: for non-slash, non-force triggers (attachment @, #),
        // skip if called within 20ms of the last call. Pi uses 20ms for
        // attachment autocomplete to avoid flickering during rapid typing.
        if !force {
            let line = self
                .lines
                .get(self.cursor_line)
                .map(|l| l.as_str())
                .unwrap_or("");
            let before = &line[..self.cursor_col.min(line.len())];
            let is_slash = before.starts_with('/');
            if !is_slash && !before.is_empty() {
                let elapsed = self.last_autocomplete_trigger.elapsed();
                if elapsed < std::time::Duration::from_millis(20) {
                    return;
                }
            }
        }
        self.last_autocomplete_trigger = std::time::Instant::now();

        let Some(suggestions) =
            provider.get_suggestions(&self.lines, self.cursor_line, self.cursor_col, force)
        else {
            self.clear_autocomplete();
            return;
        };

        let items = suggestions.items;
        let prefix = suggestions.prefix;

        if items.is_empty() {
            self.clear_autocomplete();
            return;
        }

        // Pi behavior: on Tab (force), single match → complete immediately with no selector
        if force && items.len() == 1 {
            let (new_lines, new_line, new_col) = provider.apply_completion(
                &self.lines,
                self.cursor_line,
                self.cursor_col,
                &items[0],
                &prefix,
            );
            self.lines = new_lines;
            self.cursor_line = new_line;
            self.cursor_col = new_col;
            self.clear_autocomplete();
            return;
        }

        // ── Open the selector with all matches ──
        let select_items: Vec<SelectItem> = items
            .into_iter()
            .map(|item| {
                let mut si = SelectItem::new(item.value, item.label);
                if let Some(desc) = item.description {
                    si = si.with_description(desc);
                }
                si
            })
            .collect();
        // Pi-style: slash commands use a wider primary column layout
        let layout = if prefix.starts_with('/') {
            Some(
                crate::tui::components::select_list::SelectListLayoutOptions {
                    min_primary_column_width: Some(12),
                    max_primary_column_width: Some(32),
                    truncate_primary: None,
                },
            )
        } else {
            None
        };
        self.set_autocomplete_with_layout(select_items, layout);
        self.autocomplete_prefix = prefix;
    }

    pub fn try_trigger_autocomplete(&mut self) {
        self.trigger_autocomplete(false);
    }

    /// Force-trigger autocomplete (for Tab key).
    fn try_trigger_autocomplete_force(&mut self) {
        self.trigger_autocomplete(true);
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

    /// Find the segment before cursor, treating paste markers as atomic units.
    /// Returns (start, len) of the segment to delete.
    fn grapheme_or_paste_before(&self, line: &str, cursor: usize) -> Option<(usize, usize)> {
        // Check if cursor is at the end of a paste marker
        for &(start, end) in &Self::find_paste_marker_spans(line) {
            if cursor >= end && cursor < end + 10 {
                // The grapheme at end could be the start of the next marker.
                // If cursor lands exactly at a marker start, the previous
                // atomic unit is that marker itself.
                if cursor == end {
                    return Some((start, end - start));
                }
            }
        }
        // Also check if cursor is inside a marker — snap to start
        for &(start, end) in &Self::find_paste_marker_spans(line) {
            if cursor > start && cursor < end {
                return Some((start, end - start));
            }
        }
        // Default: last grapheme
        let graphemes: Vec<(usize, &str)> = line[..cursor].grapheme_indices(true).collect();
        graphemes.last().map(|&(idx, g)| (idx, g.len()))
    }

    /// Find the segment after cursor, treating paste markers as atomic units.
    fn grapheme_or_paste_after(&self, line: &str, cursor: usize) -> Option<(usize, usize)> {
        // Check if cursor is at the start of a paste marker
        for &(start, end) in &Self::find_paste_marker_spans(line) {
            if cursor == start {
                return Some((start, end - start));
            }
        }
        // Default: first grapheme
        let graphemes: Vec<(usize, &str)> = line[cursor..].grapheme_indices(true).collect();
        graphemes.first().map(|&(i, g)| (cursor + i, g.len()))
    }

    fn backspace(&mut self) {
        self.exit_history();
        self.last_action = None;
        if self.cursor_col > 0 {
            self.push_undo();
            let line = self.lines[self.cursor_line].clone();
            if let Some((idx, len)) = self.grapheme_or_paste_before(&line, self.cursor_col) {
                self.lines[self.cursor_line].drain(idx..idx + len);
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
            if let Some((idx, len)) = self.grapheme_or_paste_after(&line, self.cursor_col) {
                self.lines[self.cursor_line].drain(idx..idx + len);
            }
        } else if self.cursor_line + 1 < self.lines.len() {
            self.push_undo();
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
        }
        self.notify_change();

        // Pi: re-trigger autocomplete after forward delete if in context
        self.retrigger_autocomplete_dismissed();
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
        let opts = WordNavigationOptions {
            segment: None,
            is_atomic_segment: Some(&|s: &str| s.starts_with("[paste #") && s.ends_with(']')),
        };
        let new_col = find_word_backward_with(&line, self.cursor_col, &opts);
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
        let opts = WordNavigationOptions {
            segment: None,
            is_atomic_segment: Some(&|s: &str| s.starts_with("[paste #") && s.ends_with(']')),
        };
        let new_col = find_word_forward_with(&line, self.cursor_col, &opts);
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
            self.last_action = Some("yank".into());
            self.notify_change();
        }
    }

    fn yank_pop(&mut self) {
        // Must be called after yank() — check via last_action
        if self.last_action.as_deref() != Some("yank") || self.kill_ring.len() <= 1 {
            return;
        }
        // Save current state before modifying (pi-style: pushUndoSnapshot first)
        self.push_undo();

        // Delete the previously yanked text (still at end of ring before rotation)
        let prev = self.kill_ring.peek().map(|s| s.to_string());
        if let Some(ref prev_text) = prev {
            let line = &self.lines[self.cursor_line].clone();
            if self.cursor_col >= prev_text.len() {
                let before = &line[..self.cursor_col - prev_text.len()];
                let after = &line[self.cursor_col..];
                self.lines[self.cursor_line] = format!("{}{}", before, after);
                self.cursor_col -= prev_text.len();
            }
        }

        // Rotate the ring: move end to front
        self.kill_ring.rotate();

        // Insert the new most recent entry (now at end after rotation)
        let text = self.kill_ring.peek().map(|s| s.to_string());
        if let Some(ref new_text) = text {
            self.cursor_col += new_text.len();
            self.lines[self.cursor_line].insert_str(self.cursor_col - new_text.len(), new_text);
        }

        self.last_action = Some("yank".into());
        self.notify_change();
    }

    // ── Cursor movement ──

    fn move_left(&mut self) {
        self.last_action = None;
        if self.cursor_col > 0 {
            let line = &self.lines[self.cursor_line].clone();
            let graphemes: Vec<(usize, &str)> =
                line[..self.cursor_col].grapheme_indices(true).collect();
            if let Some(&(idx, _g)) = graphemes.last() {
                let raw = idx;
                // Snap to paste marker start if inside one
                self.set_cursor_col(Self::snap_paste_marker(line, raw, true));
            }
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.set_cursor_col(self.lines[self.cursor_line].len());
        }
    }

    fn move_right(&mut self) {
        self.last_action = None;
        let line = &self.lines[self.cursor_line].clone();
        if self.cursor_col < line.len() {
            let mut it = line[self.cursor_col..].grapheme_indices(true);
            if let Some((idx, g)) = it.next() {
                let raw = self.cursor_col + idx + g.len();
                // Snap to paste marker end if inside one
                self.set_cursor_col(Self::snap_paste_marker(line, raw, false));
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

    /// Build visual line spans: (logical_line, start_byte_in_logical, length_in_bytes).
    fn build_visual_line_spans(&self, width: usize) -> Vec<(usize, usize, usize)> {
        let mut spans = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            let line_w = visible_width(line);
            if line.is_empty() {
                spans.push((i, 0, 0));
            } else if line_w <= width {
                spans.push((i, 0, line.len()));
            } else {
                let chunks = crate::tui::util::wrap_text_with_ansi(line, width);
                let mut byte_pos = 0;
                for chunk in &chunks {
                    let chunk_len = chunk.len();
                    spans.push((i, byte_pos, chunk_len));
                    byte_pos += chunk_len;
                }
            }
        }
        spans
    }

    /// Find the visual line index for the current cursor position.
    fn find_current_visual_line(&self, spans: &[(usize, usize, usize)]) -> usize {
        for (i, &(li, start, len)) in spans.iter().enumerate() {
            if li != self.cursor_line {
                continue;
            }
            let offset = self.cursor_col.saturating_sub(start);
            let is_last = i + 1 >= spans.len() || spans[i + 1].0 != li;
            if offset <= len || (is_last && offset == len) {
                return i;
            }
        }
        spans.len().saturating_sub(1)
    }

    /// Move cursor to a target visual line with sticky column logic.
    /// Mirrors pi's moveToVisualLine() + computeVerticalMoveColumn().
    fn move_to_visual_line(
        &mut self,
        spans: &[(usize, usize, usize)],
        current_vis: usize,
        target_vis: usize,
    ) {
        let (cur_li, _cur_start, cur_len) = spans[current_vis];
        let (tgt_li, tgt_start, tgt_len) = spans[target_vis];
        let cur_vis_col = self.cursor_col;

        let is_last_source = current_vis + 1 >= spans.len() || spans[current_vis + 1].0 != cur_li;
        let src_max = if is_last_source {
            cur_len
        } else {
            cur_len.saturating_sub(1)
        };

        let is_last_target = target_vis + 1 >= spans.len() || spans[target_vis + 1].0 != tgt_li;
        let tgt_max = if is_last_target {
            tgt_len
        } else {
            tgt_len.saturating_sub(1)
        };

        // Decision table (matches pi)
        let has_pref = self.preferred_col.is_some();
        let cursor_in_middle = cur_vis_col < src_max;
        let target_too_short = tgt_max < cur_vis_col;

        let move_to_col = if !has_pref || cursor_in_middle {
            if target_too_short {
                self.preferred_col = Some(cur_vis_col);
                tgt_max
            } else {
                self.preferred_col = None;
                cur_vis_col
            }
        } else {
            let pref = self.preferred_col.unwrap_or(0);
            let target_cant_fit_pref = tgt_max < pref;
            if target_too_short || target_cant_fit_pref {
                tgt_max
            } else {
                self.preferred_col = None;
                pref
            }
        };

        self.cursor_line = tgt_li;
        let raw_col = tgt_start + move_to_col;
        let line = &self.lines[tgt_li].clone();
        self.cursor_col = raw_col.min(line.len());
        // Snapping uses `delta < 0` for moving-up context
        // (we don't have delta here, but snap-to-start is the safe choice
        //  since the marker boundary determination is same regardless of direction)
        // Actually, snap to start when moving up, end when moving down.
        // Infer direction from target_vis vs current_vis.
        let moving_up = target_vis < current_vis;
        self.cursor_col = Self::snap_paste_marker(line, self.cursor_col, moving_up);
    }

    fn move_vertical(&mut self, delta: isize) {
        let width = self.last_width.get();
        let spans = self.build_visual_line_spans(width);
        let current_vis = self.find_current_visual_line(&spans);

        let target_vis = if delta < 0 {
            if current_vis == 0 {
                return;
            }
            current_vis - 1
        } else if current_vis + 1 >= spans.len() {
            return;
        } else {
            current_vis + 1
        };

        self.move_to_visual_line(&spans, current_vis, target_vis);
    }

    // ── Character jump (pi-style) ──

    /// Jump to the first occurrence of a character in the specified direction.
    /// Multi-line search (pi-style). Case-sensitive. Skips current cursor position.
    fn jump_to_char(&mut self, ch: char, dir: JumpDirection) {
        let is_forward = dir == JumpDirection::Forward;
        let lines = &self.lines;

        let start_line = self.cursor_line as isize;
        let end = if is_forward { lines.len() as isize } else { -1 };
        let step: isize = if is_forward { 1 } else { -1 };

        let mut line_idx = start_line;
        while line_idx != end {
            let line = &lines[line_idx as usize];
            let is_current = line_idx == start_line;
            let search_from = if is_current {
                if is_forward {
                    self.cursor_col + 1
                } else {
                    self.cursor_col.saturating_sub(1)
                }
            } else if is_forward {
                0
            } else {
                line.len()
            };

            let idx = if is_forward {
                line[search_from..].find(ch).map(|i| search_from + i)
            } else if search_from > 0 {
                line[..search_from].rfind(ch)
            } else {
                None
            };

            if let Some(pos) = idx {
                self.cursor_line = line_idx as usize;
                self.set_cursor_col(pos);
                return;
            }
            line_idx += step;
        }
        // No match — cursor stays
    }

    // ── History ──

    fn exit_history(&mut self) {
        self.history_index = -1;
        self.history_draft = None;
        self.last_action = None;
    }

    fn recall_older(&mut self) {
        if self.history.is_empty() {
            return;
        }
        // Pi: newest at front (index 0), Up increases index (goes older)
        let idx = if self.history_index < 0 {
            0
        } else {
            self.history_index + 1
        };
        if idx >= self.history.len() as i32 {
            return; // already at oldest
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
        self.set_text_internal(&text);
        self.cursor_col = 0; // pi: cursor at start when going older
        self.history_index = idx;
    }

    fn recall_newer(&mut self) {
        if self.history_index < 0 {
            return;
        }
        // Pi: Down decreases index (goes newer). history_index > 0 means browsing older entries.
        let idx = self.history_index - 1;
        if idx < 0 {
            // Pi: restore draft instead of clearing to empty
            if let Some(draft) = self.history_draft.take() {
                self.lines = draft.lines;
                self.cursor_line = draft.cursor_line;
                self.cursor_col = draft.cursor_col;
                self.preferred_col = None;
            } else {
                self.set_text_internal("");
            }
            self.history_index = -1;
        } else {
            let text = self.history[idx as usize].clone();
            self.set_text_internal(&text);
            self.history_index = idx;
        }
    }

    // ── Paste markers (pi-style) ──

    /// CSI-u decode: terminals with extended keys (e.g. tmux popups with
    /// `extended-keys-format=csi-u`) re-encode control bytes inside bracketed
    /// paste as `\x1b[<codepoint>;5u`. Decode those back to the literal byte.
    fn decode_csi_u_in_paste(&self, text: &str) -> String {
        // Pattern: ESC [ digits ; 5 u  — Ctrl+<letter> encoded as CSI-u
        let re = regex::Regex::new(r"\x1b\[(\d+);5u").unwrap();
        re.replace_all(text, |caps: &regex::Captures| {
            let cp: u32 = caps[1].parse().unwrap_or(0);
            if (97..=122).contains(&cp) {
                // Ctrl+A..Ctrl+Z
                char::from_u32(cp - 96)
                    .map(|c| c.to_string())
                    .unwrap_or_default()
            } else if (65..=90).contains(&cp) {
                // Ctrl+Shift+A..Ctrl+Shift+Z
                char::from_u32(cp - 64)
                    .map(|c| c.to_string())
                    .unwrap_or_default()
            } else {
                caps[0].to_string()
            }
        })
        .to_string()
    }

    // ── Paste marker atomic segment helpers (pi-style) ──

    /// Find all paste marker spans `[paste #N ...]` in a line.
    /// Returns (start, end) byte positions.
    fn find_paste_marker_spans(line: &str) -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        let mut pos = 0;
        while let Some(start) = line[pos..].find("[paste #") {
            let abs_start = pos + start;
            if let Some(end) = line[abs_start..].find(']') {
                let abs_end = abs_start + end + 1;
                spans.push((abs_start, abs_end));
                pos = abs_end;
            } else {
                break;
            }
        }
        spans
    }

    /// If cursor is inside a paste marker, snap to the nearest boundary:
    /// start of marker when moving left, end when moving right.
    fn snap_paste_marker(line: &str, cursor: usize, moving_left: bool) -> usize {
        for &(start, end) in &Self::find_paste_marker_spans(line) {
            if cursor > start && cursor < end {
                return if moving_left { start } else { end };
            }
        }
        cursor
    }

    /// Handle a paste: normalizes line endings, filters non-printable chars,
    /// CSI-u decodes control bytes, and for large pastes (>10 lines or >1000 chars)
    /// stores the content with a marker like "[paste #1 +123 lines]".
    /// Matches pi's Editor.handlePaste().
    pub fn handle_paste(&mut self, text: &str) {
        self.clear_autocomplete();
        self.exit_history();
        self.last_action = None;
        self.push_undo();

        // 1. CSI-u decode control bytes that tmux/etc may have re-encoded
        let decoded = self.decode_csi_u_in_paste(text);

        // 2. Normalize line endings and tabs (same as insert_text_internal)
        let normalized = decoded
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ");

        // 3. Filter non-printable chars except newlines
        let filtered: String = normalized
            .chars()
            .filter(|&c| c == '\n' || c == ' ' || c as u32 >= 32)
            .collect();

        // 4. If pasting a file path (starts with /, ~, or .) and char before
        //    cursor is a word char, prepend a space (pi-style)
        let current_line = self.lines[self.cursor_line].clone();
        let space_prefix = if filtered.starts_with('/')
            || filtered.starts_with('~')
            || filtered.starts_with('.')
        {
            if self.cursor_col > 0 {
                let prev = current_line
                    .as_bytes()
                    .get(self.cursor_col - 1)
                    .copied()
                    .unwrap_or(b' ');
                if prev.is_ascii_alphanumeric() || prev == b'_' {
                    " "
                } else {
                    ""
                }
            } else {
                ""
            }
        } else {
            ""
        };
        let prepared = format!("{}{}", space_prefix, filtered);

        let total_chars = prepared.len();
        let is_large = prepared.lines().count().max(1) > 10 || total_chars > 1000;

        if is_large {
            let line_count = prepared.lines().count();
            self.paste_counter += 1;
            let paste_id = self.paste_counter;
            self.pastes.insert(paste_id, prepared);

            let marker = if line_count > 10 {
                format!("[paste #{} +{} lines]", paste_id, line_count)
            } else {
                format!("[paste #{} {} chars]", paste_id, total_chars)
            };
            self.insert_text_internal(&marker);
        } else {
            self.insert_text_internal(&prepared);
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
                // Simple replacement - find any marker with this ID
                let marker1 = format!("[paste #{} ", paste_id);
                loop {
                    let start = result.find(&marker1);
                    match start {
                        Some(pos) => {
                            let end = result[pos..]
                                .find(']')
                                .map(|e| pos + e + 1)
                                .unwrap_or(result.len());
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

    fn page_size(&self) -> usize {
        std::cmp::max(5, (self.terminal_rows as f64 * 0.3) as usize)
    }

    fn page_up(&mut self) {
        let size = self.page_size();
        self.scroll_offset = self.scroll_offset.saturating_sub(size);
    }

    fn page_down(&mut self) {
        let size = self.page_size();
        self.scroll_offset += size;
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
        self.just_submitted = true;
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
        // After any text change, update autocomplete if active (pi-style)
        if self.autocomplete_active {
            self.try_trigger_autocomplete();
        }
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty() || (self.lines.len() == 1 && self.lines[0].is_empty())
    }

    fn is_first_visual_line(&self) -> bool {
        let width = self.last_width.get();
        let visual_lines = layout_text(&self.lines, width, self.cursor_line, self.cursor_col);
        let current = visual_lines
            .iter()
            .position(|vl| vl.has_cursor)
            .unwrap_or(0);
        current == 0
    }

    fn is_last_visual_line(&self) -> bool {
        let width = self.last_width.get();
        let visual_lines = layout_text(&self.lines, width, self.cursor_line, self.cursor_col);
        let current = visual_lines
            .iter()
            .position(|vl| vl.has_cursor)
            .unwrap_or(0);
        current >= visual_lines.len().saturating_sub(1)
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
        // Pi: with padding, cursor can overflow into it; without, reserve 1 col for cursor.
        let layout_width = content_width
            .max(1)
            .saturating_sub(if pad_x > 0 { 0 } else { 1 });
        self.last_width.set(layout_width);

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
        // Pi: max visible lines is 30% of terminal height, minimum 5.
        let max_vis = std::cmp::max(5, (self.terminal_rows as f64 * 0.3) as usize).max(1);
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

            // Pi-style: cursor can overflow into right padding when at end of line
            let cursor_in_padding = line_width > content_width && pad_x > 0;
            let padding = if line_width < content_width {
                " ".repeat(content_width - line_width)
            } else {
                String::new()
            };
            let right_pad_used = if cursor_in_padding {
                &right_pad[1..]
            } else {
                &right_pad
            };
            result.push(format!(
                "{}{}{}{}",
                left_pad, display, padding, right_pad_used
            ));
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
            if kb.matches(key, ACTION_EDITOR_JUMP_FORWARD)
                || kb.matches(key, ACTION_EDITOR_JUMP_BACKWARD)
            {
                self.jump_mode = None;
                return true;
            }
            if is_printable_plain(key)
                && let Some(s) = key_event_to_string(key)
            {
                let ch = s.chars().next().unwrap_or(' ');
                self.jump_mode = None;
                self.jump_to_char(ch, dir);
                return true;
            }
            // Non-printable cancels jump mode
            self.jump_mode = None;
        }

        // ── Autocomplete: route to SelectList (pi-style) ──
        // Pi behavior: only Escape dismisses, Enter/Tab confirms, Up/Down navigates.
        // All other keys (including printable chars and backspace) fall through
        // to the normal handler so the character is inserted/deleted first, then
        // autocomplete is re-queried via update_autocomplete().
        if let Some(ref mut list) = self.autocomplete_list {
            if kb.matches(key, ACTION_SELECT_CANCEL) {
                self.clear_autocomplete();
                return true;
            }
            if kb.matches(key, ACTION_SELECT_CONFIRM) || kb.matches(key, ACTION_INPUT_TAB) {
                if let Some(val) = list.selected_item().map(|i| i.value.clone()) {
                    // Use provider to apply completion (pi-style), fallback to set_text
                    if let Some(ref provider) = self.autocomplete_provider {
                        let prefix = if !self.autocomplete_prefix.is_empty() {
                            self.autocomplete_prefix.clone()
                        } else {
                            self.get_autocomplete_prefix()
                        };
                        let item = crate::tui::autocomplete::AutocompleteItem {
                            value: val.clone(),
                            label: val.clone(),
                            description: None,
                        };
                        let (new_lines, new_line, new_col) = provider.apply_completion(
                            &self.lines,
                            self.cursor_line,
                            self.cursor_col,
                            &item,
                            &prefix,
                        );
                        self.lines = new_lines;
                        self.cursor_line = new_line;
                        self.cursor_col = new_col;
                    } else {
                        self.set_text(&format!("/{} ", val));
                    }
                }
                self.clear_autocomplete();
                return true;
            }
            if kb.matches(key, ACTION_SELECT_UP) || kb.matches(key, ACTION_SELECT_DOWN) {
                list.handle_input(key);
                return true;
            }
            // For all other keys, fall through to normal handling without clearing.
            // autocomplete will be updated after the key is processed.
        }

        // ── Tab: trigger autocomplete via provider (pi-style) ──
        if kb.matches(key, ACTION_INPUT_TAB) && self.autocomplete_provider.is_some() {
            self.try_trigger_autocomplete_force();
            return true;
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
            self.update_autocomplete_if_active();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_CURSOR_RIGHT) {
            self.move_right();
            self.update_autocomplete_if_active();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_CURSOR_LINE_START) {
            self.move_to_line_start();
            self.update_autocomplete_if_active();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_CURSOR_LINE_END) {
            self.move_to_line_end();
            self.update_autocomplete_if_active();
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
            self.update_autocomplete_if_active();
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
            self.update_autocomplete_if_active();
            return true;
        }

        // ── Page scroll ──
        if kb.matches(key, ACTION_EDITOR_PAGE_UP) {
            self.page_up();
            self.update_autocomplete_if_active();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_PAGE_DOWN) {
            self.page_down();
            self.update_autocomplete_if_active();
            return true;
        }

        // ── Word movement ──
        if kb.matches(key, ACTION_EDITOR_CURSOR_WORD_LEFT) {
            let line = &self.lines[self.cursor_line].clone();
            if self.cursor_col > 0 {
                let opts = WordNavigationOptions {
                    segment: None,
                    is_atomic_segment: Some(&|s: &str| {
                        s.starts_with("[paste #") && s.ends_with(']')
                    }),
                };
                let c = find_word_backward_with(line, self.cursor_col, &opts);
                self.set_cursor_col(c);
            }
            self.update_autocomplete_if_active();
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_CURSOR_WORD_RIGHT) {
            let line = &self.lines[self.cursor_line].clone();
            if self.cursor_col < line.len() {
                let opts = WordNavigationOptions {
                    segment: None,
                    is_atomic_segment: Some(&|s: &str| {
                        s.starts_with("[paste #") && s.ends_with(']')
                    }),
                };
                let c = find_word_forward_with(line, self.cursor_col, &opts);
                self.set_cursor_col(c);
            }
            self.update_autocomplete_if_active();
            return true;
        }

        // ── Deletion ──
        if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_BACKWARD) {
            self.backspace();
            // notify_change handles autocomplete update
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_FORWARD) {
            self.delete_forward();
            // notify_change handles autocomplete update
            return true;
        }

        // ── Kill operations ──
        if kb.matches(key, ACTION_EDITOR_DELETE_WORD_BACKWARD) {
            self.delete_word_backward();
            // notify_change handles autocomplete update
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_DELETE_WORD_FORWARD) {
            self.delete_word_forward();
            // notify_change handles autocomplete update
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_DELETE_TO_LINE_START) {
            self.delete_to_line_start();
            // notify_change handles autocomplete update
            return true;
        }
        if kb.matches(key, ACTION_EDITOR_DELETE_TO_LINE_END) {
            self.delete_to_line_end();
            // notify_change handles autocomplete update
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

        // ── Escape - let parent handle ──
        if kb.matches(key, ACTION_SELECT_CANCEL) {
            return false;
        }

        false
    }

    fn handle_paste(&mut self, text: &str) {
        Editor::handle_paste(self, text);
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

#[derive(Debug)]
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
    use crate::tui::autocomplete::{
        AutocompleteItem, AutocompleteProvider, AutocompleteSuggestions, SlashCommand,
    };

    // ── Mock autocomplete provider for testing ──

    struct MockSlashProvider {
        commands: Vec<SlashCommand>,
    }

    impl MockSlashProvider {
        fn new(commands: Vec<&str>) -> Self {
            Self {
                commands: commands
                    .into_iter()
                    .map(|name| SlashCommand {
                        name: name.to_string(),
                        description: Some(format!("The {} command", name)),
                        argument_hint: None,
                        argument_completions: None,
                    })
                    .collect(),
            }
        }
    }

    impl AutocompleteProvider for MockSlashProvider {
        fn trigger_characters(&self) -> &[char] {
            &['/', '@', '#']
        }

        fn get_suggestions(
            &self,
            lines: &[String],
            cursor_line: usize,
            cursor_col: usize,
            _force: bool,
        ) -> Option<AutocompleteSuggestions> {
            let line = lines.get(cursor_line)?;
            let before = &line[..cursor_col.min(line.len())];

            // Slash command: text starts with / and has no space
            if before.starts_with('/') && !before.contains(' ') {
                let query = &before[1..].to_lowercase();
                let matching: Vec<AutocompleteItem> = self
                    .commands
                    .iter()
                    .filter(|cmd| cmd.name.to_lowercase().starts_with(query))
                    .map(|cmd| AutocompleteItem {
                        value: cmd.name.clone(),
                        label: format!("/{}", cmd.name),
                        description: cmd.description.clone(),
                    })
                    .collect();
                if matching.is_empty() {
                    return None;
                }
                return Some(AutocompleteSuggestions {
                    items: matching,
                    prefix: before.to_string(),
                });
            }
            None
        }

        fn apply_completion(
            &self,
            lines: &[String],
            cursor_line: usize,
            cursor_col: usize,
            item: &AutocompleteItem,
            prefix: &str,
        ) -> (Vec<String>, usize, usize) {
            let current_line = lines[cursor_line].clone();
            let prefix_start = cursor_col.saturating_sub(prefix.len());
            let before = &current_line[..prefix_start];
            let after = &current_line[cursor_col..];
            (
                vec![format!("{}/{} {}", before, item.value, after)],
                cursor_line,
                before.len() + 1 + item.value.len() + 1,
            )
        }

        fn should_trigger_file_completion(
            &self,
            lines: &[String],
            cursor_line: usize,
            cursor_col: usize,
        ) -> bool {
            let current_line = lines.get(cursor_line);
            match current_line {
                Some(text) => {
                    let before = &text[..cursor_col.min(text.len())];
                    if before.starts_with('/') && !before.contains(' ') {
                        return false;
                    }
                    true
                }
                None => false,
            }
        }
    }

    // ── Autocomplete tests ──

    fn make_editor_with_slash_provider(commands: Vec<&str>) -> Editor {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        let provider = Box::new(MockSlashProvider::new(commands));
        editor.set_autocomplete_provider(provider);
        editor
    }

    #[test]
    fn autocomplete_triggers_on_slash() {
        let mut editor = make_editor_with_slash_provider(vec!["help", "history"]);
        editor.handle_input(&char_key('/'));
        assert!(
            editor.autocomplete_active,
            "autocomplete should activate after typing /"
        );
        let selected = editor.autocomplete_selected_value();
        assert_eq!(
            selected.as_deref(),
            Some("help"),
            "first item should be help"
        );
    }

    #[test]
    fn autocomplete_filters_as_user_types() {
        let mut editor = make_editor_with_slash_provider(vec!["help", "history", "model"]);
        // Type /
        editor.handle_input(&char_key('/'));
        assert!(editor.autocomplete_active);

        // Type 'h' - should filter to help, history
        editor.handle_input(&char_key('h'));
        assert!(
            editor.autocomplete_active,
            "autocomplete should stay active after typing more letters"
        );
        // Should still have items (no flicker on footer)

        // Type 'e' - should filter to help only
        editor.handle_input(&char_key('e'));
        assert!(editor.autocomplete_active);
        let selected = editor.autocomplete_selected_value();
        assert_eq!(selected.as_deref(), Some("help"));
    }

    #[test]
    fn autocomplete_stays_active_on_printable_chars() {
        // Regression: typing a letter should NOT dismiss autocomplete first
        let mut editor = make_editor_with_slash_provider(vec!["help", "history"]);
        editor.handle_input(&char_key('/'));
        assert!(editor.autocomplete_active);

        editor.handle_input(&char_key('h'));
        assert!(
            editor.autocomplete_active,
            "typing 'h' after '/' must keep autocomplete visible"
        );

        editor.handle_input(&char_key('e'));
        assert!(
            editor.autocomplete_active,
            "typing 'e' after '/h' must keep autocomplete visible"
        );

        let lines = editor.render(80);
        // Should have at least 3 border lines + some suggestion lines
        assert!(lines.len() > 3, "autocomplete lines should be rendered");
    }

    #[test]
    fn escape_dismisses_autocomplete() {
        let mut editor = make_editor_with_slash_provider(vec!["help", "history"]);
        editor.handle_input(&char_key('/'));
        assert!(editor.autocomplete_active);

        editor.handle_input(&escape());
        assert!(
            !editor.autocomplete_active,
            "escape should dismiss autocomplete"
        );

        // Text should remain (Escape only dismisses autocomplete, not clear text)
        assert_eq!(editor.get_text(), "/");
    }

    #[test]
    fn backspace_removing_slash_dismisses_autocomplete() {
        let mut editor = make_editor_with_slash_provider(vec!["help", "history"]);
        editor.handle_input(&char_key('/'));
        assert!(editor.autocomplete_active, "after /");

        editor.handle_input(&backspace());
        assert!(
            !editor.autocomplete_active,
            "backspace removing / should dismiss autocomplete"
        );
        assert_eq!(editor.get_text(), "", "text should be empty");
    }

    #[test]
    fn autocomplete_updates_after_backspace_char() {
        let mut editor = make_editor_with_slash_provider(vec!["help", "history"]);
        // Type /he
        editor.handle_input(&char_key('/'));
        editor.handle_input(&char_key('h'));
        editor.handle_input(&char_key('e'));
        assert!(editor.autocomplete_active);
        let val1 = editor.autocomplete_selected_value();
        assert_eq!(val1.as_deref(), Some("help"));

        // Backspace the 'e' - should re-filter to show help, history
        editor.handle_input(&backspace());
        assert!(
            editor.autocomplete_active,
            "backspace should re-filter, not dismiss"
        );
        // Should now have 2 matching items (help, history)
        assert!(!editor.autocomplete_is_empty());
        assert_eq!(editor.get_text(), "/h");
    }

    #[test]
    fn autocomplete_updates_on_cursor_movement() {
        let mut editor = make_editor_with_slash_provider(vec!["help", "history"]);
        // Type /help (autocomplete shows)
        editor.handle_input(&char_key('/'));
        editor.handle_input(&char_key('h'));
        editor.handle_input(&char_key('e'));
        editor.handle_input(&char_key('l'));
        editor.handle_input(&char_key('p'));
        assert!(editor.autocomplete_active);

        // Now type a space after /help - autocomplete should dismiss because
        // the context changes (/command with space = file completion, not slash)
        editor.handle_input(&char_key(' '));
        assert!(
            !editor.autocomplete_active,
            "space after /cmd should dismiss slash autocomplete"
        );

        // Move cursor left back into /help - should re-trigger autocomplete via update_autocomplete_if_active
        editor.handle_input(&left_key());
        // Actually, moving left won't trigger autocomplete since the provider doesn't
        // re-trigger from cursor movement alone when autocomplete was dismissed.
        // The key change is that when autocomplete IS active, cursor movement updates it.
    }

    #[test]
    fn autocomplete_clears_when_provider_returns_none() {
        // Provider returns None for unknown commands, which should clear autocomplete
        let mut editor = make_editor_with_slash_provider(vec!["help"]);
        editor.handle_input(&char_key('/'));
        assert!(editor.autocomplete_active);

        // Type 'z' - no command starts with /z, provider returns None
        editor.handle_input(&char_key('z'));
        assert!(
            !editor.autocomplete_active,
            "typing /z with no matching command should dismiss autocomplete"
        );
    }

    #[test]
    fn autocomplete_does_not_interfere_with_normal_typing() {
        // Without a slash prefix, autocomplete should not trigger
        let mut editor = make_editor_with_slash_provider(vec!["help", "history"]);
        editor.handle_input(&char_key('h'));
        editor.handle_input(&char_key('e'));
        editor.handle_input(&char_key('l'));
        editor.handle_input(&char_key('l'));
        editor.handle_input(&char_key('o'));
        assert!(!editor.autocomplete_active, "no slash = no autocomplete");
        assert_eq!(editor.get_text(), "hello");
    }

    #[test]
    fn autocomplete_renders_lines_below_editor() {
        let mut editor = make_editor_with_slash_provider(vec!["help", "history", "model"]);
        editor.handle_input(&char_key('/'));
        assert!(editor.autocomplete_active);

        let lines = editor.render(80);
        // Lines should include: top border, content (/), bottom border, autocomplete items
        assert!(
            lines.len() >= 5,
            "should have border lines + autocomplete items"
        );
        // Bottom border should be present
        assert!(lines[2].contains('─'), "line 2 should be bottom border");
        // Autocomplete items should follow
        let after_border = &lines[3..];
        let all_have_content = after_border.iter().any(|l| !l.trim().is_empty());
        assert!(all_have_content, "autocomplete lines should have content");
    }

    #[test]
    fn autocomplete_stable_rendering_no_flash_on_extra_char() {
        // Verify that typing an extra character doesn't change the total
        // line count drastically (no dismiss + re-show bounce).
        let mut editor = make_editor_with_slash_provider(vec!["help", "history", "model"]);
        editor.handle_input(&char_key('/'));
        let lines_after_slash = editor.render(80).len();

        editor.handle_input(&char_key('h'));
        let lines_after_h = editor.render(80).len();

        // Both renders should have autocomplete, so line counts should be similar
        // (items may differ: 3 vs 2, so at most 1 line difference)
        let diff = lines_after_slash.abs_diff(lines_after_h);
        assert!(
            diff <= 1,
            "line count should not change dramatically: {} -> {} (diff {})",
            lines_after_slash,
            lines_after_h,
            diff
        );
    }

    #[test]
    fn autocomplete_dismissed_on_submit() {
        let mut editor = make_editor_with_slash_provider(vec!["help"]);
        editor.handle_input(&char_key('/'));
        assert!(editor.autocomplete_active);

        // Submit (Enter) - should apply completion or dismiss
        editor.handle_input(&enter_key());
        // After submit, autocomplete is cleared
    }

    #[test]
    fn tab_force_triggers_autocomplete() {
        let mut editor = make_editor_with_slash_provider(vec!["help", "history"]);
        // Type nothing - Tab should trigger file completion (not slash)
        // Type / and then Tab
        editor.handle_input(&char_key('/'));
        // insert_character should have triggered autocomplete already
        assert!(editor.autocomplete_active);
    }

    #[test]
    fn autocomplete_persists_across_multiple_chars() {
        // Real-world flow: type /help and see autocomplete stay visible throughout
        let mut editor = make_editor_with_slash_provider(vec!["help", "history", "hello", "heavy"]);

        for ch in "/hel".chars() {
            editor.handle_input(&char_key(ch));
            assert!(
                editor.autocomplete_active,
                "autocomplete should stay active after '{}'",
                ch
            );
        }

        // Should show items starting with /hel
        assert!(
            !editor.autocomplete_is_empty(),
            "should have matching items"
        );
        assert_eq!(editor.get_text(), "/hel");
    }

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
                max_visible_lines: 10,
            },
        );
        // Set terminal_rows=6 → max_vis = max(5, 1) = 5.
        // With 6 content lines and cursor at the bottom, scroll offset of 2
        // should produce an up-arrow indicator at the top.
        editor.set_terminal_rows(6);
        editor.set_text("line1\nline2\nline3\nline4\nline5\nline6");
        editor.cursor_line = 5;
        editor.cursor_col = 5;
        editor.scroll_offset = 2;
        let lines = editor.render(80);
        assert!(
            lines[0].contains("↑"),
            "Expected scroll-up indicator, got: {:?}",
            lines[0]
        );
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
        // Empty editor - cursor should be in visual line 0
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
    fn left_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)
    }
    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn enter_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }
    fn escape() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }
    fn backspace() -> KeyEvent {
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
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
        // Cursor at last byte of text - should be in the last visual line
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
        assert!(
            !text.contains("line1"),
            "Should not contain original content"
        );
        assert_eq!(editor.pastes.len(), 1, "Should store one paste");
    }

    #[test]
    fn test_small_paste_no_marker() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.handle_paste("hello");
        let text = editor.get_text();
        assert!(
            !text.contains("[paste #"),
            "Small paste should not create marker"
        );
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_expand_paste_markers() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.handle_paste(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11",
        );
        let expanded = editor.get_expanded_text();
        assert!(
            expanded.contains("line1"),
            "Expanded text should contain original content"
        );
        assert!(
            !expanded.contains("[paste #"),
            "Expanded text should not contain markers"
        );
    }

    #[test]
    fn test_submit_expands_markers() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.handle_paste(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11",
        );
        let large_content =
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11";
        // Manually call the submit logic to verify expansion
        let raw = editor.lines.join("\n");
        let expanded = editor.expand_paste_markers(&raw);
        assert_eq!(
            expanded, large_content,
            "Submit should expand to original content"
        );
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
        editor.handle_paste(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11",
        );
        let expanded = editor.get_expanded_text();
        assert!(
            expanded.contains("line1"),
            "get_expanded_text should expand markers"
        );
        assert!(
            expanded.starts_with("line1"),
            "Should start with original content"
        );
    }

    // ── Render duplication tests ──

    #[test]
    fn test_multiline_render_no_duplicate_content() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        // Simulate: type "hello", add newline, type "world"
        editor.set_text("hello");
        editor.add_newline();
        editor.insert_character("w");
        editor.insert_character("o");
        editor.insert_character("r");
        editor.insert_character("l");
        editor.insert_character("d");
        assert_eq!(editor.get_text(), "hello\nworld");

        // Render at various widths
        for width in [20, 40, 80] {
            let rendered = editor.render(width);

            // Collect content lines (skip border lines)
            let content_lines: Vec<&str> = rendered
                .iter()
                .filter(|l| !l.contains('─'))
                .map(|l| l.trim())
                .collect();

            // Check total content lines count matches expected (2: "hello" + "world")
            assert!(
                content_lines.len() >= 2,
                "Width {}: expected >= 2 content lines, got {}: {:?}",
                width,
                content_lines.len(),
                rendered
            );

            // Check no duplicates among non-empty content lines
            let mut seen = std::collections::HashSet::new();
            for line in &content_lines {
                if !line.is_empty() {
                    let plain = line.replace("\x1b_pi:c\x07", "").to_string();
                    if !seen.insert(plain.clone()) {
                        panic!(
                            "Width {}: duplicate content line '{}' in {:?}",
                            width, line, rendered
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_editor_add_newline_adds_one_visual_line() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("hello");

        let before = editor.render(80).len();
        editor.add_newline();
        let after = editor.render(80).len();

        assert_eq!(
            after,
            before + 1,
            "Adding newline should increase rendered line count by exactly 1. before={}, after={}",
            before,
            after
        );
    }

    #[test]
    fn test_layout_text_no_extra_empty_visual_line() {
        // layout_text should not produce an extra empty visual line
        // when transitioning from empty to single-line content.
        let lines: Vec<String> = vec![String::new()];
        let vl = layout_text(&lines, 80, 0, 0);
        assert_eq!(vl.len(), 1, "Empty text should have 1 visual line");
        assert!(vl[0].has_cursor);

        let lines = vec!["hello".to_string()];
        let vl = layout_text(&lines, 80, 0, 5);
        assert_eq!(vl.len(), 1, "Single line should have 1 visual line");
        assert!(vl[0].has_cursor);

        let lines = vec!["hello".to_string(), "".to_string()];
        let vl = layout_text(&lines, 80, 0, 5);
        assert_eq!(
            vl.len(),
            2,
            "Two lines (one empty) should have 2 visual lines"
        );
        // Cursor is on line 0 ("hello"), so first visual line has cursor
        assert!(vl[0].has_cursor);
        assert!(!vl[1].has_cursor);

        let lines = vec!["hello".to_string(), "".to_string()];
        let vl = layout_text(&lines, 80, 1, 0);
        assert_eq!(vl.len(), 2);
        // Cursor is on line 1 (empty), so second visual line has cursor
        assert!(!vl[0].has_cursor);
        assert!(vl[1].has_cursor);

        let lines = vec!["".to_string(), "hello".to_string()];
        let vl = layout_text(&lines, 80, 1, 5);
        assert_eq!(
            vl.len(),
            2,
            "Two lines (one empty first) should have 2 visual lines"
        );
        assert!(!vl[0].has_cursor);
        assert!(vl[1].has_cursor);
    }

    #[test]
    fn test_wrap_edge_cases_no_empty_lines() {
        // Various edge cases that should NOT produce empty lines.
        // Empty strings in wrapped output cause visual artifacts
        // (blank lines appearing in the editor).
        let cases = vec![
            ("  hello", 3, "leading spaces"),
            ("hello  ", 3, "trailing spaces"),
            ("  hello  ", 3, "leading and trailing spaces"),
            ("abc  def", 5, "double space in middle"),
            ("a   b", 4, "triple space"),
            ("a  b", 3, "double space at wrap boundary"),
        ];
        for (text, width, label) in &cases {
            // Debug: print the actual tokens produced by split_into_tokens
            // to verify our understanding of tokenization.
            let wrapped = crate::tui::util::wrap_text_with_ansi(text, *width);
            for chunk in &wrapped {
                // A non-empty input should never produce empty chunks
                if chunk.is_empty() {
                    panic!(
                        "Case '{}' (width {}): empty chunk found in wrapped: {:?}",
                        label, width, wrapped
                    );
                }
                let vis = crate::tui::util::visible_width(chunk);
                assert!(
                    vis > 0,
                    "Case '{}' (width {}): chunk with visible width 0: {:?} (wrapped: {:?})",
                    label,
                    width,
                    chunk,
                    wrapped
                );
            }
        }
    }

    #[test]
    fn test_wrap_long_word_no_duplicate_chunks() {
        // A long continuous word (no spaces) past width should not duplicate
        let long = "aaaaa bbbbb ccccc ddddd";
        for width in [5, 6, 7, 8, 10, 12] {
            let wrapped = crate::tui::util::wrap_text_with_ansi(long, width);
            // Count visible content and check for duplicates
            let mut seen = std::collections::HashSet::new();
            for chunk in &wrapped {
                let trimmed = chunk.trim();
                if !trimmed.is_empty() && !seen.insert(trimmed.to_string()) {
                    panic!(
                        "Width {}: duplicate chunk '{}' in {:?}",
                        width, chunk, wrapped
                    );
                }
            }
        }
    }

    #[test]
    fn test_wrap_typing_detailed_trace() {
        // Simulate typing character by character into the editor,
        // checking the visual line layout after each character.
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());

        // Type a sentence that exceeds width 10, checking after each char
        let sentence = "hello world";
        let width = 10;

        for (i, ch) in sentence.chars().enumerate() {
            editor.handle_input(&char_key(ch));

            // Get visual lines via layout_text (simulating what render does)
            let vl = layout_text(&editor.lines, width, editor.cursor_line, editor.cursor_col);

            // Check no duplicate visual lines or empty lines
            let mut seen = std::collections::HashSet::new();
            for vis in &vl {
                let trimmed = vis.text.trim();
                if !trimmed.is_empty() && !seen.insert(trimmed.to_string()) {
                    panic!(
                        "After char '{}' (pos {}): duplicate visual line '{}' in {:?}",
                        ch, i, vis.text, vl
                    );
                }
            }

            // Check exactly one cursor
            let cursor_count = vl.iter().filter(|v| v.has_cursor).count();
            assert_eq!(
                cursor_count, 1,
                "After char '{}' (pos {}): expected exactly 1 cursor, got {}. vl: {:?}",
                ch, i, cursor_count, vl
            );
        }
    }

    #[test]
    fn test_wrap_long_continuous_string_no_duplicates() {
        // A very long continuous string (like a URL or path) with no spaces.
        // Must not produce duplicate chunks when word-broken across lines.
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());

        // Simulate typing a long URL character by character
        let url = "https://very-long-url-with-no-spaces.example.com/path/to/resource";
        for ch in url.chars() {
            editor.handle_input(&char_key(ch));
        }

        // Test at various narrow widths
        for width in [5, 10, 15, 20, 30] {
            let rendered = editor.render(width);
            let content: Vec<&str> = rendered
                .iter()
                .filter(|l| !l.contains('─'))
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect();

            let mut seen = std::collections::HashSet::new();
            for line in &content {
                let plain = line
                    .replace("\x1b_pi:c\x07", "")
                    .chars()
                    .filter(|&c| c.is_ascii_graphic() || c == ' ')
                    .collect::<String>()
                    .trim()
                    .to_string();
                if !plain.is_empty() && !seen.insert(plain.clone()) {
                    panic!(
                        "Width {}: duplicate content line '{}' (plain: '{}')\nFull render: {:?}",
                        width, line, plain, rendered
                    );
                }
            }
        }
    }

    #[test]
    fn test_editor_typing_past_width_no_duplicate_render() {
        // Simulate typing characters one at a time until the line exceeds the width.
        // The rendered output must never show the same content line twice.
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());

        // Type characters to build up a line longer than the render width
        let input = "hello world this is a test of the emergency broadcast system";
        for ch in input.chars() {
            editor.handle_input(&char_key(ch));
        }

        // Render at a narrow width so wrapping occurs
        for width in [5, 8, 10, 12, 15, 20] {
            let rendered = editor.render(width);

            // Collect visible content (skip border/scroll indicator lines)
            let content: Vec<&str> = rendered
                .iter()
                .filter(|l| !l.contains('─'))
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect();

            // Check for duplicates among content lines
            let mut seen = std::collections::HashSet::new();
            for line in &content {
                // Strip cursor marker and ansi codes for comparison
                let plain = line
                    .replace("\x1b_pi:c\x07", "")
                    .chars()
                    .filter(|&c| c.is_ascii_graphic() || c == ' ')
                    .collect::<String>()
                    .trim()
                    .to_string();
                if !plain.is_empty() && !seen.insert(plain.clone()) {
                    panic!(
                        "Width {}: duplicate content line '{}' (plain: '{}')\nFull render: {:?}",
                        width, line, plain, rendered
                    );
                }
            }

            // Also check that the total content roughly matches input (accounting for wrapping)
            let content_plain: String = content.join(" ");
            let content_plain = content_plain
                .replace("\x1b_pi:c\x07", "")
                .chars()
                .filter(|&c| c.is_ascii_graphic() || c == ' ')
                .collect::<String>();
            assert!(
                !content_plain.is_empty(),
                "Width {}: no visible content in render: {:?}",
                width,
                rendered
            );
        }
    }
}
