#![allow(clippy::type_complexity)]

use crate::tui::Component;
use crate::tui::util::visible_width;

/// Type alias for background color functions.
pub type BgFn = Box<dyn Fn(&str) -> String>;

/// A container with padding and background color function.
/// Children are rendered inside the padded area.
/// Port of pi's `packages/tui/src/components/box.ts`.
pub struct TuiBox {
    children: Vec<Box<dyn Component>>,
    padding_x: usize,
    padding_y: usize,
    bg_fn: Option<BgFn>,
}

impl TuiBox {
    pub fn new(padding_x: usize, padding_y: usize, bg_fn: Option<BgFn>) -> Self {
        Self {
            children: Vec::new(),
            padding_x,
            padding_y,
            bg_fn,
        }
    }

    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.children.push(component);
    }

    pub fn set_bg_fn(&mut self, bg_fn: Option<BgFn>) {
        self.bg_fn = bg_fn;
    }

    fn pad_to_width(&self, line: &str, width: usize) -> String {
        format!(
            "{}{}",
            line,
            " ".repeat(width.saturating_sub(visible_width(line)))
        )
    }

    fn apply_bg(&self, line: &str, width: usize) -> String {
        let padded = self.pad_to_width(line, width);
        match &self.bg_fn {
            Some(bg_fn) => bg_fn(&padded),
            None => padded,
        }
    }
}

impl Component for TuiBox {
    fn render(&self, width: usize) -> Vec<String> {
        // Pi: return [] when no children
        if self.children.is_empty() {
            return vec![];
        }

        let content_width = width.saturating_sub(2 * self.padding_x).max(1);
        let left_pad = " ".repeat(self.padding_x);

        // Render all children at content width
        let mut child_lines: Vec<String> = Vec::new();
        for child in &self.children {
            for line in child.render(content_width) {
                child_lines.push(format!("{}{}", left_pad, line));
            }
        }

        // Pi: return [] when no child content produced
        if child_lines.is_empty() {
            return vec![];
        }

        let mut result: Vec<String> = Vec::new();

        // Top padding (pi: paddingY lines of empty content with bg)
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }

        // Content lines with background
        for line in &child_lines {
            result.push(self.apply_bg(line, width));
        }

        // Bottom padding
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }

        result
    }

    fn invalidate(&mut self) {
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
        // 1 top pad + 1 content + 1 bottom pad = 3
        assert!(lines.len() >= 3);
    }
}
