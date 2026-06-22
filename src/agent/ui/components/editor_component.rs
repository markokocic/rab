use std::cell::RefCell;
use std::rc::Rc;

use crate::agent::ui::chat_editor::ChatEditor;
use crate::tui::Component;

/// Wrapper Component that delegates render to a shared ChatEditor.
/// App keeps one Rc for mutation; TUI.root keeps one for rendering.
pub struct EditorComponent(pub Rc<RefCell<ChatEditor>>);

impl Component for EditorComponent {
    fn render(&self, width: usize) -> Vec<String> {
        self.0.borrow().editor.render(width)
    }

    fn invalidate(&mut self) {
        self.0.borrow_mut().editor.invalidate();
    }
}
