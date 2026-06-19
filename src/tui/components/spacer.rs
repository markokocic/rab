use crate::tui::Component;

/// Empty vertical space.
pub struct Spacer {
    lines: usize,
}

impl Spacer {
    pub fn new(lines: usize) -> Self {
        Self { lines }
    }

    pub fn set_lines(&mut self, lines: usize) {
        self.lines = lines;
    }
}

impl Component for Spacer {
    fn render(&self, width: usize) -> Vec<String> {
        vec![" ".repeat(width); self.lines]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::util::visible_width;

    #[test]
    fn test_spacer() {
        let spacer = Spacer::new(3);
        let lines = spacer.render(10);
        assert_eq!(lines.len(), 3);
        for line in &lines {
            assert_eq!(visible_width(line), 10);
        }
    }
}
