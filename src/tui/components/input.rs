#![allow(clippy::type_complexity)]

use crate::tui::component::Component;
use crate::tui::focusable::{CURSOR_MARKER, Focusable};
use crate::tui::keybindings::{
    ACTION_EDITOR_CURSOR_LEFT, ACTION_EDITOR_CURSOR_LINE_END, ACTION_EDITOR_CURSOR_LINE_START,
    ACTION_EDITOR_CURSOR_RIGHT, ACTION_EDITOR_CURSOR_WORD_LEFT, ACTION_EDITOR_CURSOR_WORD_RIGHT,
    ACTION_EDITOR_DELETE_CHAR_BACKWARD, ACTION_EDITOR_DELETE_CHAR_FORWARD,
    ACTION_EDITOR_DELETE_TO_LINE_END, ACTION_EDITOR_DELETE_TO_LINE_START,
    ACTION_EDITOR_DELETE_WORD_BACKWARD, ACTION_EDITOR_DELETE_WORD_FORWARD, ACTION_EDITOR_UNDO,
    ACTION_EDITOR_YANK, ACTION_EDITOR_YANK_POP, ACTION_INPUT_SUBMIT, ACTION_SELECT_CANCEL,
    get_keybindings,
};
use crate::tui::keys::key_event_to_string;
use crate::tui::kill_ring::KillRing;
use crate::tui::undo_stack::UndoStack;
use crate::tui::util::{slice_by_column, visible_width};
use crate::tui::word_nav::{find_word_backward, find_word_forward};
use crossterm::event::KeyEvent;
use unicode_segmentation::UnicodeSegmentation;

/// Single-line text input component.
///
/// Supports Emacs-style cursor movement and kill ring operations,
/// bracketed paste, and undo coalescing (pi fish-style).
pub struct Input {
    value: String,
    cursor: usize,
    prompt: String,
    kill_ring: KillRing,
    undo_stack: UndoStack<String>,
    focused: bool,
    on_submit: Option<Box<dyn FnMut(String)>>,
    on_escape: Option<Box<dyn FnMut()>>,
    on_change: Option<Box<dyn FnMut(&str)>>,

    // Undo coalescing (pi fish-style)
    last_action: Option<&'static str>,
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
            on_submit: None,
            on_escape: None,
            on_change: None,
            last_action: None,
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
        self.last_action = None;
        self.save_undo();
        self.value = value.to_string();
        self.cursor = self.value.len();
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

    // ── Undo coalescing (pi fish-style) ──

    fn maybe_push_undo(&mut self, char: &str) {
        use crate::tui::util::is_whitespace_char;
        // Consecutive word chars coalesce into one undo unit
        // Space captures state before itself (so undo removes space + following word together)
        if is_whitespace_char(char) || self.last_action != Some("type-word") {
            self.save_undo();
        }
        self.last_action = Some("type-word");
    }

    // ── Text insertion ──

    fn insert_text(&mut self, text: &str) {
        self.maybe_push_undo(text);
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    // ── Bracketed paste ──

    fn handle_paste(&mut self, pasted_text: &str) {
        self.last_action = None;
        self.save_undo();

        let clean = pasted_text.replace(['\r', '\n'], "").replace('\t', "    ");

        self.value = format!(
            "{}{}{}",
            &self.value[..self.cursor],
            clean,
            &self.value[self.cursor..]
        );
        self.cursor += clean.len();
    }

    // ── Deletion ──

    fn delete_before_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.last_action = None;
        self.save_undo();
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
    }

    fn delete_after_cursor(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.last_action = None;
        self.save_undo();
        let graphemes: Vec<(usize, &str)> = self.value.grapheme_indices(true).collect();
        for &(idx, g) in &graphemes {
            if idx >= self.cursor {
                self.value.drain(idx..idx + g.len());
                break;
            }
        }
    }

    // ── Cursor movement ──

