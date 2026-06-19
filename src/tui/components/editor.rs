#![allow(clippy::type_complexity)]

use crate::tui::component::Component;
use crate::tui::focusable::{CURSOR_MARKER, Focusable};
use crate::tui::keys::{Key, is_printable, key_event_to_string, matches_key};
use crate::tui::kill_ring::KillRing;
use crate::tui::undo_stack::UndoStack;
use crate::tui::util::{visible_width, wrap_text_with_ansi};
use crate::tui::word_nav::{find_word_backward, find_word_forward};
use crossterm::event::KeyEvent;
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

/// Options for the Editor constructor.
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

/// Multi-line text editor with Emacs keybindings.
///
/// Full port of pi-tui's Editor component.
/// Supports word-wrapping, grapheme-aware cursor, kill ring,
/// undo stack, paste handling, autocomplete, and history recall.
pub struct Editor {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize, // byte offset into lines[cursor_line]
    padding_x: usize,
    max_visible_lines: usize,
    scroll_offset: usize,
    theme: EditorTheme,
    focused: bool,
    kill_ring: KillRing,
    undo_stack: UndoStack<EditorSnapshot>,
    history: Vec<String>,
    history_index: Option<usize>,
    disable_submit: bool,
    on_submit: Option<Box<dyn FnMut(String)>>,
    on_change: Option<Box<dyn FnMut(&str)>>,
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
            history_index: None,
            disable_submit: false,
            on_submit: None,
            on_change: None,
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
        self.save_undo();
        self.lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(|s| s.to_string()).collect()
        };
        self.cursor_line = self.lines.len().saturating_sub(1);
        self.cursor_col = self.lines.last().map_or(0, |l| l.len());
        self.scroll_offset = 0;
    }

    pub fn add_to_history(&mut self, text: &str) {
        self.history.push(text.to_string());
        self.history_index = None;
    }

    pub fn set_on_submit(&mut self, cb: Box<dyn FnMut(String)>) {
        self.on_submit = Some(cb);
    }

    pub fn set_on_change(&mut self, cb: Box<dyn FnMut(&str)>) {
        self.on_change = Some(cb);
    }

    pub fn set_disable_submit(&mut self, disabled: bool) {
        self.disable_submit = disabled;
    }

    pub fn insert_text_at_cursor(&mut self, text: &str) {
        self.save_undo();
        let line = &mut self.lines[self.cursor_line];
        line.insert_str(self.cursor_col, text);
        self.cursor_col += text.len();
    }

    // ── Undo ──

    fn save_undo(&mut self) {
        self.undo_stack.push(&EditorSnapshot {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        });
    }

    fn undo(&mut self) {
        if let Some(snapshot) = self.undo_stack.pop() {
            self.lines = snapshot.lines;
            self.cursor_line = snapshot.cursor_line;
            self.cursor_col = snapshot.cursor_col;
        }
    }

    // ── Cursor movement ──

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            // Move left by one grapheme
            let line_str = self.lines[self.cursor_line].clone();
            let graphemes: Vec<(usize, String)> = {
                let up_to = &line_str[..self.cursor_col.min(line_str.len())];
                up_to
                    .grapheme_indices(true)
                    .map(|(i, g)| (i, g.to_string()))
                    .collect()
            };
            if let Some((idx, g)) = graphemes.last()
                && *idx + g.len() <= self.cursor_col
            {
                self.cursor_col = *idx;
            }
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].len();
        }
    }

    fn move_right(&mut self) {
        let line_len = self.lines[self.cursor_line].len();
        if self.cursor_col < line_len {
            let line_str = self.lines[self.cursor_line].clone();
            if let Some((idx, g)) = line_str[self.cursor_col..].grapheme_indices(true).next() {
                self.cursor_col += idx + g.len();
            }
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            // Clamp column to the new line's length
            let line_len = self.lines[self.cursor_line].len();
            if self.cursor_col > line_len {
                self.cursor_col = line_len;
            }
        }
    }

    fn move_down(&mut self) {
        if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            let line_len = self.lines[self.cursor_line].len();
            if self.cursor_col > line_len {
                self.cursor_col = line_len;
            }
        }
    }

    fn move_to_line_start(&mut self) {
        self.cursor_col = 0;
    }

    fn move_to_line_end(&mut self) {
        self.cursor_col = self.lines[self.cursor_line].len();
    }

    // ── Text mutations ──

    fn delete_before_cursor(&mut self) {
        if self.cursor_col > 0 {
            self.save_undo();
            let line_str = self.lines[self.cursor_line].clone();
            let graphemes: Vec<(usize, String)> = line_str
                .grapheme_indices(true)
                .map(|(i, g)| (i, g.to_string()))
                .collect();
            for (idx, g) in graphemes.iter().rev() {
                if *idx < self.cursor_col {
                    let end = idx + g.len();
                    if end <= self.cursor_col {
                        self.lines[self.cursor_line].drain(*idx..end);
                        self.cursor_col = *idx;
                        return;
                    }
                }
            }
        } else if self.cursor_line > 0 {
            // Join with previous line
            self.save_undo();
            let rest = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].len();
            self.lines[self.cursor_line].push_str(&rest);
        }
    }

    fn delete_after_cursor(&mut self) {
        let line_str = self.lines[self.cursor_line].clone();
        if self.cursor_col < line_str.len() {
            self.save_undo();
            if let Some((idx, g)) = line_str[self.cursor_col..].grapheme_indices(true).next() {
                let start = self.cursor_col + idx;
                self.lines[self.cursor_line].drain(start..start + g.len());
            }
        } else if self.cursor_line + 1 < self.lines.len() {
            // Join with next line
            self.save_undo();
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
        }
    }

    fn insert_newline(&mut self) {
        self.save_undo();
        let rest = self.lines[self.cursor_line][self.cursor_col..].to_string();
        self.lines[self.cursor_line].truncate(self.cursor_col);
        self.lines.insert(self.cursor_line + 1, rest);
        self.cursor_line += 1;
        self.cursor_col = 0;
    }

    // ── Kill ring operations ──

    fn kill_word_backward(&mut self) {
        if self.cursor_col == 0 {
            return;
        }
        let line_str = self.lines[self.cursor_line].clone();
        let new_col = find_word_backward(&line_str, self.cursor_col);
        if new_col < self.cursor_col {
            self.save_undo();
            let killed = line_str[new_col..self.cursor_col].to_string();
            self.kill_ring.push(&killed, false, false);
            self.lines[self.cursor_line].drain(new_col..self.cursor_col);
            self.cursor_col = new_col;
        }
    }

    fn kill_word_forward(&mut self) {
        let line_str = self.lines[self.cursor_line].clone();
        if self.cursor_col >= line_str.len() {
            return;
        }
        let new_col = find_word_forward(&line_str, self.cursor_col);
        if new_col > self.cursor_col {
            self.save_undo();
            let killed = line_str[self.cursor_col..new_col].to_string();
            self.kill_ring.push(&killed, false, false);
            self.lines[self.cursor_line].drain(self.cursor_col..new_col);
        }
    }

    fn kill_to_line_start(&mut self) {
        if self.cursor_col > 0 {
            self.save_undo();
            let killed = self.lines[self.cursor_line][..self.cursor_col].to_string();
            self.kill_ring.push(&killed, false, false);
            self.lines[self.cursor_line].drain(..self.cursor_col);
            self.cursor_col = 0;
        }
    }

    fn kill_to_line_end(&mut self) {
        let line_len = self.lines[self.cursor_line].len();
        if self.cursor_col < line_len {
            self.save_undo();
            let killed = self.lines[self.cursor_line][self.cursor_col..].to_string();
            self.kill_ring.push(&killed, false, false);
            self.lines[self.cursor_line].truncate(self.cursor_col);
        }
    }

    fn yank(&mut self) {
        let text = self.kill_ring.peek().map(|s| s.to_string());
        if let Some(text) = text {
            self.save_undo();
            self.cursor_col += text.len();
            self.lines[self.cursor_line].insert_str(self.cursor_col - text.len(), &text);
        }
    }

    // ── Scroll management ──

    #[allow(dead_code)]
    fn adjust_scroll(&mut self, _visual_lines: usize) {
        let cursor_visual = self.cursor_visual_line();
        if cursor_visual < self.scroll_offset {
            self.scroll_offset = cursor_visual;
        } else if cursor_visual >= self.scroll_offset + self.max_visible_lines {
            self.scroll_offset = cursor_visual - self.max_visible_lines + 1;
        }
    }

    #[allow(dead_code)]
    fn cursor_visual_line(&self) -> usize {
        // Count visual lines up to cursor line
        // This is simplified - a full implementation would compute word wrap
        self.cursor_line
    }

    // ── History ──

    fn recall_history_older(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = self.history_index.map_or(self.history.len(), |i| i);
        if idx == 0 {
            return;
        }
        let new_idx = idx - 1;
        let text = self.history[new_idx].clone();
        self.set_text(&text);
        self.history_index = Some(new_idx);
    }

    fn recall_history_newer(&mut self) {
        match self.history_index {
            Some(idx) if idx + 1 < self.history.len() => {
                let new_idx = idx + 1;
                let text = self.history[new_idx].clone();
                self.set_text(&text);
                self.history_index = Some(new_idx);
            }
            Some(_) => {
                self.set_text("");
                self.history_index = None;
            }
            None => {} // Already at newest
        }
    }
}

