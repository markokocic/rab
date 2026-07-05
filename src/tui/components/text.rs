use crate::tui::Component;
use crate::tui::Style;
use crate::tui::util::{visible_width, wrap_text_with_ansi};

/// A styled segment of text with an optional Style (foreground, bold, etc.).
#[derive(Debug, Clone)]
pub struct StyledSegment {
    pub text: String,
    pub style: Option<Style>,
}

impl StyledSegment {
    /// Build the ANSI-escaped representation of this segment.
    pub fn render(&self) -> String {
        match &self.style {
            Some(style) => style.apply(&self.text),
            None => self.text.clone(),
        }
    }
}

/// Multi-line text component with word wrapping and padding.
/// Supports multiple styled segments per line via `StyledSegment`.
///
/// Port of pi's `packages/tui/src/components/text.ts`.
pub struct Text {
    segments: Vec<StyledSegment>,
    padding_x: usize,
    padding_y: usize,
    /// Default style applied to the full padded line (typically background).
    /// Segments without an explicit style inherit foreground/bold/etc from this
    /// when it is applied to the final padded line.
    style: Option<Style>,
    // Render cache
    cached_content: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Vec<String>,
}

impl Text {
    /// Create a new Text component with a single, unstyled segment.
    /// `style` is applied to the entire padded line (e.g. background color).
    pub fn new(
        content: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        style: Option<Style>,
    ) -> Self {
        Self {
            segments: vec![StyledSegment {
                text: content.into(),
                style: None,
            }],
            padding_x,
            padding_y,
            style,
            cached_content: None,
            cached_width: None,
            cached_lines: Vec::new(),
        }
    }

    /// Create a Text component from multiple styled segments.
    /// `default_style` is applied to the full padded line (e.g. background color).
    pub fn from_segments(
        segments: Vec<StyledSegment>,
        padding_x: usize,
        padding_y: usize,
        default_style: Option<Style>,
    ) -> Self {
        Self {
            segments,
            padding_x,
            padding_y,
            style: default_style,
            cached_content: None,
            cached_width: None,
            cached_lines: Vec::new(),
        }
    }

    /// Append a styled segment to the text.
    pub fn push_segment(&mut self, segment: StyledSegment) {
        self.segments.push(segment);
        self.invalidate();
    }

    /// Replace content with a single, unstyled segment.
    pub fn set_text(&mut self, content: impl Into<String>) {
        self.segments = vec![StyledSegment {
            text: content.into(),
            style: None,
        }];
        self.invalidate();
    }

    /// Replace content with styled segments.
    pub fn set_segments(&mut self, segments: Vec<StyledSegment>) {
        self.segments = segments;
        self.invalidate();
    }

    /// Set the default style applied to the final padded line.
    pub fn set_style(&mut self, style: Option<Style>) {
        self.style = style;
        self.invalidate();
    }

    /// Build the full ANSI-escaped content string from all segments.
    fn build_content(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.render())
            .collect::<Vec<_>>()
            .join("")
    }
}

impl Component for Text {
    fn render(&mut self, width: usize) -> Vec<String> {
        // Build content key for cache
        let content_key = self.build_content();

        // Check cache
        if self.cached_content.as_deref() == Some(&content_key) && self.cached_width == Some(width)
        {
            return self.cached_lines.clone();
        }

        // Pi: return [] when content is empty or whitespace-only
        if content_key.is_empty() || content_key.trim().is_empty() {
            return Vec::new();
        }

        // Pi: replace tabs with 3 spaces
        let normalized = content_key.replace('\t', "   ");

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
            let line = match self.style {
                Some(ref style) => style.apply(&padded),
                None => padded,
            };
            content_lines.push(line);
        }

        let empty_line = " ".repeat(width);
        let empty_with_style = self
            .style
            .as_ref()
            .map(|style| style.apply(&empty_line))
            .unwrap_or_else(|| empty_line.clone());

        let mut result = Vec::new();
        for _ in 0..self.padding_y {
            result.push(empty_with_style.clone());
        }
        result.extend(content_lines);
        for _ in 0..self.padding_y {
            result.push(empty_with_style.clone());
        }

        // Update cache
        self.cached_content = Some(content_key);
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

    #[test]
    fn test_styled_segment_render() {
        let style = Style::new().fg("\x1b[38;5;196m".to_string());
        let segment = StyledSegment {
            text: "red".to_string(),
            style: Some(style),
        };
        let result = segment.render();
        assert!(result.starts_with("\x1b[38"));
        assert!(result.contains("red"));
        assert!(result.ends_with("\x1b[39m"));
    }

    #[test]
    fn test_unstyled_segment_render() {
        let segment = StyledSegment {
            text: "plain".to_string(),
            style: None,
        };
        assert_eq!(segment.render(), "plain");
    }

    #[test]
    fn test_from_segments() {
        let bold = Style::new().bold();
        let segments = vec![
            StyledSegment {
                text: "hello ".to_string(),
                style: None,
            },
            StyledSegment {
                text: "world".to_string(),
                style: Some(bold),
            },
        ];
        let mut text = Text::from_segments(segments, 0, 0, None);
        let lines = text.render(20);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("hello"));
        assert!(lines[0].contains("\x1b[1mworld\x1b[22m"));
    }
}
