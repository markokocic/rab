use std::cell::RefCell;
use std::rc::Rc;

use crate::agent::ui::footer::Footer;
use crate::tui::Component;

/// Wrapper Component that delegates render to a shared Footer.
/// App keeps one Rc for mutation; TUI.root keeps one for rendering.
pub struct FooterComponent(pub Rc<RefCell<Footer>>);

impl Component for FooterComponent {
    fn render(&self, width: usize) -> Vec<String> {
        self.0.borrow().render(width)
    }

    fn invalidate(&mut self) {
        self.0.borrow_mut().invalidate();
    }
}
