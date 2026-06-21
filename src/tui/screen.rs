use std::io::{self, Write};

use crate::tui::focusable::CURSOR_MARKER;

/// The diff renderer — maintains previous frame and emits minimal ANSI updates.
pub struct Screen {
    prev_lines: Vec<String>,
    prev_width: u16,
    prev_height: u16,
    cursor_row: usize,
    hardware_cursor_row: usize,
    prev_viewport_top: usize,
    max_lines_rendered: usize,
    full_redraw_count: usize,
    clear_on_shrink: bool,
}

impl Screen {
    pub fn new() -> Self {
        Self {
            prev_lines: Vec::new(),
            prev_width: 0,
            prev_height: 0,
            cursor_row: 0,
            hardware_cursor_row: 0,
            prev_viewport_top: 0,
            max_lines_rendered: 0,
            full_redraw_count: 0,
            clear_on_shrink: true,
        }
    }

    /// Viewport top position (first visible line in terminal)
    pub fn prev_viewport_top(&self) -> usize {
        self.prev_viewport_top
    }

    pub fn prev_width(&self) -> usize {
        self.prev_width as usize
    }

    pub fn prev_height(&self) -> usize {
        self.prev_height as usize
    }

    pub fn full_redraw_count(&self) -> usize {
        self.full_redraw_count
    }

    /// Total number of lines in the last rendered frame.
    pub fn total_lines(&self) -> usize {
        self.prev_lines.len()
    }

    /// Move cursor to one line past all rendered content (for clean program exit).
    /// Writes the ANSI cursor-positioning sequences and `\r\n` so that subsequent
    /// shell output appears on a fresh line after all TUI content.
    pub fn finalize(&mut self, writer: &mut dyn Write) -> io::Result<()> {
        if self.prev_lines.is_empty() {
            return Ok(());
        }
        let target_row = self.prev_lines.len(); // one past the last content line
        let line_diff = target_row as i64 - self.hardware_cursor_row as i64;
        let mut buf = String::new();
        if line_diff > 0 {
            buf.push_str(&format!("\x1b[{}B", line_diff));
        } else if line_diff < 0 {
            buf.push_str(&format!("\x1b[{}A", -line_diff));
        }
        buf.push_str("\r\n");
        write!(writer, "{}", buf)?;
        writer.flush()?;
        Ok(())
    }

    pub fn set_clear_on_shrink(&mut self, enabled: bool) {
        self.clear_on_shrink = enabled;
    }