    fn move_cursor_left(&mut self) {
        self.last_action = None;
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
        self.last_action = None;
        if self.cursor >= self.value.len() {
            return;
        }
        if let Some((idx, g)) = self.value[self.cursor..].grapheme_indices(true).next() {
            self.cursor += idx + g.len();
        }
    }

    fn move_to_start(&mut self) {
        self.last_action = None;
        self.cursor = 0;
    }

    fn move_to_end(&mut self) {
        self.last_action = None;
        self.cursor = self.value.len();
    }

    // ── Kill operations ──

    fn kill_word_backward(&mut self) {
        let new_cursor = find_word_backward(&self.value, self.cursor);
        if new_cursor < self.cursor {
            self.save_undo();
            let killed = self.value[new_cursor..self.cursor].to_string();
            let accumulate = self.last_action == Some("kill");
            self.kill_ring.push(&killed, true, accumulate);
            self.value.drain(new_cursor..self.cursor);
            self.cursor = new_cursor;
            self.last_action = Some("kill");
        }
    }

    fn kill_word_forward(&mut self) {
        let new_cursor = find_word_forward(&self.value, self.cursor);
        if new_cursor > self.cursor {
            self.save_undo();
            let killed = self.value[self.cursor..new_cursor].to_string();
            let accumulate = self.last_action == Some("kill");
            self.kill_ring.push(&killed, false, accumulate);
            self.value.drain(self.cursor..new_cursor);
            self.last_action = Some("kill");
        }
    }

    fn kill_to_start(&mut self) {
        if self.cursor > 0 {
            self.save_undo();
            let killed = self.value[..self.cursor].to_string();
            let accumulate = self.last_action == Some("kill");
            self.kill_ring.push(&killed, true, accumulate);
            self.value.drain(..self.cursor);
            self.cursor = 0;
            self.last_action = Some("kill");
        }
    }

    fn kill_to_end(&mut self) {
        if self.cursor < self.value.len() {
            self.save_undo();
            let killed = self.value[self.cursor..].to_string();
            let accumulate = self.last_action == Some("kill");
            self.kill_ring.push(&killed, false, accumulate);
            self.value.truncate(self.cursor);
            self.last_action = Some("kill");
        }
    }

    // ── Yank ──

    fn yank(&mut self) {
        let text = self.kill_ring.peek().map(|s| s.to_string());
        if let Some(text) = text {
            self.save_undo();
            self.cursor += text.len();
            self.value.insert_str(self.cursor - text.len(), &text);
        }
        self.last_action = Some("yank");
    }

    fn yank_pop(&mut self) {
        // Must follow yank() — save current state, delete previously yanked
        // text, rotate ring, insert new entry. Matches pi's yankPop().
        if self.kill_ring.len() <= 1 {
            return;
        }
        let prev = self.kill_ring.peek().map(|s| s.to_string());
        if let Some(ref prev_text) = prev {
            self.save_undo();
            if self.cursor >= prev_text.len() {
                let before = self.value[..self.cursor - prev_text.len()].to_string();
                let after = self.value[self.cursor..].to_string();
                self.value = format!("{}{}", before, after);
                self.cursor -= prev_text.len();
            }
        }
        self.kill_ring.rotate();
        let text = self.kill_ring.peek().map(|s| s.to_string());
        if let Some(ref new_text) = text {
            self.value.insert_str(self.cursor, new_text);
            self.cursor += new_text.len();
        }
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.value = prev;
            self.cursor = self.value.len().min(self.cursor);
            self.last_action = None;
        }
    }
}

