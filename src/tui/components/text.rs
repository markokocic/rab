#![allow(clippy::type_complexity)]

use crate::tui::Component;
use crate::tui::util::{visible_width, wrap_text_with_ansi};

/// Multi-line text component with word wrapping and padding.
pub struct Text {
    content: String,
    padding_x: usize,
    padding_y: usize,
    bg_fn: Option<Box<dyn Fn(&str) -> String>>,
    cached_width: Option<usize>,
    cached_lines: Vec<String>,
}

impl Text {
    /// Create a new Text component.
    ///
    /// - `content`: The text to display (may contain newlines).
    /// - `padding_x`: Horizontal padding (spaces on left and right).
    /// - `padding_y`: Vertical padding (empty lines above and below).
    /// - `bg_fn`: Optional background color function applied to each line.
    pub fn new(
        content: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        bg_fn: Option<Box<dyn Fn(&str) -> String>>,
    ) -> Self {
        Self {
            content: content.into(),
            padding_x,
            padding_y,
            bg_fn,
            cached_width: None,
            cached_lines: Vec::new(),
        }
    }

    /// Update the text content.
    pub fn set_text(&mut self, content: impl Into<String>) {
        self.content = content.into();
        self.cached_width = None;
    }

    /// Set the background color function.
    pub fn set_bg_fn(&mut self, bg_fn: Option<Box<dyn Fn(&str) -> String>>) {
        self.bg_fn = bg_fn;
        self.cached_width = None;
    }
}

impl Component for Text {
    fn render(&self, width: usize) -> Vec<String> {
        if self.cached_width == Some(width) {
            return self.cached_lines.clone();
        }

        let inner_width = width.saturating_sub(2 * self.padding_x);
        if inner_width == 0 {
            let empty = " ".repeat(width);
            return vec![empty];
        }

        let padding_str = " ".repeat(self.padding_x);
        let pad_right = " ".repeat(width.saturating_sub(self.padding_x));

        let mut lines = Vec::new();

        // Top padding
        for _ in 0..self.padding_y {
            lines.push(self.apply_bg(&format!("{}{}", padding_str, pad_right), width));
        }

        // Content with word wrapping
        let wrapped = wrap_text_with_ansi(&self.content, inner_width);
        for line in wrapped {
            let padded = format!("{}{}", padding_str, line);
            let padded_line = if visible_width(&padded) < width {
                format!("{}{}", padded, " ".repeat(width - visible_width(&padded)))
            } else {
                padded
            };
            lines.push(self.apply_bg(&padded_line, width));
        }

        // Bottom padding
        for _ in 0..self.padding_y {
            lines.push(self.apply_bg(&format!("{}{}", padding_str, pad_right), width));
        }

        lines
    }

    fn invalidate(&mut self) {
        self.cached_width = None;
    }
}

impl Text {
    fn apply_bg(&self, line: &str, width: usize) -> String {
        if let Some(ref bg_fn) = self.bg_fn {
            let padded = if visible_width(line) < width {
                format!("{}{}", line, " ".repeat(width - visible_width(line)))
            } else {
                line.to_string()
            };
            bg_fn(&padded)
        } else {
            line.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_render() {
        let text = Text::new("hello", 1, 0, None);
        let lines = text.render(20);
        assert!(lines.len() > 0);
        assert!(lines[0].contains("hello"));
    }

    #[test]
    fn test_width_respected() {
        let text = Text::new("hello world this is a long line", 1, 0, None);
        let lines = text.render(10);
        for line in &lines {
            assert!(visible_width(line) <= 10);
        }
    }

    #[test]
    fn test_padding() {
        let text = Text::new("hi", 2, 1, None);
        let lines = text.render(10);
        // 1 top padding + 1 content + 1 bottom padding = 3 lines
        assert_eq!(lines.len(), 3);
    }
}
