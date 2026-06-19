//! Editor widget for rab TUI.
//!
//! Multi-line text editing with Emacs-style keybindings and grapheme-aware cursor.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::Block;
use unicode_segmentation::UnicodeSegmentation;

/// Info about a slash command for autocomplete.
#[derive(Clone)]
pub struct SlashCommandInfo {
    pub name: String,
    pub description: String,
}

/// Info about an autocomplete item.
#[derive(Clone, Debug)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// What the editor rendered, returned by `render()`.
pub struct EditorRender {
    pub text_lines: Vec<String>,
    pub cursor_col: u16,
    pub cursor_row: u16,
    pub autocomplete_lines: Vec<String>,
    pub autocomplete_selection: usize,
    pub autocomplete_active: bool,
}

// ── Editor ─────────────────────────────────────────────────────────

struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
}

pub struct Editor {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
    block: Block<'static>,
    /// Prompt history (oldest-first).
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<EditorSnapshot>,
    /// Slash commands for autocomplete.
    slash_commands: Vec<SlashCommandInfo>,
    /// CWD for @ file path completion.
    cwd: std::path::PathBuf,
    /// Autocomplete state.
    autocomplete: Option<AutocompleteState>,
}

#[derive(Clone)]
struct EditorSnapshot {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

#[derive(Clone)]
struct AutocompleteState {
    items: Vec<AutocompleteItem>,
    selected: usize,
    prefix: String,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            block: Block::default(),
            history: Vec::new(),
            history_index: None,
            history_draft: None,
            slash_commands: Vec::new(),
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            autocomplete: None,
        }
    }

    pub fn set_slash_commands(&mut self, commands: Vec<SlashCommandInfo>) {
        self.slash_commands = commands;
    }

    pub fn set_cwd(&mut self, cwd: std::path::PathBuf) {
        self.cwd = cwd;
    }

    pub fn set_block(&mut self, block: Block<'static>) {
        self.block = block;
    }

    pub fn block(&self) -> &Block<'static> {
        &self.block
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn raw_text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn expanded_text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines_raw(&self) -> &[String] {
        &self.lines
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    pub fn set_text(&mut self, text: &str) {
        self.autocomplete = None;
        if self.history_index.is_none() {
            self.history_draft = None;
        }
        self.lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(|s| s.to_string()).collect()
        };
        self.cursor_row = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_row].len();
    }

    pub fn autocomplete_active(&self) -> bool {
        self.autocomplete.is_some()
    }

    pub fn dismiss_autocomplete(&mut self) {
        self.autocomplete = None;
    }

    /// Number of lines the autocomplete dropdown occupies when active.
    pub fn autocomplete_line_count(&self) -> usize {
        if let Some(ref ac) = self.autocomplete {
            if ac.items.is_empty() {
                return 0;
            }
            let max = 5.min(ac.items.len());
            let has_scroll = ac.items.len() > max;
            1 + max + if has_scroll { 1 } else { 0 } // sep + items + scroll info
        } else {
            0
        }
    }

    pub fn accept_autocomplete_if_active(&mut self) -> bool {
        if let Some(ac) = &self.autocomplete
            && !ac.items.is_empty()
        {
            let item = ac.items[ac.selected].clone();
            let prefix = ac.prefix.clone();
            self.autocomplete = None;
            self.apply_autocomplete_item(&item, &prefix);
            return true;
        }
        false
    }

    pub fn set_argument_completions(&mut self, items: Vec<AutocompleteItem>) {
        if let Some(ref mut ac) = self.autocomplete {
            if items.is_empty() {
                self.autocomplete = None;
            } else {
                ac.items = items;
                ac.selected = 0;
            }
        }
    }

    // ── History ──────────────────────────────────────────────────

    pub fn add_to_history(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.history.last().map(|s| s.as_str()) == Some(trimmed) {
            return;
        }
        self.history.push(trimmed.to_string());
        if self.history.len() > 100 {
            self.history.remove(0);
        }
    }

    pub fn recall_history(&mut self, direction: isize) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let len = self.history.len();
        let current = match self.history_index {
            Some(i) => i,
            None => {
                self.history_draft = Some(self.snapshot());
                len // past-the-end sentinel
            }
        };
        let new_index = if direction < 0 {
            if current == 0 {
                return false;
            }
            current - 1
        } else if current >= len {
            return false;
        } else {
            current + 1
        };
        if new_index >= len {
            self.history_index = None;
            if let Some(draft) = self.history_draft.take() {
                self.restore_snapshot(&draft);
            } else {
                self.set_text("");
            }
        } else {
            self.history_index = Some(new_index);
            let text = self.history[new_index].clone();
            self.set_text(&text);
            self.history_index = Some(new_index); // restore after set_text clears it
        }
        true
    }

    fn snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            lines: self.lines.clone(),
            cursor_row: self.cursor_row,
            cursor_col: self.cursor_col,
        }
    }

    fn restore_snapshot(&mut self, snap: &EditorSnapshot) {
        self.lines = snap.lines.clone();
        self.cursor_row = snap.cursor_row;
        self.cursor_col = snap.cursor_col;
    }

    // ── Paste ────────────────────────────────────────────────────

    pub fn handle_paste(&mut self, text: &str) {
        // Reset state that could interfere
        self.autocomplete = None;
        self.history_index = None;
        self.history_draft = None;
        // Defensive: clamp cursor to valid range before manipulating
        self.clamp_cursor();

        let mut text = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ");

        // Filter ANSI escape sequences (CSI, OSC, etc.) that can leak
        // in when pasting terminal selection. These corrupt display width
        // calculations and rendering.
        text = Self::strip_ansi(&text);

        // Strip trailing newlines (prevents auto-submit on paste)
        let text = text.trim_end_matches('\n');
        if text.is_empty() {
            return;
        }
        // Split and insert multi-line
        let pasted: Vec<&str> = text.split('\n').collect();
        if pasted.len() == 1 {
            self.insert_str_at_cursor(text);
        } else {
            let current = self.lines[self.cursor_row].clone();
            let before = current[..self.cursor_col].to_string();
            let after = current[self.cursor_col..].to_string();
            self.lines[self.cursor_row] = before + pasted[0];
            let mut pos = self.cursor_row + 1;
            for line in &pasted[1..pasted.len() - 1] {
                self.lines.insert(pos, line.to_string());
                pos += 1;
            }
            let last = pasted[pasted.len() - 1].to_string() + &after;
            self.lines.insert(pos, last);
            self.cursor_row = pos;
            self.cursor_col = pasted[pasted.len() - 1].len();
        }
    }

    fn insert_str_at_cursor(&mut self, s: &str) {
        let line = &mut self.lines[self.cursor_row];
        line.insert_str(self.cursor_col, s);
        self.cursor_col += s.len();
    }

    // ── Key handling ─────────────────────────────────────────────

    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        self.clamp_cursor();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        // Autocomplete mode
        if self.autocomplete.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.autocomplete = None;
                    return true;
                }
                KeyCode::Up => {
                    self.update_ac_selection(-1);
                    return true;
                }
                KeyCode::Down => {
                    self.update_ac_selection(1);
                    return true;
                }
                KeyCode::Tab | KeyCode::Enter => {
                    self.accept_autocomplete_if_active();
                    return true;
                }
                _ => {
                    // Char keys keep autocomplete open for filtering
                    if !matches!(key.code, KeyCode::Char(_)) || ctrl {
                        self.autocomplete = None;
                    }
                }
            }
        }

        match key.code {
            KeyCode::Char('a') if ctrl => {
                self.cursor_col = 0;
                true
            }
            KeyCode::Char('e') if ctrl => {
                self.cursor_col = self.lines[self.cursor_row].len();
                true
            }
            KeyCode::Char('b') if ctrl => {
                self.move_left();
                true
            }
            KeyCode::Char('f') if ctrl => {
                self.move_right();
                true
            }
            KeyCode::Char('p') if ctrl => {
                self.move_up();
                true
            }
            KeyCode::Char('n') if ctrl => {
                self.move_down();
                true
            }
            KeyCode::Left if ctrl || alt => {
                self.move_word_left();
                true
            }
            KeyCode::Right if ctrl || alt => {
                self.move_word_right();
                true
            }
            KeyCode::Backspace if alt => {
                self.delete_word_left();
                true
            }
            KeyCode::Delete if alt => {
                self.delete_word_right();
                true
            }
            KeyCode::Char('k') if ctrl => {
                self.kill_to_end();
                true
            }
            KeyCode::Char('u') if ctrl => {
                self.kill_to_start();
                true
            }
            KeyCode::Char('w') if ctrl => {
                self.kill_word_left();
                true
            }
            KeyCode::Char(c) if !ctrl => {
                self.insert_char(c);
                self.update_autocomplete_after_typing();
                true
            }
            KeyCode::Backspace => {
                if self.cursor_col > 0 {
                    let line = &self.lines[self.cursor_row];
                    let new_pos = Self::grapheme_pos_relative(line, self.cursor_col, -1);
                    self.lines[self.cursor_row].drain(new_pos..self.cursor_col);
                    self.cursor_col = new_pos;
                } else if self.cursor_row > 0 {
                    let rest = self.lines[self.cursor_row].clone();
                    self.lines.remove(self.cursor_row);
                    self.cursor_row -= 1;
                    self.cursor_col = self.lines[self.cursor_row].len();
                    self.lines[self.cursor_row].push_str(&rest);
                }
                self.update_autocomplete_after_typing();
                true
            }
            KeyCode::Delete => {
                let line = &self.lines[self.cursor_row];
                if self.cursor_col < line.len() {
                    let end = Self::grapheme_pos_relative(line, self.cursor_col, 1);
                    self.lines[self.cursor_row].drain(self.cursor_col..end);
                } else if self.cursor_row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.cursor_row + 1);
                    self.lines[self.cursor_row].push_str(&next);
                }
                true
            }
            KeyCode::Left => {
                self.move_left();
                true
            }
            KeyCode::Right => {
                self.move_right();
                true
            }
            KeyCode::Up => {
                self.move_up();
                true
            }
            KeyCode::Down => {
                self.move_down();
                true
            }
            KeyCode::Home => {
                self.cursor_col = 0;
                true
            }
            KeyCode::End => {
                self.cursor_col = self.lines[self.cursor_row].len();
                true
            }
            KeyCode::Enter => {
                // Alt+Enter inserts newline; plain Enter is handled by caller
                if alt || key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.newline();
                    return true;
                }
                false // caller handles submit
            }
            KeyCode::Tab => {
                self.try_autocomplete();
                true
            }
            _ => false,
        }
    }

    /// Legacy handle_key for backward compat.
    pub fn handle_key(&mut self, code: KeyCode, ctrl: bool) -> bool {
        let modifiers = if ctrl {
            KeyModifiers::CONTROL
        } else {
            KeyModifiers::empty()
        };
        self.handle_key_event(KeyEvent::new(code, modifiers))
    }

    // ── Cursor movement ──────────────────────────────────────────

    fn move_left(&mut self) -> bool {
        if self.cursor_col > 0 {
            let line = &self.lines[self.cursor_row];
            self.cursor_col = Self::grapheme_pos_relative(line, self.cursor_col, -1);
            true
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
            true
        } else {
            false
        }
    }

    fn move_right(&mut self) -> bool {
        let line = &self.lines[self.cursor_row];
        if self.cursor_col < line.len() {
            self.cursor_col = Self::grapheme_pos_relative(line, self.cursor_col, 1);
            true
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
            true
        } else {
            false
        }
    }

    fn move_up(&mut self) -> bool {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            let max = self.lines[self.cursor_row].len();
            if self.cursor_col > max {
                self.cursor_col = max;
            }
            true
        } else {
            false
        }
    }

    fn move_down(&mut self) -> bool {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            let max = self.lines[self.cursor_row].len();
            if self.cursor_col > max {
                self.cursor_col = max;
            }
            true
        } else {
            false
        }
    }

    // ── Word movement ────────────────────────────────────────────

    fn word_char_class(c: char) -> u8 {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    }

    fn move_word_left(&mut self) {
        let line = &self.lines[self.cursor_row];
        self.cursor_col = Self::find_word_left(line, self.cursor_col);
    }

    fn move_word_right(&mut self) {
        let line = &self.lines[self.cursor_row];
        self.cursor_col = Self::find_word_right(line, self.cursor_col);
    }

    fn find_word_left(s: &str, from: usize) -> usize {
        if from == 0 {
            return 0;
        }
        let graphemes: Vec<(usize, &str)> = s.grapheme_indices(true).collect();
        let mut idx = graphemes.len();
        for (i, (pos, _)) in graphemes.iter().enumerate() {
            if *pos >= from {
                idx = i;
                break;
            }
        }
        let mut i = idx;
        while i > 0 {
            let c = graphemes[i - 1].1.chars().next().unwrap_or(' ');
            if !c.is_whitespace() {
                break;
            }
            i -= 1;
        }
        if i == 0 {
            return 0;
        }
        let target = Self::word_char_class(graphemes[i - 1].1.chars().next().unwrap_or(' '));
        while i > 0 {
            let (_pos, g) = graphemes[i - 1];
            let c = g.chars().next().unwrap_or(' ');
            if Self::word_char_class(c) != target {
                return graphemes[i].0;
            }
            i -= 1;
            if i == 0 {
                return 0;
            }
        }
        0
    }

    fn find_word_right(s: &str, from: usize) -> usize {
        if from >= s.len() {
            return s.len();
        }
        let graphemes: Vec<(usize, &str)> = s.grapheme_indices(true).collect();
        let mut idx = 0;
        for (i, (pos, _)) in graphemes.iter().enumerate() {
            if *pos >= from {
                idx = i;
                break;
            }
        }
        if idx >= graphemes.len() {
            return s.len();
        }
        let target = Self::word_char_class(graphemes[idx].1.chars().next().unwrap_or(' '));
        let mut i = idx;
        while i < graphemes.len() {
            let (_, g) = graphemes[i];
            let c = g.chars().next().unwrap_or(' ');
            if Self::word_char_class(c) != target {
                break;
            }
            i += 1;
        }
        while i < graphemes.len() {
            let (_, g) = graphemes[i];
            let c = g.chars().next().unwrap_or(' ');
            if !c.is_whitespace() {
                break;
            }
            i += 1;
        }
        if i < graphemes.len() {
            graphemes[i].0
        } else {
            s.len()
        }
    }

    // ── Kill / delete ────────────────────────────────────────────

    fn kill_to_end(&mut self) {
        let line = &self.lines[self.cursor_row];
        if self.cursor_col < line.len() {
            self.lines[self.cursor_row].truncate(self.cursor_col);
        } else if self.cursor_row + 1 < self.lines.len() {
            self.lines.remove(self.cursor_row + 1);
        }
    }

    fn kill_to_start(&mut self) {
        if self.cursor_col > 0 {
            self.lines[self.cursor_row].drain(..self.cursor_col);
            self.cursor_col = 0;
        } else if self.cursor_row > 0 {
            let rest = self.lines[self.cursor_row].clone();
            self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
            self.lines[self.cursor_row].push_str(&rest);
        }
    }

    fn kill_word_left(&mut self) {
        if self.cursor_col > 0 {
            let line = &self.lines[self.cursor_row];
            let start = Self::find_word_left(line, self.cursor_col);
            self.lines[self.cursor_row].drain(start..self.cursor_col);
            self.cursor_col = start;
        } else if self.cursor_row > 0 {
            let rest = self.lines[self.cursor_row].clone();
            self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
            self.lines[self.cursor_row].push_str(&rest);
        }
    }

    fn delete_word_left(&mut self) {
        self.kill_word_left();
    }

    fn delete_word_right(&mut self) {
        let line = &self.lines[self.cursor_row];
        if self.cursor_col < line.len() {
            let end = Self::find_word_end(line, self.cursor_col);
            self.lines[self.cursor_row].drain(self.cursor_col..end);
        } else if self.cursor_row + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next);
        }
    }

    fn find_word_end(s: &str, from: usize) -> usize {
        if from >= s.len() {
            return s.len();
        }
        let graphemes: Vec<(usize, &str)> = s.grapheme_indices(true).collect();
        let mut idx = 0;
        for (i, (pos, _)) in graphemes.iter().enumerate() {
            if *pos >= from {
                idx = i;
                break;
            }
        }
        if idx >= graphemes.len() {
            return s.len();
        }
        let target = Self::word_char_class(graphemes[idx].1.chars().next().unwrap_or(' '));
        let mut i = idx;
        while i < graphemes.len() {
            let (_, g) = graphemes[i];
            let c = g.chars().next().unwrap_or(' ');
            if Self::word_char_class(c) != target {
                break;
            }
            i += 1;
        }
        if i < graphemes.len() {
            graphemes[i].0
        } else {
            s.len()
        }
    }

    // ── Text insertion ───────────────────────────────────────────

    fn clamp_cursor(&mut self) {
        if self.cursor_row >= self.lines.len() {
            self.cursor_row = self.lines.len().saturating_sub(1);
        }
        let max = self.lines[self.cursor_row].len();
        if self.cursor_col > max {
            self.cursor_col = max;
        }
    }

    fn insert_char(&mut self, c: char) {
        self.clamp_cursor();
        if self.history_index.is_some() {
            self.history_index = None;
            self.history_draft = None;
        }
        let line = &mut self.lines[self.cursor_row];
        line.insert(self.cursor_col, c);
        self.cursor_col += c.len_utf8();
    }

    fn newline(&mut self) {
        self.clamp_cursor();
        if self.history_index.is_some() {
            self.history_index = None;
            self.history_draft = None;
        }
        let rest = self.lines[self.cursor_row][self.cursor_col..].to_string();
        self.lines[self.cursor_row].truncate(self.cursor_col);
        self.lines.insert(self.cursor_row + 1, rest);
        self.cursor_row += 1;
        self.cursor_col = 0;
    }

    // ── ANSI escape filtering ────────────────────────────────────

    /// Strip ANSI escape sequences (CSI, OSC, DCS, etc.) from text.
    /// These leak in when pasting terminal selection and corrupt
    /// display width calculations and rendering.
    fn strip_ansi(input: &str) -> String {
        let mut result = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                match chars.next() {
                    Some('[') => {
                        // CSI: skip until final byte (0x40-0x7E)
                        while let Some(&nc) = chars.peek() {
                            if ('@'..='~').contains(&nc) {
                                chars.next();
                                break;
                            }
                            chars.next();
                        }
                    }
                    Some(']') => {
                        // OSC: skip until BEL (0x07) or ST (ESC \)
                        while let Some(&nc) = chars.peek() {
                            if nc == '\x07' {
                                chars.next();
                                break;
                            }
                            if nc == '\x1b' {
                                chars.next();
                                if chars.peek() == Some(&'\\') {
                                    chars.next();
                                }
                                break;
                            }
                            chars.next();
                        }
                    }
                    Some(_) => {
                        // Other escape: skip the next char
                    }
                    None => break,
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    // ── Grapheme helpers ─────────────────────────────────────────

    fn grapheme_pos_relative(s: &str, byte_pos: usize, delta: isize) -> usize {
        let graphemes: Vec<(usize, &str)> = s.grapheme_indices(true).collect();
        let current_idx = graphemes
            .iter()
            .position(|(i, _)| *i >= byte_pos)
            .unwrap_or(graphemes.len());
        let new_idx = (current_idx as isize + delta).clamp(0, graphemes.len() as isize);
        if new_idx >= graphemes.len() as isize {
            s.len()
        } else {
            graphemes[new_idx as usize].0
        }
    }

    // ── Autocomplete ─────────────────────────────────────────────

    fn update_autocomplete_after_typing(&mut self) {
        let text = self.lines.join("\n");
        let trimmed = text.trim();
        if let Some(rest) = trimmed.strip_prefix('/') {
            if !rest.contains(' ') {
                self.update_slash_command_completions(rest);
            }
            return;
        }
        // @ file path
        let cursor_line = self.lines[self.cursor_row].clone();
        let before = &cursor_line[..self.cursor_col];
        if let Some(at) = before.rfind('@')
            && (at == 0 || before.as_bytes().get(at.wrapping_sub(1)) == Some(&b' '))
        {
            let prefix = before[at..].to_string();
            self.update_file_completions(&prefix[1..], true);
        }
    }

    fn try_autocomplete(&mut self) {
        let text = self.lines.join("\n");
        let trimmed = text.trim();
        if let Some(rest) = trimmed.strip_prefix('/') {
            self.update_slash_command_completions(rest);
            // Auto-accept single match (pi: explicitTab + single item)
            if self
                .autocomplete
                .as_ref()
                .is_some_and(|ac| ac.items.len() == 1)
            {
                self.accept_autocomplete_if_active();
            }
            return;
        }
        // File path completion
        let cursor_line = self.lines[self.cursor_row].clone();
        let before = &cursor_line[..self.cursor_col];
        let start = before.rfind(' ').map_or(0, |i| i + 1);
        let prefix = before[start..].to_string();
        let starts_with_at = prefix.starts_with('@');
        if !prefix.is_empty() || before.ends_with(' ') {
            let cleaned = prefix.trim_start_matches('@').to_string();
            self.update_file_completions(&cleaned, starts_with_at);
            // Auto-accept single file match too
            if self
                .autocomplete
                .as_ref()
                .is_some_and(|ac| ac.items.len() == 1)
            {
                self.accept_autocomplete_if_active();
            }
        }
    }

    /// Fuzzy match: all query characters must appear in order (case-insensitive).
    /// Matches pi's fuzzyFilter behavior for slash commands.
    fn fuzzy_match(query: &str, text: &str) -> bool {
        let query = query.to_lowercase();
        let text = text.to_lowercase();
        if query.is_empty() {
            return true;
        }
        let mut qi = query.chars().peekable();
        for c in text.chars() {
            if qi.peek() == Some(&c) {
                qi.next();
                if qi.peek().is_none() {
                    return true;
                }
            }
        }
        false
    }

    fn update_slash_command_completions(&mut self, prefix: &str) {
        let matching: Vec<AutocompleteItem> = self
            .slash_commands
            .iter()
            .filter(|c| Self::fuzzy_match(prefix, &c.name))
            .map(|c| AutocompleteItem {
                value: format!("/{} ", c.name),
                label: c.name.clone(),
                description: Some(c.description.clone()),
            })
            .collect();
        if matching.is_empty() {
            self.autocomplete = None;
        } else {
            self.autocomplete = Some(AutocompleteState {
                selected: 0,
                prefix: format!("/{}", prefix),
                items: matching,
            });
        }
    }

    fn update_file_completions(&mut self, prefix: &str, _is_at: bool) {
        let (search_prefix, search_dir) = Self::resolve_path_prefix(prefix, &self.cwd);
        let entries = Self::list_dir(&search_dir);
        if entries.is_empty() {
            self.autocomplete = None;
            return;
        }
        let lower = search_prefix.to_lowercase();
        let mut items: Vec<AutocompleteItem> = entries
            .iter()
            .filter(|e| e.name.to_lowercase().starts_with(&lower))
            .map(|e| AutocompleteItem {
                value: e.path.clone(),
                label: format!("{}{}", e.name, if e.is_dir { "/" } else { "" }),
                description: None,
            })
            .collect();
        items.sort_by(|a, b| {
            let a_dir = a.value.ends_with('/');
            let b_dir = b.value.ends_with('/');
            if a_dir && !b_dir {
                std::cmp::Ordering::Less
            } else if !a_dir && b_dir {
                std::cmp::Ordering::Greater
            } else {
                a.label.cmp(&b.label)
            }
        });
        if items.is_empty() {
            self.autocomplete = None;
        } else {
            self.autocomplete = Some(AutocompleteState {
                selected: 0,
                prefix: prefix.to_string(),
                items,
            });
        }
    }

    fn resolve_path_prefix(prefix: &str, cwd: &std::path::Path) -> (String, std::path::PathBuf) {
        let clean = prefix.trim_start_matches('@');
        if clean.is_empty() {
            return (String::new(), cwd.to_path_buf());
        }
        if clean.starts_with('/') {
            let path = std::path::Path::new(clean);
            let parent = path
                .parent()
                .map_or_else(|| std::path::PathBuf::from("/"), |p| p.to_path_buf());
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            return (name.to_string(), parent);
        }
        if clean.ends_with('/') {
            (String::new(), cwd.join(clean))
        } else {
            let path = std::path::Path::new(clean);
            let parent = path
                .parent()
                .map_or_else(|| cwd.to_path_buf(), |p| cwd.join(p));
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            (name.to_string(), parent)
        }
    }

    fn list_dir(dir: &std::path::Path) -> Vec<DirEntry> {
        let mut entries = Vec::new();
        let read_dir = match std::fs::read_dir(dir) {
            Ok(d) => d,
            Err(_) => return entries,
        };
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push(DirEntry {
                name: name.clone(),
                path: name,
                is_dir,
            });
        }
        entries
    }

    fn update_ac_selection(&mut self, delta: isize) {
        if let Some(ref mut ac) = self.autocomplete {
            let len = ac.items.len();
            if len == 0 {
                return;
            }
            ac.selected = if delta > 0 {
                (ac.selected + 1).min(len - 1)
            } else if ac.selected == 0 {
                len - 1
            } else {
                ac.selected - 1
            };
        }
    }

    fn apply_autocomplete_item(&mut self, item: &AutocompleteItem, prefix: &str) {
        if prefix.starts_with('/') {
            self.lines = vec![item.value.clone()];
            self.cursor_row = 0;
            self.cursor_col = item.value.len();
        } else {
            let cursor_line = &mut self.lines[self.cursor_row];
            let before = &cursor_line[..self.cursor_col];
            if let Some(pos) = before.rfind(prefix) {
                cursor_line.replace_range(pos..self.cursor_col, &item.value);
                self.cursor_col = pos + item.value.len();
            }
        }
    }

    // ── Render ───────────────────────────────────────────────────

    pub fn render_with_max(&self, area_width: u16, max_text_lines: usize) -> EditorRender {
        let layout_width = area_width.saturating_sub(0) as usize;
        let layout_width = layout_width.max(1);

        let mut layout_lines: Vec<LayoutLine> = Vec::new();
        if self.lines.is_empty() || (self.lines.len() == 1 && self.lines[0].is_empty()) {
            layout_lines.push(LayoutLine {
                text: String::new(),
                start_col: 0,
                is_cursor: true,
            });
        } else {
            for (row, line) in self.lines.iter().enumerate() {
                let wrapped = Self::word_wrap_line(line, layout_width);
                let mut col_offset = 0;
                for (wi, chunk) in wrapped.iter().enumerate() {
                    let is_last = wi == wrapped.len() - 1;
                    let cursor_in_chunk = if row == self.cursor_row {
                        if is_last {
                            self.cursor_col >= col_offset
                        } else {
                            self.cursor_col >= col_offset
                                && self.cursor_col < col_offset + chunk.len()
                        }
                    } else {
                        false
                    };
                    layout_lines.push(LayoutLine {
                        text: chunk.clone(),
                        start_col: col_offset,
                        is_cursor: cursor_in_chunk,
                    });
                    col_offset += chunk.len();
                }
            }
        }

        let mut cursor_visual_row = 0u16;
        let mut cursor_visual_col = 0u16;
        for (vi, ll) in layout_lines.iter().enumerate() {
            if ll.is_cursor {
                cursor_visual_row = vi as u16;
                cursor_visual_col = (self.cursor_col.saturating_sub(ll.start_col)) as u16;
                break;
            }
        }

        let mut text_lines: Vec<String> = layout_lines.iter().map(|l| l.text.clone()).collect();

        // Limit visible lines
        if text_lines.len() > max_text_lines {
            let cursor_line = cursor_visual_row as usize;
            let visible_start = if cursor_line >= max_text_lines {
                cursor_line - max_text_lines + 1
            } else {
                0
            };
            let visible_end = (visible_start + max_text_lines).min(text_lines.len());
            text_lines = text_lines[visible_start..visible_end].to_vec();
            cursor_visual_row = cursor_visual_row.saturating_sub(visible_start as u16);
        }

        let (ac_lines, ac_sel, ac_active) = self.render_autocomplete(layout_width);

        EditorRender {
            text_lines,
            cursor_col: cursor_visual_col,
            cursor_row: cursor_visual_row,
            autocomplete_lines: ac_lines,
            autocomplete_selection: ac_sel,
            autocomplete_active: ac_active,
        }
    }

    fn render_autocomplete(&self, width: usize) -> (Vec<String>, usize, bool) {
        if let Some(ref ac) = self.autocomplete {
            if ac.items.is_empty() {
                return (Vec::new(), 0, false);
            }
            let max_visible = 5.min(ac.items.len());
            let total = ac.items.len();
            let has_scroll = total > max_visible;

            // Compute primary column width (clamped 12–32, like pi)
            let widest_label = ac
                .items
                .iter()
                .map(|item| item.label.len())
                .max()
                .unwrap_or(0)
                .clamp(12, 32);
            let primary_width = widest_label + 2; // gap after label

            let mut lines = Vec::with_capacity(1 + max_visible + if has_scroll { 1 } else { 0 });

            // Separator line
            lines.push("─".repeat(width.min(60)));

            // SelectList-style scrolling: center selection in visible window
            let start = if total <= max_visible {
                0
            } else {
                (ac.selected.saturating_sub(max_visible / 2)).min(total - max_visible)
            };
            let end = (start + max_visible).min(total);

            for i in start..end {
                let item = &ac.items[i];
                let selected = i == ac.selected;
                let marker = if selected { "→ " } else { "  " };
                let full = format!("{}{}", marker, item.label);

                let line = if let Some(ref desc) = item.description {
                    let label_w = full.len();
                    // Pad label to primary column width, then append description
                    let padding = primary_width.saturating_sub(label_w);
                    let padded = format!("{}{}", full, " ".repeat(padding));
                    let avail = width.saturating_sub(primary_width + 2);
                    if avail > 10 {
                        let desc_trunc = if desc.len() > avail {
                            let mut d = desc[..avail.saturating_sub(1)].to_string();
                            d.push('…');
                            d
                        } else {
                            desc.clone()
                        };
                        format!("{}— {}", padded, desc_trunc)
                    } else {
                        padded
                    }
                } else {
                    full
                };

                lines.push(if line.len() > width {
                    let mut l = line[..width.saturating_sub(1)].to_string();
                    l.push('…');
                    l
                } else {
                    line
                });
            }

            // Scroll indicator (pi: shows when start > 0 or end < total)
            if has_scroll {
                lines.push(format!("  ({}/{})", ac.selected + 1, total));
            }
            (lines, ac.selected, true)
        } else {
            (Vec::new(), 0, false)
        }
    }

    // ── Word wrapping ────────────────────────────────────────────

    pub fn word_wrap(text: &str, max_width: usize) -> Vec<String> {
        let mut result = Vec::new();
        for line in text.split('\n') {
            result.extend(Self::word_wrap_line(line, max_width));
        }
        result
    }

    fn word_wrap_line(line: &str, max_width: usize) -> Vec<String> {
        if max_width == 0 {
            return vec![String::new()];
        }
        let line_width = Self::display_width(line);
        if line_width <= max_width {
            return vec![line.to_string()];
        }
        let graphemes: Vec<&str> = line.graphemes(true).collect();
        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut current_width = 0;
        let mut wrap_opp: Option<usize> = None;
        let mut wrap_opp_width = 0;

        for (i, g) in graphemes.iter().enumerate() {
            let gw = Self::display_width(g);
            let is_ws = g.chars().all(|c| c.is_whitespace());

            if current_width + gw > max_width {
                if let Some(wo) = wrap_opp
                    && current_width - wrap_opp_width + gw <= max_width
                {
                    let remainder = current[wo..].to_string();
                    current.truncate(wo);
                    if !current.is_empty() {
                        chunks.push(current);
                    }
                    current = remainder;
                    current_width -= wrap_opp_width;
                    wrap_opp = None;
                }
                if current_width + gw > max_width && !current.is_empty() {
                    chunks.push(current.clone());
                    current.clear();
                    current_width = 0;
                    wrap_opp = None;
                }
            }
            if gw > max_width {
                if !current.is_empty() {
                    chunks.push(current.clone());
                    current.clear();
                    current_width = 0;
                }
                chunks.push(g.to_string());
                wrap_opp = None;
                continue;
            }
            current.push_str(g);
            current_width += gw;

            let next = graphemes.get(i + 1);
            let next_is_ws = next.is_none_or(|n| n.chars().all(|c| c.is_whitespace()));
            if is_ws && !next_is_ws {
                wrap_opp = Some(current.len());
                wrap_opp_width = current_width;
            }
        }
        if !current.is_empty() {
            chunks.push(current);
        }
        if chunks.is_empty() {
            chunks.push(String::new());
        }
        chunks
    }

    fn display_width(s: &str) -> usize {
        s.graphemes(true)
            .map(|g| {
                let first = g.chars().next().unwrap_or(' ');
                if Self::is_cjk(first) {
                    2
                } else if first.is_ascii() {
                    1
                } else if first as u32 > 0x2000 {
                    2
                } else {
                    1
                }
            })
            .sum()
    }

    fn is_cjk(c: char) -> bool {
        matches!(
            c,
            '\u{1100}'..='\u{115F}'
                | '\u{2E80}'..='\u{303E}'
                | '\u{3040}'..='\u{33BF}'
                | '\u{3400}'..='\u{4DBF}'
                | '\u{4E00}'..='\u{9FFF}'
                | '\u{F900}'..='\u{FAFF}'
                | '\u{FE30}'..='\u{FE4F}'
                | '\u{FF01}'..='\u{FF60}'
                | '\u{FFE0}'..='\u{FFE6}'
                | '\u{20000}'..='\u{2FA1F}'
        )
    }
}

