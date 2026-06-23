use crate::tui::Component;
use crate::tui::util::{truncate_to_width, visible_width};

/// Text truncated to fit within a maximum visible width with configurable ellipsis.
/// Port of pi's `packages/tui/src/components/truncated-text.ts`.
pub struct TruncatedText {
    text: String,
    ellipsis: String,
    padding_x: usize,
    padding_y: usize,
    cached_width: Option<usize>,
    cached_line: String,
}

impl TruncatedText {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ellipsis: "...".to_string(),
            padding_x: 0,
            padding_y: 0,
            cached_width: None,
            cached_line: String::new(),
        }
    }

    pub fn with_ellipsis(mut self, ellipsis: impl Into<String>) -> Self {
        self.ellipsis = ellipsis.into();
        self
    }

    pub fn with_padding(mut self, padding_x: usize, padding_y: usize) -> Self {
        self.padding_x = padding_x;
        self.padding_y = padding_y;
        self
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cached_width = None;
    }

    pub fn set_ellipsis(&mut self, ellipsis: impl Into<String>) {
        self.ellipsis = ellipsis.into();
        self.cached_width = None;
    }
}

impl Component for TruncatedText {
    fn render(&mut self, width: usize) -> Vec<String> {
        // Use cache for single-line no-padding case
        if self.padding_x == 0 && self.padding_y == 0 && self.cached_width == Some(width) {
            return vec![self.cached_line.clone()];
        }

        let mut result: Vec<String> = Vec::new();

        // Pi: vertical padding above
        let empty_line = " ".repeat(width);
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        // Pi: only first line before newline is used
        let single_line = match self.text.find('\n') {
            Some(pos) => &self.text[..pos],
            None => &self.text,
        };

        // Pi: calculate available width after horizontal padding
        let available = width.saturating_sub(2 * self.padding_x).max(1);

        // Pi: truncate with ellipsis
        let display = truncate_to_width(single_line, available, &self.ellipsis, false);

        // Pi: add horizontal padding
        let left = " ".repeat(self.padding_x);
        let padded = format!("{}{}", left, display);
        let vw = visible_width(&padded);

        // Pi: pad to full width
        let line = if vw < width {
            format!("{}{}", padded, " ".repeat(width - vw))
        } else {
            padded
        };
        result.push(line);

        // Pi: vertical padding below
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        // Cache single-line no-padding case
        if self.padding_x == 0 && self.padding_y == 0 {
            self.cached_width = Some(width);
            self.cached_line = if result.is_empty() {
                String::new()
            } else {
                result[0].clone()
            };
        }

        result
    }

    fn invalidate(&mut self) {
        self.cached_width = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::util::visible_width;

    #[test]
    fn test_no_truncation() {
        let mut tt = TruncatedText::new("hello");
        let lines = tt.render(10);
        // Pi: padded to full width
        assert!(lines[0].starts_with("hello"));
        assert_eq!(crate::tui::util::visible_width(&lines[0]), 10);
    }

    #[test]
    fn test_truncated() {
        let mut tt = TruncatedText::new("hello world");
        let lines = tt.render(8);
        assert!(visible_width(&lines[0]) <= 8);
        assert!(lines[0].contains("..."));
    }

    #[test]
    fn test_padding() {
        let mut tt = TruncatedText::new("hello").with_padding(1, 1);
        let lines = tt.render(10);
        assert_eq!(lines.len(), 3, "Should have top pad + line + bottom pad");
        assert!(
            lines[0].chars().all(|c| c == ' '),
            "Top padding should be spaces"
        );
        assert!(lines[1].contains("hello"), "Content should contain text");
        assert!(
            lines[2].chars().all(|c| c == ' '),
            "Bottom padding should be spaces"
        );
    }

    #[test]
    fn test_only_first_line() {
        let mut tt = TruncatedText::new("line1\nline2");
        let lines = tt.render(20);
        assert_eq!(lines.len(), 1);
        assert!(
            !lines[0].contains("line2"),
            "Should not contain second line"
        );
    }
}
