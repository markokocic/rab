use crate::tui::Component;

/// Container - a component that contains other components rendered vertically.
pub struct Container {
    children: Vec<Box<dyn Component>>,
}

impl Container {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }

    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.children.push(component);
    }

    pub fn remove_child(&mut self, component: &dyn Component) {
        // Use pointer-based identity check - simplistic but works for our use case
        self.children.retain(|c| {
            !std::ptr::eq(
                c.as_ref() as *const dyn Component,
                component as *const dyn Component,
            )
        });
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }

    pub fn children(&self) -> &[Box<dyn Component>] {
        &self.children
    }

    pub fn children_mut(&mut self) -> &mut [Box<dyn Component>] {
        &mut self.children
    }
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Container {
    fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &self.children {
            let child_lines = child.render(width);
            lines.extend(child_lines);
        }
        lines
    }

    fn handle_input(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        for child in self.children.iter_mut().rev() {
            if child.handle_input(key) {
                return true;
            }
        }
        false
    }

    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }
}
