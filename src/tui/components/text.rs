#![allow(clippy::type_complexity)]

use std::cell::RefCell;

use crate::tui::Component;
use crate::tui::util::{visible_width, wrap_text_with_ansi};

/// Multi-line text component with word wrapping and padding.
/// Port of pi's `packages/tui/src/components/text.ts`.
pub struct Text {
    content: String,
    padding_x: usize,
    padding_y: usize,
    bg_fn: Option<Box<dyn Fn(&str) -> String>>,
    // Render cache (RefCell for interior mutability since render takes &self)
    cached_content: RefCell<Option<String>>,
    cached_width: RefCell<Option<usize>>,
    cached_lines: RefCell<Vec<String>>,
}

impl Text {
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
            cached_content: RefCell::new(None),
            cached_width: RefCell::new(None),
            cached_lines: RefCell::new(Vec::new()),
        }
    }

    pub fn set_text(&mut self, content: impl Into<String>) {
        self.content = content.into();
        self.invalidate();
    }

    pub fn set_bg_fn(&mut self, bg_fn: Option<Box<dyn Fn(&str) -> String>>) {
        self.bg_fn = bg_fn;
        self.invalidate();
    }
}

impl Component for Text {
    fn render(&self, width: usize) -> Vec<String> {
        // Check cache
        if self.cached_content.borrow().as_deref() == Some(&self.content)
            && *self.cached_width.borrow() == Some(width)
        {
            return self.cached_lines.borrow().clone();
        }

        // Pi: return [] when content is empty or whitespace-only
        if self.content.is_empty() || self.content.trim().is_empty() {
            let lines: Vec<String> = Vec::new();
            // Skip cache for empty — need to detect when content changes
            return lines;
        }

        // Pi: replace tabs with 3 spaces
        let normalized = self.content.replace('\t', "   ");

        // Pi: max(1, width - paddingX * 2)
        let content_width = width.saturating_sub(2 * self.padding_x).max(1);
        let left_margin = " ".repeat(self.padding_x);

        // Pi: wrap text (preserves ANSI, does NOT pad)
        let wrapped = wrap_text_with_ansi(&normalized, content_width);

        let mut content_lines: Vec<String> = Vec::new();
        for line in wrapped {
            let line_with_margins = format!("{}{}{}", left_margin, line, left_margin);
            let vw = visible_width(&line_with_margins);
            if let Some(ref bg_fn) = self.bg_fn {
                let padded = if vw < width {
                    format!("{}{}", line_with_margins, " ".repeat(width - vw))
                } else {
                    line_with_margins
                };
                content_lines.push(bg_fn(&padded));
            } else {
                let padded = if vw < width {
                    format!("{}{}", line_with_margins, " ".repeat(width - vw))
                } else {
                    line_with_margins
                };
                content_lines.push(padded);
            }
        }

        let empty_line = " ".repeat(width);
        let empty_with_bg = self
            .bg_fn
            .as_ref()
            .map(|bg| bg(&empty_line))
            .unwrap_or_else(|| empty_line.clone());

        let mut result = Vec::new();
        for _ in 0..self.padding_y {
            result.push(empty_with_bg.clone());
        }
        result.extend(content_lines);
        for _ in 0..self.padding_y {
            result.push(empty_with_bg.clone());
        }

        // Update cache
        *self.cached_content.borrow_mut() = Some(self.content.clone());
        *self.cached_width.borrow_mut() = Some(width);
        *self.cached_lines.borrow_mut() = result.clone();

        result
    }

    fn invalidate(&mut self) {
        *self.cached_content.borrow_mut() = None;
        *self.cached_width.borrow_mut() = None;
        self.cached_lines.borrow_mut().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_render() {
        let text = Text::new("hello", 1, 0, None);
        let lines = text.render(20);
        assert!(!lines.is_empty());
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
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_cache_hit() {
        let text = Text::new("hello", 1, 0, None);
        let a = text.render(20);
        let b = text.render(20);
        assert_eq!(a, b);
    }
}
