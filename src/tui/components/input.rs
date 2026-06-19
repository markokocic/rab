#![allow(clippy::type_complexity)]

use crate::tui::component::Component;
use crate::tui::focusable::{CURSOR_MARKER, Focusable};
use crate::tui::keys::{Key, is_printable, key_event_to_string, matches_key};
use crate::tui::kill_ring::KillRing;
use crate::tui::undo_stack::UndoStack;
use crate::tui::util::{slice_by_column, visible_width};
use crate::tui::word_nav::{find_word_backward, find_word_forward};
use crossterm::event::KeyEvent;
use unicode_segmentation::UnicodeSegmentation;

/// Single-line text input component.
///
/// Supports Emacs-style cursor movement and kill ring operations.
/// Renders with `> prompt text█padding...` layout.
pub struct Input {
    value: String,
    cursor: usize, // byte offset into value
    prompt: String,
    kill_ring: KillRing,
    undo_stack: UndoStack<String>,
    focused: bool,
    scroll_offset: usize,
    on_submit: Option<Box<dyn FnMut(String)>>,
    on_escape: Option<Box<dyn FnMut()>>,
    on_change: Option<Box<dyn FnMut(&str)>>,
}

impl Input {
    pub fn new() -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            prompt: "> ".to_string(),
            kill_ring: KillRing::new(),
            undo_stack: UndoStack::new(),
            focused: false,
            scroll_offset: 0,
            on_submit: None,
            on_escape: None,
            on_change: None,
        }
    }

    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }

    pub fn get_value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: &str) {
        self.save_undo();
        self.value = value.to_string();
        self.cursor = self.value.len();
        self.scroll_offset = 0;
        if let Some(ref mut cb) = self.on_change {
            cb(&self.value);
        }
    }

    pub fn set_on_submit(&mut self, cb: Box<dyn FnMut(String)>) {
        self.on_submit = Some(cb);
    }

    pub fn set_on_escape(&mut self, cb: Box<dyn FnMut()>) {
        self.on_escape = Some(cb);
    }

    pub fn set_on_change(&mut self, cb: Box<dyn FnMut(&str)>) {
        self.on_change = Some(cb);
    }

    fn save_undo(&mut self) {
        self.undo_stack.push(&self.value);
    }

    fn insert_text(&mut self, text: &str) {
        self.save_undo();
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.scroll_offset = 0;
    }

    fn delete_before_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.save_undo();
        // Delete one grapheme before cursor
        let graphemes: Vec<(usize, &str)> = self.value.grapheme_indices(true).collect();
        for &(idx, g) in graphemes.iter().rev() {
            if idx < self.cursor {
                let end = idx + g.len();
                if end <= self.cursor {
                    self.value.drain(idx..end);
                    self.cursor = idx;
                    break;
                }
            }
        }
        self.scroll_offset = 0;
    }

    fn delete_after_cursor(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.save_undo();
        let graphemes: Vec<(usize, &str)> = self.value.grapheme_indices(true).collect();
        for &(idx, g) in &graphemes {
            if idx >= self.cursor {
                self.value.drain(idx..idx + g.len());
                break;
            }
        }
        self.scroll_offset = 0;
    }

    fn move_cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let graphemes: Vec<(usize, &str)> = self.value.grapheme_indices(true).collect();
        for &(idx, g) in graphemes.iter().rev() {
            if idx < self.cursor {
                let end = idx + g.len();
                if end <= self.cursor {
                    self.cursor = idx;
                    break;
                }
            }
        }
    }

    fn move_cursor_right(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        if let Some((idx, g)) = self.value[self.cursor..].grapheme_indices(true).next() {
            self.cursor += idx + g.len();
        }
    }

    fn move_to_start(&mut self) {
        self.cursor = 0;
    }

    fn move_to_end(&mut self) {
        self.cursor = self.value.len();
    }

    fn kill_word_backward(&mut self) {
        let new_cursor = find_word_backward(&self.value, self.cursor);
        if new_cursor < self.cursor {
            self.save_undo();
            let killed = self.value[new_cursor..self.cursor].to_string();
            self.kill_ring.push(&killed, false, false);
            self.value.drain(new_cursor..self.cursor);
            self.cursor = new_cursor;
        }
    }

    fn kill_word_forward(&mut self) {
        let new_cursor = find_word_forward(&self.value, self.cursor);
        if new_cursor > self.cursor {
            self.save_undo();
            let killed = self.value[self.cursor..new_cursor].to_string();
            self.kill_ring.push(&killed, false, false);
            self.value.drain(self.cursor..new_cursor);
        }
    }

    fn kill_to_start(&mut self) {
        if self.cursor > 0 {
            self.save_undo();
            let killed = self.value[..self.cursor].to_string();
            self.kill_ring.push(&killed, false, false);
            self.value.drain(..self.cursor);
            self.cursor = 0;
        }
    }

    fn kill_to_end(&mut self) {
        if self.cursor < self.value.len() {
            self.save_undo();
            let killed = self.value[self.cursor..].to_string();
            self.kill_ring.push(&killed, false, false);
            self.value.truncate(self.cursor);
        }
    }

    fn yank(&mut self) {
        let text = self.kill_ring.peek().map(|s| s.to_string());
        if let Some(text) = text {
            self.save_undo();
            self.cursor += text.len();
            self.value.insert_str(self.cursor - text.len(), &text);
        }
    }

    fn yank_pop(&mut self) {
        self.kill_ring.rotate();
        // Undo previous yank and insert new one
        // Simplified: just pop undo and yank again
        let text = self.kill_ring.peek().map(|s| s.to_string());
        if let Some(prev) = self.undo_stack.pop()
            && let Some(text) = text
        {
            self.value = prev;
            self.cursor += text.len();
            self.value.insert_str(self.cursor - text.len(), &text);
        }
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.value = prev;
            self.cursor = self.value.len().min(self.cursor);
        }
    }
}