    fn full_render(
        &mut self,
        lines: &[String],
        w: &mut dyn Write,
        clear: bool,
        width: usize,
        height: usize,
    ) -> io::Result<()> {
        self.full_redraw_count += 1;
        let mut buf = String::new();

        if clear {
            buf.push_str("\x1b[2J\x1b[H\x1b[3J");
        }

        if lines.is_empty() {
            buf.push_str("\x1b[?2026h");
            buf.push_str("\x1b[?2026l");
            write!(w, "{}", buf)?;
            w.flush()?;
            self.cursor_row = 0;
            self.hardware_cursor_row = 0;
            self.max_lines_rendered = 0;
            self.prev_viewport_top = 0;
            self.prev_lines = lines.to_vec();
            self.prev_width = width as u16;
            self.prev_height = height as u16;
            return Ok(());
        }

        buf.push_str("\x1b[?2026h");

        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                buf.push_str("\r\n");
            }
            buf.push_str(line);
        }

        buf.push_str("\x1b[?2026l");
        write!(w, "{}", buf)?;
        w.flush()?;

        self.cursor_row = lines.len().saturating_sub(1);
        self.hardware_cursor_row = self.cursor_row;
        if clear {
            self.max_lines_rendered = lines.len();
        } else {
            self.max_lines_rendered = self.max_lines_rendered.max(lines.len());
        }
        let buffer_len = height.max(lines.len());
        self.prev_viewport_top = buffer_len.saturating_sub(height);
        self.prev_lines = lines.to_vec();
        self.prev_width = width as u16;
        self.prev_height = height as u16;

        Ok(())
    }

    /// Render new lines to the terminal using differential updates.
    /// `writer` should be the terminal's stdout (in raw mode).
    /// `width` and `height` are the current terminal dimensions.
    pub fn render(
        &mut self,
        new_lines: Vec<String>,
        width: u16,
        height: u16,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let width_usize = width as usize;
        let height_usize = height as usize;
        let width_changed = self.prev_width != 0 && self.prev_width as usize != width_usize;
        let height_changed = self.prev_height != 0 && self.prev_height as usize != height_usize;
        let prev_buffer_len = if self.prev_height > 0 {
            self.prev_viewport_top + self.prev_height as usize
        } else {
            height_usize
        };
        let prev_viewport_top = if height_changed {
            prev_buffer_len.saturating_sub(height_usize)
        } else {
            self.prev_viewport_top
        };
        let mut viewport_top = prev_viewport_top;

        // First render — output everything without clearing (assumes clean screen)
        if self.prev_lines.is_empty() && !width_changed && !height_changed {
            return self.full_render(&new_lines, writer, false, width_usize, height_usize);
        }

        // Width/height changes need a full redraw
        if width_changed || height_changed {
            return self.full_render(&new_lines, writer, true, width_usize, height_usize);
        }

        // Content shrunk — full redraw to clear empty rows
        if self.clear_on_shrink && new_lines.len() < self.max_lines_rendered {
            return self.full_render(&new_lines, writer, true, width_usize, height_usize);
        }

        // Find changed range
        let mut first_changed: i32 = -1;
        let mut last_changed: i32 = -1;
        let max_lines = new_lines.len().max(self.prev_lines.len());
        for i in 0..max_lines {
            let old = if i < self.prev_lines.len() {
                &self.prev_lines[i]
            } else {
                ""
            };
            let new = if i < new_lines.len() {
                &new_lines[i]
            } else {
                ""
            };
            if old != new {
                if first_changed == -1 {
                    first_changed = i as i32;
                }
                last_changed = i as i32;
            }
        }

        let appended = new_lines.len() > self.prev_lines.len();
        if appended && first_changed == -1 {
            first_changed = self.prev_lines.len() as i32;
            last_changed = new_lines.len() as i32 - 1;
        }

        // No changes
        if first_changed == -1 {
            self.prev_height = height_usize as u16;
            self.prev_viewport_top = prev_viewport_top;
            return Ok(());
        }

        // All changes are in deleted lines
        let first = first_changed as usize;
        let last = last_changed as usize;
        if first >= new_lines.len() {
            let mut buf = String::new();

            // Move cursor to end of new content
            let target_row = new_lines.len().saturating_sub(1);
            let line_diff = if target_row >= prev_viewport_top {
                (target_row - prev_viewport_top) as i32
                    - (self.hardware_cursor_row.saturating_sub(prev_viewport_top)) as i32
            } else {
                // Target is above viewport — need full redraw
                return self.full_render(&new_lines, writer, true, width_usize, height_usize);
            };

            buf.push_str("\x1b[?2026h");

            if line_diff > 0 {
                buf.push_str(&format!("\x1b[{}B", line_diff));
            } else if line_diff < 0 {
                buf.push_str(&format!("\x1b[{}A", -line_diff));
            }
            buf.push('\r');

            // Clear extra lines
            let extra = self.prev_lines.len().saturating_sub(new_lines.len());
            if extra > height_usize {
                return self.full_render(&new_lines, writer, true, width_usize, height_usize);
            }
            if extra > 0 && !new_lines.is_empty() {
                buf.push_str("\x1b[1B");
            }
            for i in 0..extra {
                buf.push_str("\r\x1b[2K");
                if i + 1 < extra {
                    buf.push_str("\x1b[1B");
                }
            }
            let move_back = extra.saturating_sub(1) + if new_lines.is_empty() { 0 } else { 1 };
            if move_back > 0 {
                buf.push_str(&format!("\x1b[{}A", move_back));
            }

            buf.push_str("\x1b[?2026l");
            write!(writer, "{}", buf)?;
            writer.flush()?;

            self.cursor_row = target_row;
            self.hardware_cursor_row = target_row;
            self.prev_lines = new_lines;
            self.prev_viewport_top = prev_viewport_top;
            self.prev_height = height_usize as u16;
            return Ok(());
        }

        // First changed line is above viewport — need full redraw
        if first < prev_viewport_top {
            return self.full_render(&new_lines, writer, true, width_usize, height_usize);
        }

        // Differential render: update changed lines in place
        let mut buf = String::new();
        buf.push_str("\x1b[?2026h");

        let move_target = if appended && first == self.prev_lines.len() && first > 0 {
            first - 1
        } else {
            first
        };

        // Handle scrolling if needed
        let prev_viewport_bottom = prev_viewport_top + height_usize - 1;
        if move_target > prev_viewport_bottom {
            let scroll = move_target - prev_viewport_bottom;
            // Move to bottom of screen
            let current_screen_row =
                (self.hardware_cursor_row.saturating_sub(prev_viewport_top)).min(height_usize - 1);
            let to_bottom = height_usize - 1 - current_screen_row;
            if to_bottom > 0 {
                buf.push_str(&format!("\x1b[{}B", to_bottom));
            }
            // Scroll
            for _ in 0..scroll {
                buf.push_str("\r\n");
            }
            self.hardware_cursor_row = move_target;
            // Advance viewport_top to reflect the scroll (lines scrolled off top)
            viewport_top += scroll;
        }

        // Move to first changed line
        // Use viewport_top (potentially updated by scroll) for both calculations
        // so they stay consistent even after content scrolled below viewport.
        let current_screen_row = self.hardware_cursor_row.saturating_sub(viewport_top);
        let target_screen_row = move_target.saturating_sub(viewport_top);
        let line_diff = target_screen_row as i32 - current_screen_row as i32;

        if line_diff > 0 {
            buf.push_str(&format!("\x1b[{}B", line_diff));
        } else if line_diff < 0 {
            buf.push_str(&format!("\x1b[{}A", -line_diff));
        }

        if appended && first == self.prev_lines.len() {
            buf.push_str("\r\n");
        } else {
            buf.push('\r');
        }

        // Write changed lines
        let mut render_end = last.min(new_lines.len() - 1);
        for (i, line) in new_lines
            .iter()
            .enumerate()
            .skip(first)
            .take(render_end + 1 - first)
        {
            if i > first {
                buf.push_str("\r\n");
            }

            // Extract cursor marker if present
            let line_without_marker = if line.contains(CURSOR_MARKER) {
                line.replace(CURSOR_MARKER, "")
            } else {
                line.clone()
            };

            buf.push_str("\x1b[2K"); // clear line
            buf.push_str(&line_without_marker);
        }

        // Clear any trailing old lines beyond the new content.
        // This is needed when content shrinks (e.g. autocomplete list narrows)
        // and clear_on_shrink is disabled (the app sets it to false to avoid
        // full redraws during streaming).
        if new_lines.len() < self.prev_lines.len() {
            let extra = self.prev_lines.len() - new_lines.len();

            if extra > height_usize {
                // Too many extra lines — fall back to full redraw
                buf.push_str("\x1b[?2026l");
                write!(writer, "{}", buf)?;
                writer.flush()?;
                return self.full_render(&new_lines, writer, true, width_usize, height_usize);
            }

            // Move from render_end to the first extra line = new_lines.len()
            let move_to_first_extra = new_lines.len() - render_end;
            if move_to_first_extra > 0 {
                buf.push_str(&format!("\x1b[{}B", move_to_first_extra));
            }

            // Clear each extra line
            for i in 0..extra {
                buf.push_str("\r\x1b[2K");
                if i + 1 < extra {
                    buf.push_str("\x1b[1B");
                }
            }

            // Move cursor back to new_lines.len() - 1 (end of new content).
            // After the last clear, cursor is at prev_lines.len() - 1.
            // Set render_end to final cursor position since we're moving there.
            if extra > 0 {
                buf.push_str(&format!("\x1b[{}A", extra));
                render_end = new_lines.len().saturating_sub(1);
            }
        }

        buf.push_str("\x1b[?2026l");
        write!(writer, "{}", buf)?;
        writer.flush()?;

        self.cursor_row = new_lines.len().saturating_sub(1);
        self.hardware_cursor_row = render_end;
        self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len());
        self.prev_lines = new_lines;
        // Advance viewport_top if cursor ended up below the viewport
        // (matching pi's Math.max(prevViewportTop, finalCursorRow - height + 1)).
        self.prev_viewport_top = viewport_top.max(render_end.saturating_sub(height_usize - 1));
        self.prev_height = height_usize as u16;
        self.prev_width = width_usize as u16;

        Ok(())
    }
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_screen() {
        let screen = Screen::new();
        assert_eq!(screen.full_redraw_count(), 0);
    }

    #[test]
    fn test_clear_on_shrink_default() {
        let screen = Screen::new();
        assert!(screen.clear_on_shrink);
    }

    #[test]
    fn test_first_render() {
        let mut screen = Screen::new();
        let lines = vec!["hello".to_string(), "world".to_string()];
        let mut output = Vec::new();

        screen.render(lines.clone(), 80, 24, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        // Should have synchronized output markers
        assert!(output_str.contains("\x1b[?2026h"));
        assert!(output_str.contains("hello"));
        assert!(output_str.contains("world"));
        assert!(output_str.contains("\x1b[?2026l"));
    }

    #[test]
    fn test_differential_update() {
        let mut screen = Screen::new();
        let mut output = Vec::new();

        // First render
        let lines1 = vec!["hello".to_string(), "world".to_string()];
        screen.render(lines1.clone(), 80, 24, &mut output).unwrap();
        output.clear();

        // Second render with same content — no output
        screen.render(lines1.clone(), 80, 24, &mut output).unwrap();
        assert!(output.is_empty());

        // Third render with changed content
        let lines2 = vec!["hello".to_string(), "rust".to_string()];
        screen.render(lines2.clone(), 80, 24, &mut output).unwrap();
        let output_str = String::from_utf8(output.clone()).unwrap();
        assert!(output_str.contains("rust"));
    }

    #[test]
    fn test_type_character_single_line_change() {
        let mut screen = Screen::new();
        let mut output = Vec::new();

        // Simulate compose_ui: 12 lines, editor content at index 7
        let mut initial: Vec<String> = Vec::new();
        for i in 0..12 {
            initial.push(format!("line {:02}", i));
        }
        screen.render(initial.clone(), 40, 24, &mut output).unwrap();
        output.clear();

        // Type "/" — only index 7 changes
        let mut after = initial.clone();
        after[7] = "line 07/".to_string();
        screen.render(after, 40, 24, &mut output).unwrap();

        let text = String::from_utf8_lossy(&output);
        // Should contain the changed text
        assert!(
            text.contains("line 07/"),
            "Missing changed text in: {}",
            text
        );
        // Should NOT do a full clear
        assert!(
            !text.contains("\x1b[2J"),
            "Should not full-clear on single line change"
        );
    }
}
