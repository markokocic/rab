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
    /// Pi: returns `[""]` (empty strings, not padded spaces)
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec![String::new(); self.lines]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spacer() {
        let mut spacer = Spacer::new(3);
        let lines = spacer.render(10);
        assert_eq!(lines.len(), 3);
        // Pi: spacer returns empty strings
        for line in &lines {
            assert_eq!(line, "");
        }
    }
}
