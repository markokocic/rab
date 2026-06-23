use crate::tui::Component;

/// A component that wraps a `Vec<String>` buffer that can be updated dynamically.
/// Used for sections whose content changes each frame (pending text, status, etc.).
pub struct DynamicLines {
    lines: Vec<String>,
}

impl DynamicLines {
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    /// Set the lines for this component.
    pub fn set_lines(&mut self, new_lines: Vec<String>) {
        self.lines = new_lines;
    }

    /// Clear the lines.
    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

impl Default for DynamicLines {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for DynamicLines {
    fn render(&mut self, _width: usize) -> Vec<String> {
        self.lines.clone()
    }

    fn invalidate(&mut self) {
        // No cache — always returns current buffer
    }
}