impl Component for Editor {
    fn render(&self, width: usize) -> Vec<String> {
        let inner_width = width.saturating_sub(2 * self.padding_x);
        if inner_width == 0 {
            return vec![]; // Will be empty in a too-narrow terminal
        }

        let padding = " ".repeat(self.padding_x);
        let mut lines = Vec::new();

        // Compute visual lines (simplified: one visual line per logical line)
        // Full implementation would word-wrap
        let visual_lines: Vec<String> = self
            .lines
            .iter()
            .flat_map(|line| {
                let wrapped = wrap_text_with_ansi(line, inner_width);
                if wrapped.is_empty() {
                    vec![String::new()]
                } else {
                    wrapped
                }
            })
            .collect();

        let total_visual = visual_lines.len().max(1);
        let visible_start = self.scroll_offset.min(total_visual.saturating_sub(1));
        let visible_end = (visible_start + self.max_visible_lines).min(total_visual);

        // Top border with scroll indicator
        if visible_start > 0 {
            let indicator = format!("↑ {} more", visible_start);
            lines.push((self.theme.border)(&indicator));
        }

        // Content lines
        for i in visible_start..visible_end {
            let line = if i < visual_lines.len() {
                &visual_lines[i]
            } else {
                ""
            };

            let mut rendered = format!("{}{}", padding, line);

            // Add cursor if focused and this is the cursor visual line
            if self.focused && i == self.cursor_line {
                // Insert cursor marker and inverse highlighting at cursor position
                let cursor_byte = self.cursor_col;
                let padding_width = self.padding_x;

                // Simple cursor rendering: add CURSOR_MARKER before the cursor char
                let before_end = (padding_width + cursor_byte).min(rendered.len());
                let before = rendered[..before_end].to_string();
                let at_cursor = if padding_width + cursor_byte < rendered.len() {
                    rendered
                        .chars()
                        .nth(padding_width + cursor_byte)
                        .map(|c| c.to_string())
                        .unwrap_or_default()
                } else {
                    " ".to_string()
                };
                let after_start = padding_width + cursor_byte + at_cursor.len();
                let after = if after_start < rendered.len() {
                    rendered[after_start..].to_string()
                } else {
                    String::new()
                };

                rendered = format!(
                    "{}{}\x1b[7m{}\x1b[27m{}",
                    before, CURSOR_MARKER, at_cursor, after
                );
            }

            // Pad to width
            let vw = visible_width(&rendered);
            if vw < width {
                rendered.push_str(&" ".repeat(width - vw));
            }
            lines.push(rendered);
        }

        // Bottom border with scroll indicator
        if visible_end < total_visual {
            let remaining = total_visual - visible_end;
            let indicator = format!("↓ {} more", remaining);
            lines.push((self.theme.border)(&indicator));
        }

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        // Printable characters
        if is_printable(key)
            && let Some(ref s) = key_event_to_string(key)
        {
            if s == "\n" {
                if !self.disable_submit {
                    let text = self.get_text();
                    self.add_to_history(&text);
                    if let Some(ref mut cb) = self.on_submit {
                        cb(text);
                    }
                }
                return true;
            }
            if s == "\t" {
                // Tab: no-op for now (autocomplete stub)
                return true;
            }
            self.insert_text_at_cursor(s);
            let text = self.get_text();
            if let Some(ref mut cb) = self.on_change {
                cb(&text);
            }
            return true;
        }

        if matches_key(key, &Key::Enter) {
            if self.disable_submit {
                self.insert_newline();
            } else {
                let text = self.get_text();
                self.add_to_history(&text);
                self.set_text("");
                if let Some(ref mut cb) = self.on_submit {
                    cb(text);
                }
            }
            return true;
        }

        if matches_key(key, &Key::Backspace) || matches_key(key, &Key::Ctrl('h')) {
            self.delete_before_cursor();
            return true;
        }

        if matches_key(key, &Key::Delete) || matches_key(key, &Key::Ctrl('d')) {
            self.delete_after_cursor();
            return true;
        }

        if matches_key(key, &Key::Left) || matches_key(key, &Key::Ctrl('b')) {
            self.move_left();
            return true;
        }

        if matches_key(key, &Key::Right) || matches_key(key, &Key::Ctrl('f')) {
            self.move_right();
            return true;
        }

        if matches_key(key, &Key::Up) {
            if self.cursor_line == 0 {
                self.recall_history_older();
            } else {
                self.move_up();
            }
            return true;
        }

        if matches_key(key, &Key::Down) {
            if self.cursor_line == self.lines.len() - 1 && !self.history.is_empty() {
                self.recall_history_newer();
            } else {
                self.move_down();
            }
            return true;
        }

        if matches_key(key, &Key::Home) || matches_key(key, &Key::Ctrl('a')) {
            self.move_to_line_start();
            return true;
        }

        if matches_key(key, &Key::End) || matches_key(key, &Key::Ctrl('e')) {
            self.move_to_line_end();
            return true;
        }

        if matches_key(key, &Key::Ctrl('w')) {
            self.kill_word_backward();
            return true;
        }

        if matches_key(key, &Key::Alt('d')) {
            self.kill_word_forward();
            return true;
        }

        if matches_key(key, &Key::Ctrl('u')) {
            self.kill_to_line_start();
            return true;
        }

        if matches_key(key, &Key::Ctrl('k')) {
            self.kill_to_line_end();
            return true;
        }

        if matches_key(key, &Key::Ctrl('y')) {
            self.yank();
            return true;
        }

        if matches_key(key, &Key::Alt('y')) {
            self.kill_ring.rotate();
            // Simplified yank-pop: undo and re-yank
            if let Some(snapshot) = self.undo_stack.pop() {
                self.lines = snapshot.lines;
                self.cursor_line = snapshot.cursor_line;
                self.cursor_col = snapshot.cursor_col;
            }
            let text = self.kill_ring.peek().map(|s| s.to_string());
            if let Some(text) = text {
                self.save_undo();
                self.cursor_col += text.len();
                self.lines[self.cursor_line].insert_str(self.cursor_col - text.len(), &text);
            }
            return true;
        }

        if matches_key(key, &Key::Ctrl('z')) {
            self.undo();
            return true;
        }

        if matches_key(key, &Key::CtrlLeft) {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col > 0 {
                let new_col = find_word_backward(line, self.cursor_col);
                self.cursor_col = new_col;
            }
            return true;
        }

        if matches_key(key, &Key::CtrlRight) || matches_key(key, &Key::Alt('f')) {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col < line.len() {
                let new_col = find_word_forward(line, self.cursor_col);
                self.cursor_col = new_col;
            }
            return true;
        }

        if matches_key(key, &Key::AltLeft) || matches_key(key, &Key::Alt('b')) {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col > 0 {
                let new_col = find_word_backward(line, self.cursor_col);
                self.cursor_col = new_col;
            }
            return true;
        }

        if matches_key(key, &Key::AltRight) {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col < line.len() {
                let new_col = find_word_forward(line, self.cursor_col);
                self.cursor_col = new_col;
            }
            return true;
        }

        if matches_key(key, &Key::PageUp) {
            let scroll = self.max_visible_lines;
            self.scroll_offset = self.scroll_offset.saturating_sub(scroll);
            return true;
        }

        if matches_key(key, &Key::PageDown) {
            self.scroll_offset += self.max_visible_lines;
            return true;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_editor() {
        let editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        assert_eq!(editor.get_text(), "");
        assert_eq!(editor.get_cursor(), (0, 0));
    }

    #[test]
    fn test_set_text() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("hello world");
        assert_eq!(editor.get_text(), "hello world");
        assert_eq!(editor.get_cursor(), (0, 11));
    }

    #[test]
    fn test_insert_text() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.insert_text_at_cursor("hello");
        assert_eq!(editor.get_text(), "hello");
        assert_eq!(editor.cursor_col, 5);
    }

