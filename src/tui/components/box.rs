use crate::tui::Component;
use crate::tui::Style;
use crate::tui::util::visible_width;

/// A container with padding and background color function.
/// Children are rendered inside the padded area.
/// Port of pi's `packages/tui/src/components/box.ts`.
pub struct TuiBox {
    children: Vec<Box<dyn Component>>,
    padding_x: usize,
    padding_y: usize,
    bg_style: Option<Style>,
    // Render cache
    cached_child_lines: Vec<String>,
    cached_width: usize,
    cached_bg_sample: Option<String>,
    cached_lines: Vec<String>,
}

impl TuiBox {
    pub fn new(padding_x: usize, padding_y: usize, bg_style: Option<Style>) -> Self {
        Self {
            children: Vec::new(),
            padding_x,
            padding_y,
            bg_style,
            cached_child_lines: Vec::new(),
            cached_width: 0,
            cached_bg_sample: None,
            cached_lines: Vec::new(),
        }
    }

    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.children.push(component);
        self.invalidate_cache();
    }

    pub fn set_bg_style(&mut self, bg_style: Option<Style>) {
        self.bg_style = bg_style;
        self.invalidate_cache();
    }

    fn invalidate_cache(&mut self) {
        self.cached_child_lines.clear();
        self.cached_lines.clear();
    }

    fn apply_bg(&self, line: &str, width: usize) -> String {
        if let Some(ref style) = self.bg_style {
            let vis = visible_width(line);
            let padded = if vis < width {
                format!("{}{}", line, " ".repeat(width - vis))
            } else {
                line.to_string()
            };
            style.apply(&padded)
        } else {
            let vis = visible_width(line);
            if vis < width {
                format!("{}{}", line, " ".repeat(width - vis))
            } else {
                line.to_string()
            }
        }
    }
}

impl Component for TuiBox {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.children.is_empty() {
            return vec![];
        }

        let content_width = width.saturating_sub(2 * self.padding_x).max(1);
        let left_pad = " ".repeat(self.padding_x);

        // Render all children at content width
        let mut child_lines: Vec<String> = Vec::new();
        for child in self.children.iter_mut() {
            for line in child.render(content_width) {
                child_lines.push(format!("{}{}", left_pad, line));
            }
        }

        if child_lines.is_empty() {
            return vec![];
        }

        // Check cache: compare child lines, width, and bg sample
        let bg_sample = self.bg_style.as_ref().map(|s| s.apply("test"));
        if self.cached_child_lines == child_lines
            && self.cached_width == width
            && self.cached_bg_sample == bg_sample
            && !self.cached_lines.is_empty()
        {
            return self.cached_lines.clone();
        }

        let mut result: Vec<String> = Vec::new();
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }
        for line in &child_lines {
            result.push(self.apply_bg(line, width));
        }
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }

        // Update cache
        self.cached_child_lines = child_lines;
        self.cached_width = width;
        self.cached_bg_sample = bg_sample;
        self.cached_lines = result.clone();

        result
    }

    fn invalidate(&mut self) {
        self.invalidate_cache();
        for child in &mut self.children {
            child.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::Text;

    #[test]
    fn test_box_render() {
        let mut b = TuiBox::new(1, 1, None);
        b.add_child(Box::new(Text::new("hello", 0, 0, None)));
        let lines = b.render(20);
        assert!(lines.len() >= 3);
    }
}
