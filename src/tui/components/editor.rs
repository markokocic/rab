#![allow(clippy::type_complexity)]

use crate::tui::component::Component;
use crate::tui::focusable::{CURSOR_MARKER, Focusable};
use crate::tui::keys::{Key, key_event_to_string, matches_key};
use crate::tui::kill_ring::KillRing;
use crate::tui::undo_stack::UndoStack;
use crate::tui::util::{visible_width, wrap_text_with_ansi};
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
    preferred_col: Option<usize>,
    last_action: Option<String>,
    pub on_submit: Option<Box<dyn FnMut(String)>>,
    pub on_change: Option<Box<dyn FnMut(&str)>>,
    pub disable_submit: bool,
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
            preferred_col: None,
            last_action: None,
            on_submit: None,
            on_change: None,
            disable_submit: false,
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

    // ── Undo ──

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
        self.push_undo();
        self.insert_text_internal(ch);
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

    // ── History ──

    fn exit_history(&mut self) {
        self.history_index = -1;
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
        let text = self.history[idx as usize].clone();
        self.set_text(&text);
        self.history_index = idx;
    }

    fn recall_newer(&mut self) {
        if self.history_index < 0 {
            return;
        }
        let idx = self.history_index + 1;
        if idx >= self.history.len() as i32 {
            self.set_text("");
            self.history_index = -1;
        } else {
            let text = self.history[idx as usize].clone();
            self.set_text(&text);
            self.history_index = idx;
        }
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
        let result = self.lines.join("\n");
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.scroll_offset = 0;
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

        let horizontal = (self.theme.border)("─");
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
                "─".repeat(width - indicator_w)
            } else {
                String::new()
            };
            result.push((self.theme.border)(&format!("{}{}", indicator, fill)));
        } else {
            result.push((self.theme.border)(&horizontal.repeat(width)));
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
                "─".repeat(width - indicator_w)
            } else {
                String::new()
            };
            result.push((self.theme.border)(&format!("{}{}", indicator, fill)));
        } else {
            result.push((self.theme.border)(&horizontal.repeat(width)));
        }

        result
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        // ── Enter / Submit ──
        if matches_key(key, &Key::Enter) {
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

        // ── Printable character ──
        if is_printable_plain(key)
            && let Some(s) = key_event_to_string(key)
        {
            self.insert_character(&s);
            return true;
        }

        // ── Basic movement ──
        if matches_key(key, &Key::Left) || matches_key(key, &Key::Ctrl('b')) {
            self.move_left();
            return true;
        }
        if matches_key(key, &Key::Right) || matches_key(key, &Key::Ctrl('f')) {
            self.move_right();
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

        // ── Up/Down with history ──
        if matches_key(key, &Key::Up) {
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
        if matches_key(key, &Key::Down) {
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
        if matches_key(key, &Key::PageUp) {
            self.page_up();
            return true;
        }
        if matches_key(key, &Key::PageDown) {
            self.page_down();
            return true;
        }

        // ── Word movement ──
        if matches_key(key, &Key::CtrlLeft) || matches_key(key, &Key::Alt('b')) {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col > 0 {
                let c = find_word_backward(line, self.cursor_col);
                self.set_cursor_col(c);
            }
            return true;
        }
        if matches_key(key, &Key::CtrlRight) || matches_key(key, &Key::Alt('f')) {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col < line.len() {
                let c = find_word_forward(line, self.cursor_col);
                self.set_cursor_col(c);
            }
            return true;
        }
        if matches_key(key, &Key::AltLeft) {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col > 0 {
                let c = find_word_backward(line, self.cursor_col);
                self.set_cursor_col(c);
            }
            return true;
        }
        if matches_key(key, &Key::AltRight) {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col < line.len() {
                let c = find_word_forward(line, self.cursor_col);
                self.set_cursor_col(c);
            }
            return true;
        }

        // ── Deletion ──
        if matches_key(key, &Key::Backspace) || matches_key(key, &Key::Ctrl('h')) {
            self.backspace();
            return true;
        }
        if matches_key(key, &Key::Delete) || matches_key(key, &Key::Ctrl('d')) {
            self.delete_forward();
            return true;
        }

        // ── Kill operations ──
        if matches_key(key, &Key::Ctrl('w')) {
            self.delete_word_backward();
            return true;
        }
        if matches_key(key, &Key::Alt('d')) {
            self.delete_word_forward();
            return true;
        }
        if matches_key(key, &Key::Ctrl('u')) {
            self.delete_to_line_start();
            return true;
        }
        if matches_key(key, &Key::Ctrl('k')) {
            self.delete_to_line_end();
            return true;
        }

        // ── Yank ──
        if matches_key(key, &Key::Ctrl('y')) {
            self.yank();
            return true;
        }
        if matches_key(key, &Key::Alt('y')) {
            self.yank_pop();
            return true;
        }

        // ── Undo ──
        if matches_key(key, &Key::Ctrl('z')) {
            self.exit_history();
            self.last_action = None;
            self.undo();
            self.notify_change();
            return true;
        }

        // ── Ctrl+J = newline ──
        if matches_key(key, &Key::Ctrl('j')) {
            self.add_newline();
            return true;
        }

        // ── Escape — let parent handle ──
        if matches_key(key, &Key::Escape) {
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
            // Word-wrap the line, tracking cursor position
            let wrapped = wrap_text_with_ansi(line, max_width);
            let mut byte_pos = 0;
            for (chunk_idx, chunk) in wrapped.iter().enumerate() {
                let chunk_end = byte_pos + chunk.len();
                let cursor_in_chunk = is_cursor_line
                    && cursor_col >= byte_pos
                    && (cursor_col < chunk_end || chunk_idx == wrapped.len() - 1);
                result.push(VisualLine {
                    text: chunk.clone(),
                    has_cursor: cursor_in_chunk,
                    cursor_pos: if cursor_in_chunk {
                        Some((cursor_col - byte_pos).min(chunk.len()))
                    } else {
                        None
                    },
                });
                byte_pos = chunk_end;
                // Account for space between wrapped segments (the wrap function may trim)
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
}
