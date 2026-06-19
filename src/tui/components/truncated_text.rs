use crate::tui::Component;
use crate::tui::util::truncate_to_width;

/// Text truncated to fit within a maximum visible width with configurable ellipsis.
pub struct TruncatedText {
    text: String,
    ellipsis: String,
    cached_width: Option<usize>,
    cached_line: String,
}

impl TruncatedText {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ellipsis: "...".to_string(),
            cached_width: None,
            cached_line: String::new(),
        }
    }

    pub fn with_ellipsis(mut self, ellipsis: impl Into<String>) -> Self {
        self.ellipsis = ellipsis.into();
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
    fn render(&self, width: usize) -> Vec<String> {
        if self.cached_width == Some(width) {
            return vec![self.cached_line.clone()];
        }

        let result = truncate_to_width(&self.text, width, &self.ellipsis, false);
        vec![result]
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
        let tt = TruncatedText::new("hello");
        let lines = tt.render(10);
        assert_eq!(lines[0], "hello");
    }

    #[test]
    fn test_truncated() {
        let tt = TruncatedText::new("hello world");
        let lines = tt.render(8);
        assert!(visible_width(&lines[0]) <= 8);
        assert!(lines[0].contains("..."));
    }
}
