use std::cell::RefCell;
use std::rc::Rc;

use crossterm::event::KeyEvent;

use crate::tui::Component;

/// A Component wrapper around `Rc<RefCell<dyn Component>>` for shared ownership.
/// Allows App to hold a `Weak<RefCell<dyn Component>>` for in-place updates.
pub struct RcRefCellComponent(pub Rc<RefCell<dyn Component>>);

impl Clone for RcRefCellComponent {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Component for RcRefCellComponent {
    fn render(&self, width: usize) -> Vec<String> {
        self.0.borrow().render(width)
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        self.0.borrow_mut().handle_input(key)
    }

    fn set_expanded(&mut self, expanded: bool) {
        self.0.borrow_mut().set_expanded(expanded);
    }

    fn set_hide_thinking(&mut self, hide: bool) {
        self.0.borrow_mut().set_hide_thinking(hide);
    }

    fn invalidate(&mut self) {
        self.0.borrow_mut().invalidate();
    }
}
