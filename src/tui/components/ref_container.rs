use std::cell::RefCell;
use std::rc::Rc;

use crate::tui::Component;
use crate::tui::Container;

/// A Component wrapper around `Rc<RefCell<Container>>` that allows
/// dynamically adding/removing children while sharing ownership with App.
///
/// Matches pi's pattern of keeping references to sub-containers for mutation:
/// ```typescript
/// this.chatContainer = new Container();
/// this.ui.addChild(this.chatContainer);
/// // Later:
/// this.chatContainer.addChild(component);
/// ```
#[derive(Clone)]
pub struct RefContainer {
    pub inner: Rc<RefCell<Container>>,
}

impl RefContainer {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Container::new())),
        }
    }

    pub fn new_rc() -> Rc<RefCell<Container>> {
        Rc::new(RefCell::new(Container::new()))
    }
}

impl Default for RefContainer {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for RefContainer {
    fn render(&self, width: usize) -> Vec<String> {
        self.inner.borrow().render(width)
    }

    fn invalidate(&mut self) {
        self.inner.borrow_mut().invalidate();
    }
}
