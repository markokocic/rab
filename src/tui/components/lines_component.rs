use std::cell::RefCell;

use crate::tui::Component;

/// A component that wraps a `Vec<String>` buffer.
/// Used as a migration bridge: compose_ui() writes lines here,
/// and this component renders them.
///
/// Once all sections are migrated to proper Component types,
/// this component can be removed.
pub struct LinesComponent {
    /// The rendered lines buffer.
    pub lines: RefCell<Vec<String>>,
}

impl LinesComponent {
    pub fn new() -> Self {
        Self {
            lines: RefCell::new(Vec::new()),
        }
    }

    /// Clear and extend from an iterator.
    pub fn set_lines(&self, new_lines: Vec<String>) {
        *self.lines.borrow_mut() = new_lines;
    }

    /// Push a line.
    pub fn push(&self, line: String) {
        self.lines.borrow_mut().push(line);
    }

    /// Extend with multiple lines.
    pub fn extend(&self, lines: impl IntoIterator<Item = String>) {
        self.lines.borrow_mut().extend(lines);
    }

    /// Clear all lines.
    pub fn clear(&self) {
        self.lines.borrow_mut().clear();
    }
}

impl Default for LinesComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for LinesComponent {
    fn render(&self, _width: usize) -> Vec<String> {
        self.lines.borrow().clone()
    }

    fn invalidate(&mut self) {
        // No cache to invalidate — always returns current buffer
    }
}