impl Component for Input {
    fn render(&self, width: usize) -> Vec<String> {
        let prompt_width = visible_width(&self.prompt);
        let avail = width.saturating_sub(prompt_width);

        if avail == 0 {
            return vec![self.prompt.clone()];
        }

        // Calculate visible window of text
        let _text_width = visible_width(&self.value);

        // Adjust scroll to keep cursor visible
        let cursor_text_width = visible_width(&self.value[..self.cursor]);
        let mut scroll = self.scroll_offset;

        if cursor_text_width < scroll {
            scroll = cursor_text_width;
        } else if cursor_text_width >= scroll + avail {
            scroll = cursor_text_width.saturating_sub(avail) + 1;
        }

        // Slice visible portion
        let visible = slice_by_column(&self.value, scroll, avail);
        let vis_width = visible_width(&visible);

        // Calculate cursor position in visible text
        let cursor_visible_pos = cursor_text_width.saturating_sub(scroll);

        // Build the line with cursor highlighting
        let mut line = self.prompt.clone();

        if self.focused && cursor_visible_pos < vis_width {
            // Split at cursor position
            let before = slice_by_column(&visible, 0, cursor_visible_pos);
            let at_cursor = slice_by_column(&visible, cursor_visible_pos, 1);
            let after = slice_by_column(&visible, cursor_visible_pos + 1, avail);

            // Emit cursor marker before the fake cursor for IME positioning
            // But only if the cursor position is within bounds
            line.push_str(CURSOR_MARKER);
            line.push_str(&before);
            line.push_str("\x1b[7m");
            if at_cursor.is_empty() {
                line.push(' ');
            } else {
                line.push_str(&at_cursor);
            }
            line.push_str("\x1b[27m");
            line.push_str(&after);
        } else if self.focused && cursor_visible_pos >= vis_width && vis_width < avail {
            // Cursor at end, past visible content
            line.push_str(CURSOR_MARKER);
            line.push_str(&visible);
            line.push_str("\x1b[7m \x1b[27m");
        } else {
            line.push_str(&visible);
            if self.focused {
                // Cursor at end
                line.push_str(CURSOR_MARKER);
            }
        }

        // Pad to width
        let line_width = visible_width(&line);
        if line_width < width {
            line.push_str(&" ".repeat(width - line_width));
        }

        vec![line]
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        // Printable characters
        if is_printable(key)
            && let Some(s) = key_event_to_string(key)
        {
            self.insert_text(&s);
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if matches_key(key, &Key::Enter) {
            if let Some(ref mut cb) = self.on_submit {
                let value = std::mem::take(&mut self.value);
                self.cursor = 0;
                self.scroll_offset = 0;
                cb(value);
            }
            return true;
        }

        if matches_key(key, &Key::Escape) {
            if let Some(ref mut cb) = self.on_escape {
                cb();
            }
            return true;
        }

        if matches_key(key, &Key::Backspace) {
            self.delete_before_cursor();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if matches_key(key, &Key::Delete) {
            self.delete_after_cursor();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if matches_key(key, &Key::Left) {
            self.move_cursor_left();
            return true;
        }

        if matches_key(key, &Key::Right) {
            self.move_cursor_right();
            return true;
        }

        if matches_key(key, &Key::Home) {
            self.move_to_start();
            return true;
        }

        if matches_key(key, &Key::End) {
            self.move_to_end();
            return true;
        }

        // Ctrl+key combinations
        if matches_key(key, &Key::Ctrl('b')) {
            self.move_cursor_left();
            return true;
        }

        if matches_key(key, &Key::Ctrl('f')) {
            self.move_cursor_right();
            return true;
        }

        if matches_key(key, &Key::Ctrl('a')) {
            self.move_to_start();
            return true;
        }

        if matches_key(key, &Key::Ctrl('e')) {
            self.move_to_end();
            return true;
        }

        if matches_key(key, &Key::Ctrl('w')) {
            self.kill_word_backward();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if matches_key(key, &Key::Ctrl('u')) {
            self.kill_to_start();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if matches_key(key, &Key::Ctrl('k')) {
            self.kill_to_end();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if matches_key(key, &Key::Ctrl('y')) {
            self.yank();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if matches_key(key, &Key::Alt('y')) {
            self.yank_pop();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if matches_key(key, &Key::Ctrl('z')) {
            self.undo();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if matches_key(key, &Key::Alt('d')) {
            self.kill_word_forward();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        // Ctrl+Left / Ctrl+Right for word movement
        if matches_key(key, &Key::CtrlLeft) {
            self.cursor = find_word_backward(&self.value, self.cursor);
            return true;
        }

        if matches_key(key, &Key::CtrlRight) {
            self.cursor = find_word_forward(&self.value, self.cursor);
            return true;
        }

        false
    }

    fn is_focusable(&self) -> bool {
        true
    }
}

impl Focusable for Input {
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn focused(&self) -> bool {
        self.focused
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_input_is_empty() {
        let input = Input::new();
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn test_insert_text() {
        let mut input = Input::new();
        input.insert_text("hello");
        assert_eq!(input.get_value(), "hello");
        assert_eq!(input.cursor, 5);
    }

    #[test]
    fn test_backspace() {
        let mut input = Input::new();
        input.insert_text("hello");
        input.delete_before_cursor();
        assert_eq!(input.get_value(), "hell");
        assert_eq!(input.cursor, 4);
    }

    #[test]
    fn test_move_cursor() {
        let mut input = Input::new();
        input.insert_text("hello");
        input.move_cursor_left();
        assert_eq!(input.cursor, 4);
        input.move_cursor_right();
        assert_eq!(input.cursor, 5);
    }

    #[test]
    fn test_set_value() {
        let mut input = Input::new();
        input.set_value("test");
        assert_eq!(input.get_value(), "test");
        assert_eq!(input.cursor, 4);
    }

    #[test]
    fn test_kill_to_end() {
        let mut input = Input::new();
        input.insert_text("hello world");
        // Move cursor to after "hello"
        for _ in 0..6 {
            input.move_cursor_left();
        }
        input.kill_to_end();
        assert_eq!(input.get_value(), "hello");
    }

    #[test]
    fn test_undo() {
        let mut input = Input::new();
        input.insert_text("hello");
        input.undo();
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn test_render_basic() {
        let mut input = Input::new();
        input.set_value("test");
        let lines = input.render(20);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("test"));
    }
}
