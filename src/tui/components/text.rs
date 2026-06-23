use crate::tui::Component;
use crate::tui::Style;
use crate::tui::util::{visible_width, wrap_text_with_ansi};

/// Multi-line text component with word wrapping and padding.
/// Port of pi's `packages/tui/src/components/text.ts`.
pub struct Text {
    content: String,
    padding_x: usize,
    padding_y: usize,
    bg_style: Option<Style>,
    // Render cache
    cached_content: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Vec<String>,
}

impl Text {
    pub fn new(
        content: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        bg_style: Option<Style>,
    ) -> Self {
        Self {
            content: content.into(),
            padding_x,
            padding_y,
            bg_style,
            cached_content: None,
            cached_width: None,
            cached_lines: Vec::new(),
        }
    }

    pub fn set_text(&mut self, content: impl Into<String>) {
        self.content = content.into();
        self.invalidate();
    }

    pub fn set_bg_style(&mut self, bg_style: Option<Style>) {
        self.bg_style = bg_style;
        self.invalidate();
    }
}

impl Component for Text {
    fn render(&mut self, width: usize) -> Vec<String> {
        // Check cache
        if self.cached_content.as_deref() == Some(&self.content) && self.cached_width == Some(width)
        {
            return self.cached_lines.clone();
        }

        // Pi: return [] when content is empty or whitespace-only
        if self.content.is_empty() || self.content.trim().is_empty() {
            return Vec::new();
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
            let padded = if vw < width {
                format!("{}{}", line_with_margins, " ".repeat(width - vw))
            } else {
                line_with_margins
            };
            let line = match self.bg_style {
                Some(ref style) => style.apply(&padded),
                None => padded,
            };
            content_lines.push(line);
        }

        let empty_line = " ".repeat(width);
        let empty_with_bg = self
            .bg_style
            .as_ref()
            .map(|style| style.apply(&empty_line))
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
        self.cached_content = Some(self.content.clone());
        self.cached_width = Some(width);
        self.cached_lines = result.clone();

        result
    }

    fn invalidate(&mut self) {
        self.cached_content = None;
        self.cached_width = None;
        self.cached_lines.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_render() {
        let mut text = Text::new("hello", 1, 0, None);
        let lines = text.render(20);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("hello"));
    }

    #[test]
    fn test_width_respected() {
        let mut text = Text::new("hello world this is a long line", 1, 0, None);
        let lines = text.render(10);
        for line in &lines {
            assert!(visible_width(line) <= 10);
        }
    }

    #[test]
    fn test_padding() {
        let mut text = Text::new("hi", 2, 1, None);
        let lines = text.render(10);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_cache_hit() {
        let mut text = Text::new("hello", 1, 0, None);
        let a = text.render(20);
        let b = text.render(20);
        assert_eq!(a, b);
    }
}