impl Component for Input {
    fn render(&mut self, width: usize) -> Vec<String> {
        let prompt_width = visible_width(&self.prompt);
        let avail = width.saturating_sub(prompt_width);

        if avail == 0 {
            return vec![self.prompt.clone()];
        }

        let total_width = visible_width(&self.value);
        let cursor_text_width = visible_width(&self.value[..self.cursor]);

        // Pi-style smart horizontal scroll: center cursor in half-width window
        let scroll = if total_width < avail {
            0
        } else if self.cursor == self.value.len() {
            // Cursor at end: show end of text
            total_width.saturating_sub(avail).saturating_sub(1)
        } else {
            // Pi: center cursor in half-width window
            let half = avail / 2;
            if cursor_text_width < half {
                0
            } else if cursor_text_width > total_width.saturating_sub(half) {
                total_width.saturating_sub(avail)
            } else {
                cursor_text_width.saturating_sub(half)
            }
        };

        // Slice visible portion
        let visible = slice_by_column(&self.value, scroll, avail);
        let vis_width = visible_width(&visible);
        let cursor_visible_pos = cursor_text_width.saturating_sub(scroll);

        // Build the line with cursor highlighting
        let mut line = self.prompt.clone();

        if self.focused && cursor_visible_pos < vis_width {
            let before = slice_by_column(&visible, 0, cursor_visible_pos);
            let at_cursor = slice_by_column(&visible, cursor_visible_pos, 1);
            let after = slice_by_column(&visible, cursor_visible_pos + 1, avail);

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
            line.push_str(CURSOR_MARKER);
            line.push_str(&visible);
            line.push_str("\x1b[7m \x1b[27m");
        } else {
            line.push_str(&visible);
            if self.focused {
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
        let kb = get_keybindings();

        // Printable characters
        if crate::tui::keys::is_printable(key)
            && let Some(s) = key_event_to_string(key)
        {
            self.insert_text(&s);
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if kb.matches(key, ACTION_INPUT_SUBMIT) {
            if let Some(ref mut cb) = self.on_submit {
                let value = std::mem::take(&mut self.value);
                self.cursor = 0;
                self.last_action = None;
                cb(value);
            }
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CANCEL) {
            if let Some(ref mut cb) = self.on_escape {
                cb();
            }
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_BACKWARD) {
            self.delete_before_cursor();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_FORWARD) {
            self.delete_after_cursor();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_CURSOR_LEFT) {
            self.move_cursor_left();
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_CURSOR_RIGHT) {
            self.move_cursor_right();
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_CURSOR_LINE_START) {
            self.move_to_start();
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_CURSOR_LINE_END) {
            self.move_to_end();
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_DELETE_WORD_BACKWARD) {
            self.kill_word_backward();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_DELETE_TO_LINE_START) {
            self.kill_to_start();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_DELETE_TO_LINE_END) {
            self.kill_to_end();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_YANK) {
            self.yank();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_YANK_POP) {
            self.yank_pop();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_UNDO) {
            self.undo();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_DELETE_WORD_FORWARD) {
            self.kill_word_forward();
            if let Some(ref mut cb) = self.on_change {
                cb(&self.value);
            }
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_CURSOR_WORD_LEFT) {
            self.cursor = find_word_backward(&self.value, self.cursor);
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_CURSOR_WORD_RIGHT) {
            self.cursor = find_word_forward(&self.value, self.cursor);
            return true;
        }

        false
    }

    fn handle_paste(&mut self, text: &str) {
        self.handle_paste(text);
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

    #[test]
    fn test_undo_coalescing() {
        let mut input = Input::new();
        input.insert_text("h");
        input.insert_text("e");
        input.insert_text(" ");
        input.insert_text("w");
        assert_eq!(input.get_value(), "he w");
        // Undo once reverts to before space ("he w" → "he").
        input.undo();
        assert_eq!(input.get_value(), "he");
        // Undo again reverts to before everything ("he" → "")
        input.undo();
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn test_paste_handling() {
        let mut input = Input::new();
        input.handle_paste("hello\nworld");
        // Newlines should be stripped in paste
        assert_eq!(input.get_value(), "helloworld");
        assert_eq!(input.cursor, 10);
    }
}
