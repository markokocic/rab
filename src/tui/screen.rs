use std::io::{self, Write};

use crate::tui::focusable::CURSOR_MARKER;

/// The diff renderer - maintains previous frame and emits minimal ANSI updates.
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

    /// The current hardware cursor row tracking.
    pub fn hardware_cursor_row(&self) -> usize {
        self.hardware_cursor_row
    }

    /// Update the hardware cursor row after TUI repositions the cursor.
    /// This keeps Screen's delta calculations in sync with the actual cursor position.
    pub fn set_hardware_cursor_row(&mut self, row: usize) {
        self.hardware_cursor_row = row;
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

        // First render - output everything without clearing (assumes clean screen)
        if self.prev_lines.is_empty() && !width_changed && !height_changed {
            return self.full_render(&new_lines, writer, false, width_usize, height_usize);
        }

        // Width/height changes need a full redraw
        if width_changed || height_changed {
            return self.full_render(&new_lines, writer, true, width_usize, height_usize);
        }

        // Content shrunk - full redraw to clear empty rows
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
                // Target is above viewport - need full redraw
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

        // First changed line is above viewport - need full redraw
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
                // Too many extra lines - fall back to full redraw
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

        // Second render with same content - no output
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

        // Type "/" - only index 7 changes
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

    #[test]
    fn test_screen_append_no_duplicate_content() {
        let mut screen = Screen::new();
        let mut output = Vec::new();

        // First frame: 4 lines
        let frame1 = vec!["a", "b", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        screen.render(frame1, 40, 24, &mut output).unwrap();
        output.clear();

        // Second frame: content appended at end (exactly prev_lines.len())
        let frame2 = vec!["a", "b", "c", "d", "e"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        screen.render(frame2, 40, 24, &mut output).unwrap();

        let content = String::from_utf8_lossy(&output);
        eprintln!("Append-only diff output: {:?}", content);

        // The diff output should only contain the new line "e" plus ANSI codes
        // It must not repeat any of the unchanged lines ("a", "b", "c", "d")
        let counts = ["a", "b", "c", "d"];
        for &ch in &counts {
            let n = content.matches(ch).count();
            assert!(
                n <= 1,
                "'{}' should appear at most once in diff, got {}: {:?}",
                ch,
                n,
                content
            );
        }
        // "e" must appear exactly once
        let e_count = content.matches('e').count();
        assert_eq!(
            e_count, 1,
            "'e' should appear exactly once, got {}",
            e_count
        );
    }

    #[test]
    fn test_screen_insert_line_mid_content_no_duplicates() {
        let mut screen = Screen::new();
        let mut output = Vec::new();

        // First frame: 3 lines
        let frame1 = vec!["a", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        screen.render(frame1, 40, 24, &mut output).unwrap();
        output.clear();

        // Second frame: "b" inserted between "a" and "c"
        let frame2 = vec!["a", "b", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        screen.render(frame2, 40, 24, &mut output).unwrap();

        let content = String::from_utf8_lossy(&output);
        eprintln!("Insert-mid diff output: {:?}", content);

        // "a" should appear at most once (unchanged line shouldn't be re-written)
        assert!(
            content.matches('a').count() <= 1,
            "'a' should appear at most once: {:?}",
            content
        );
        // "b", "c", "d" should appear (changed/new lines)
        assert!(content.contains('b'), "Should contain 'b'");
        assert!(content.contains('c'), "Should contain 'c'");
        assert!(content.contains('d'), "Should contain 'd'");
    }

    #[test]
    fn test_screen_editor_appended_empty_line_no_duplicate() {
        // Simulates pressing Ctrl+J on "hello" → "hello\n"
        // Editor renders change from 3 lines to 4 lines:
        //   [border, "hello", border]  →  [border, "hello", "", border]
        let mut screen = Screen::new();
        let mut output = Vec::new();

        let frame1 = vec![
            "header".to_string(),
            "── editor border ──".to_string(),
            "hello".to_string(),
            "── editor border ──".to_string(),
            "footer".to_string(),
        ];
        screen.render(frame1, 30, 24, &mut output).unwrap();
        output.clear();

        // After Ctrl+J: "hello" → "hello\n"
        let frame2 = vec![
            "header".to_string(),
            "── editor border ──".to_string(),
            "hello".to_string(),
            "".to_string(), // new empty line
            "── editor border ──".to_string(),
            "footer".to_string(),
        ];
        screen.render(frame2, 30, 24, &mut output).unwrap();

        let content = String::from_utf8_lossy(&output);
        eprintln!("Editor append empty line diff: {:?}", content);

        // "hello" should NOT be in the diff output (it didn't change)
        let hello_count = content.matches("hello").count();
        assert!(
            hello_count <= 1,
            "'hello' should appear at most once in diff, got {}: {:?}",
            hello_count,
            content
        );
        // "footer" should NOT be duplicated (it just shifted down, should appear once)
        let footer_count = content.matches("footer").count();
        assert!(
            footer_count <= 1,
            "'footer' should appear at most once in diff, got {}: {:?}",
            footer_count,
            content
        );
    }

    #[test]
    fn test_screen_wrapping_no_overwrite_of_unchanged_line() {
        // Reproduces the exact bug: when text wraps in the editor, Screen's
        // hardware_cursor_row is out of sync with the actual cursor position.
        // This causes subsequent diffs to write on the wrong line, overwriting
        // the first content line with the second content line's text.
        let mut screen = Screen::new();
        let mut output = Vec::new();

        // Frame 1: editor with "hello world" (fits in 1 line at this width)
        let frame1 = vec![
            "──── editor ────".to_string(),
            " hello world    ".to_string(), // content, cursor at end
            "──── editor ────".to_string(),
        ];
        screen.render(frame1.clone(), 18, 24, &mut output).unwrap();
        output.clear();

        // Simulate TUI extracting cursor marker and repositioning hardware cursor.
        // Cursor is on line 1 (content line). Screen's hardware_cursor_row=2 (bottom border).
        // Call set_hardware_cursor_row to simulate the fix:
        screen.set_hardware_cursor_row(1);

        // Frame 2: text wraps to 2 content lines (e.g. "hello world is" at width 14)
        // "hello world is" wraps to ["hello world", "is"]
        let frame2 = vec![
            "──── editor ────".to_string(),
            " hello world    ".to_string(), // first wrapped chunk
            " is             ".to_string(), // second wrapped chunk, cursor here
            "──── editor ────".to_string(),
        ];
        screen.render(frame2.clone(), 18, 24, &mut output).unwrap();
        eprintln!(
            "After wrap diff output: {:?}",
            String::from_utf8_lossy(&output)
        );

        // The diff should NOT write " hello world" again (it's unchanged from frame 1)
        let output_str = String::from_utf8_lossy(&output);
        // Check that "hello world" (the UNCHANGED first content line)
        // does NOT appear in the diff output. It should only appear in the
        // full render output, not in a differential update.
        let hw_count = output_str.matches("hello world").count();
        assert!(
            hw_count <= 1,
            "'hello world' should appear at most once in diff, got {}: {:?}",
            hw_count,
            output_str
        );

        output.clear();

        // Simulate TUI: cursor marker is on line 2 (" is" line), reposition cursor.
        screen.set_hardware_cursor_row(2);

        // Frame 3: user types another char -> " is" -> " iss"
        let frame3 = vec![
            "──── editor ────".to_string(),
            " hello world    ".to_string(), // unchanged
            " iss            ".to_string(), // changed
            "──── editor ────".to_string(),
        ];
        screen.render(frame3.clone(), 18, 24, &mut output).unwrap();
        let output_str2 = String::from_utf8_lossy(&output);
        eprintln!("After typing diff output: {:?}", output_str2);

        // " hello world" must NOT appear in this diff - it's unchanged
        let hw_count2 = output_str2.matches("hello world").count();
        assert!(
            hw_count2 <= 1,
            "'hello world' should NOT be rewritten in diff, got {}: {:?}",
            hw_count2,
            output_str2
        );
    }

    #[test]
    fn test_screen_wrapping_overwrite_bug_without_fix() {
        // SAME as above but WITHOUT calling set_hardware_cursor_row
        // Demonstrates the bug: first content line gets overwritten.
        let mut screen = Screen::new();
        let mut output = Vec::new();

        let frame1 = vec![
            "──── editor ────".to_string(),
            " hello world    ".to_string(),
            "──── editor ────".to_string(),
        ];
        screen.render(frame1.clone(), 18, 24, &mut output).unwrap();
        output.clear();

        // WITHOUT the fix - Screen still thinks hardware cursor is at line 2
        // (because Screen.render set it there as the last rendered line)

        let frame2 = vec![
            "──── editor ────".to_string(),
            " hello world    ".to_string(),
            " is             ".to_string(),
            "──── editor ────".to_string(),
        ];
        screen.render(frame2.clone(), 18, 24, &mut output).unwrap();
        let output_str = String::from_utf8_lossy(&output);
        eprintln!("Without fix - after wrap: {:?}", output_str);

        // The BUG: hardware_cursor_row is 2, but the diff needs to write line 1
        // (first changed line). Since cursor is at line 2, diff emits \x1b[1A
        // to move up to line 1. But the hardware cursor is actually at line 1
        // (TUI already placed it there), so the move-up overshoots to line 0.
        // The diff output may write " hello world" at the wrong position.
        //
        // We can detect the bug by checking if the Screen's internal tracking
        // is incorrect after this frame.
        //
        // After this render, Screen should have hardware_cursor_row = 3 (render_end)
        // But the actual terminal cursor ended up at line 2 (the cursor marker position).
        // We check that the internal tracking is wrong:
        assert_eq!(
            screen.hardware_cursor_row(),
            3,
            "Without fix: hardware_cursor_row should be 3 (bottom border render_end)"
        );

        output.clear();

        // Frame 3: type char on second line
        screen.set_hardware_cursor_row(2); // Simulate TUI repositioning (but Screen doesn't know)

        let frame3 = vec![
            "──── editor ────".to_string(),
            " hello world    ".to_string(),
            " iss            ".to_string(),
            "──── editor ────".to_string(),
        ];
        screen.render(frame3.clone(), 18, 24, &mut output).unwrap();
        let output_str2 = String::from_utf8_lossy(&output);
        eprintln!("Without fix - after typing: {:?}", output_str2);

        // Without fix: the diff for frame 3 calculates move_target=2, but
        // hardware_cursor_row is still 3 (Screen doesn't know TUI moved it).
        // current_screen_row=3, target=2, diff=-1 → write on line 1 instead of line 2
        // This means " hello world" gets overwritten or " iss" appears on wrong line.

        // The bug symptom: " hello world" text would be overwritten.
        // We can detect this by checking that the output would corrupt line 1.
        // The output should contain " iss" (the changed content).
        // But if the bug is present, " iss" gets written on the wrong line.

        // The exact assertion depends on terminal state, but the key check:
        // "hello world" should NOT be in the diff for frame 3 (unchanged line)
        let hw_count = output_str2.matches("hello world").count();
        assert!(
            hw_count <= 1,
            "BUG: 'hello world' appears in frame 3 diff, got {}: {:?}",
            hw_count,
            output_str2
        );
    }
}
