#![allow(clippy::type_complexity)]

use crate::tui::Component;
use crate::tui::util::visible_width;

/// Type alias for background color functions.
pub type BgFn = Box<dyn Fn(&str) -> String>;

/// A container with padding and background color function.
/// Children are rendered inside the padded area.
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

    fn apply_bg_force(&self, line: &str, width: usize) -> String {
        match &self.bg_fn {
            Some(bg_fn) => {
                let padded = format!(
                    "{}{}",
                    line,
                    " ".repeat(width.saturating_sub(visible_width(line)))
                );
                bg_fn(&padded)
            }
            None => format!(
                "{}{}",
                line,
                " ".repeat(width.saturating_sub(visible_width(line)))
            ),
        }
    }
}

impl Component for TuiBox {
    fn render(&self, width: usize) -> Vec<String> {
        let inner_width = width.saturating_sub(2 * self.padding_x);
        if inner_width == 0 {
            return vec![self.apply_bg_force(&" ".repeat(width), width)];
        }

        let padding_str = " ".repeat(self.padding_x);
        let mut lines = Vec::new();

        // Render children at inner width
        let child_lines: Vec<String> = self
            .children
            .iter()
            .flat_map(|c| c.render(inner_width))
            .collect();

        // Top padding
        for _ in 0..self.padding_y {
            let line = format!("{}{}", padding_str, " ".repeat(inner_width));
            lines.push(self.apply_bg_force(&line, width));
        }

        // Children with horizontal padding
        for line in &child_lines {
            let padded = format!("{}{}", padding_str, line);
            lines.push(self.apply_bg_force(&padded, width));
        }

        // Bottom padding
        for _ in 0..self.padding_y {
            let line = format!("{}{}", padding_str, " ".repeat(inner_width));
            lines.push(self.apply_bg_force(&line, width));
        }

        lines
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