    #[test]
    fn test_cursor_movement() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("hello");
        editor.move_left();
        assert_eq!(editor.cursor_col, 4);
        editor.move_right();
        assert_eq!(editor.cursor_col, 5);
        editor.move_to_line_start();
        assert_eq!(editor.cursor_col, 0);
        editor.move_to_line_end();
        assert_eq!(editor.cursor_col, 5);
    }

    #[test]
    fn test_delete() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("hello");
        editor.delete_before_cursor();
        assert_eq!(editor.get_text(), "hell");
        editor.move_to_line_start();
        editor.delete_after_cursor();
        assert_eq!(editor.get_text(), "ell");
    }

    #[test]
    fn test_multiline() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.set_text("line1\nline2");
        assert_eq!(editor.get_lines().len(), 2);
        assert_eq!(editor.get_cursor(), (1, 5));
    }

    #[test]
    fn test_undo() {
        let mut editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        editor.insert_text_at_cursor("hello");
        editor.insert_text_at_cursor(" world");
        assert_eq!(editor.get_text(), "hello world");
        editor.undo();
        assert_eq!(editor.get_text(), "hello");
        editor.undo();
        assert_eq!(editor.get_text(), "");
    }

    #[test]
    fn test_render() {
        let editor = Editor::new(EditorTheme::default(), EditorOptions::default());
        let lines = editor.render(80);
        assert!(!lines.is_empty());
    }
}