struct LayoutLine {
    text: String,
    start_col: usize,
    is_cursor: bool,
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn kc(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty())
    }

    fn ctrl_kc(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn alt_kc(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }

    #[test]
    fn test_new_empty() {
        let ed = Editor::new();
        assert!(ed.is_empty());
        assert_eq!(ed.text(), "");
    }

    #[test]
    fn test_insert_and_backspace() {
        let mut ed = Editor::new();
        ed.handle_key_event(kc('a'));
        ed.handle_key_event(kc('b'));
        assert_eq!(ed.text(), "ab");
        ed.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()));
        assert_eq!(ed.text(), "a");
    }

    #[test]
    fn test_set_text() {
        let mut ed = Editor::new();
        ed.set_text("hello\nworld");
        assert_eq!(ed.text(), "hello\nworld");
    }

    #[test]
    fn test_emoji_cursor() {
        let mut ed = Editor::new();
        ed.set_text("a😀b");
        // cursor at end; move to start
        ed.handle_key_event(ctrl_kc('a')); // Ctrl+A = line start
        assert_eq!(ed.cursor_col, 0);
        ed.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::empty()));
        assert_eq!(ed.cursor_col, 1); // after 'a'
        ed.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::empty()));
        assert_eq!(ed.cursor_col, 5); // after emoji (1+4 bytes)
    }

    #[test]
    fn test_emoji_backspace() {
        let mut ed = Editor::new();
        ed.set_text("a😀b");
        ed.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()));
        assert_eq!(ed.text(), "a😀");
        ed.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()));
        assert_eq!(ed.text(), "a");
    }

    #[test]
    fn test_word_move_right() {
        let mut ed = Editor::new();
        ed.set_text("hello world foo");
        ed.handle_key_event(ctrl_kc('a'));
        ed.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(ed.cursor_col, 6); // after "hello "
    }

    #[test]
    fn test_word_move_left() {
        let mut ed = Editor::new();
        ed.set_text("hello world");
        ed.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(ed.cursor_col, 6); // start of "world"
    }

    #[test]
    fn test_kill_line() {
        let mut ed = Editor::new();
        ed.set_text("hello world");
        ed.handle_key_event(ctrl_kc('a'));
        ed.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        ed.handle_key_event(ctrl_kc('k'));
        assert_eq!(ed.text(), "hello ");
    }

    #[test]
    fn test_paste() {
        let mut ed = Editor::new();
        ed.handle_paste("pasted text");
        assert_eq!(ed.text(), "pasted text");
    }

    #[test]
    fn test_paste_multiline() {
        let mut ed = Editor::new();
        ed.handle_paste("line1\nline2\nline3");
        assert_eq!(ed.text(), "line1\nline2\nline3");
    }

    #[test]
    fn test_paste_strips_trailing_newline() {
        let mut ed = Editor::new();
        ed.handle_paste("hello\n");
        assert_eq!(ed.text(), "hello");
    }

    #[test]
    fn test_paste_empty() {
        let mut ed = Editor::new();
        ed.set_text("keep");
        ed.handle_paste("\n");
        assert_eq!(ed.text(), "keep");
    }

    #[test]
    fn test_history() {
        let mut ed = Editor::new();
        ed.add_to_history("first");
        ed.add_to_history("second");
        ed.recall_history(-1);
        assert_eq!(ed.text(), "second");
        ed.recall_history(-1);
        assert_eq!(ed.text(), "first");
        ed.recall_history(1);
        assert_eq!(ed.text(), "second");
        ed.recall_history(1);
        assert!(ed.is_empty());
    }

    #[test]
    fn test_word_wrap_basic() {
        let result = Editor::word_wrap("hello world foo", 12);
        assert!(result.len() >= 2);
    }

    #[test]
    fn test_word_wrap_no_wrap() {
        assert_eq!(Editor::word_wrap("short", 40), vec!["short"]);
    }

    #[test]
    fn test_autocomplete_slash() {
        let mut ed = Editor::new();
        ed.set_slash_commands(vec![SlashCommandInfo {
            name: "model".into(),
            description: "Switch model".into(),
        }]);
        ed.handle_key_event(kc('/'));
        ed.handle_key_event(kc('m'));
        assert!(ed.autocomplete_active());
        ed.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(ed.text(), "/model ");
        assert!(!ed.autocomplete_active());
    }

    #[test]
    fn test_paste_strips_ansi_escapes() {
        let mut ed = Editor::new();
        ed.handle_paste("\x1b[38;5;245mrab\x1b[0m · model");
        assert_eq!(ed.text(), "rab · model");
    }

    #[test]
    fn test_paste_strips_ansi_with_newlines() {
        let mut ed = Editor::new();
        ed.handle_paste("line1\x1b[0m\n\x1b[1mline2\x1b[0m");
        assert_eq!(ed.text(), "line1\nline2");
    }

    #[test]
    fn test_paste_preserves_emoji() {
        let mut ed = Editor::new();
        ed.handle_paste("hello 🚀 world");
        assert_eq!(ed.text(), "hello 🚀 world");
    }
}
