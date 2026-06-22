use std::cell::RefCell;
use std::rc::Rc;

use crate::tui::Component;

/// A component that wraps a `Vec<String>` buffer that can be updated dynamically.
/// Used for sections whose content changes each frame (pending text, status, etc.).
pub struct DynamicLines {
    lines: RefCell<Vec<String>>,
}

/// A Component wrapper around `Rc<DynamicLines>` for shared ownership.
pub struct RcDynamicLines(pub Rc<DynamicLines>);

impl Component for RcDynamicLines {
    fn render(&self, width: usize) -> Vec<String> {
        self.0.render(width)
    }

    fn invalidate(&mut self) {
        // DynamicLines uses RefCell internally, so we can invalidate through Rc
        self.0.clear_cache();
    }
}

impl Clone for RcDynamicLines {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl DynamicLines {
    pub fn new() -> Self {
        Self {
            lines: RefCell::new(Vec::new()),
        }
    }

    /// Set the lines for this component.
    pub fn set_lines(&self, new_lines: Vec<String>) {
        *self.lines.borrow_mut() = new_lines;
    }

    /// Clear the lines.
    pub fn clear(&self) {
        self.lines.borrow_mut().clear();
    }

    /// Clear cached state (for Component::invalidate).
    pub fn clear_cache(&self) {
        // No cache to clear — always reads from current buffer.
    }
}

impl Default for DynamicLines {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for DynamicLines {
    fn render(&self, _width: usize) -> Vec<String> {
        self.lines.borrow().clone()
    }

    fn invalidate(&mut self) {
        // No cache — always returns current buffer
    }
}
