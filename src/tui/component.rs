use crossterm::event::KeyEvent;

/// Every renderable UI element.
pub trait Component {
    /// Render to lines for the given viewport width.
    /// Each returned string MUST NOT exceed `width` in visible width.
    fn render(&self, width: usize) -> Vec<String>;

    /// Handle keyboard input. Return `true` if consumed.
    fn handle_input(&mut self, _key: &KeyEvent) -> bool {
        false
    }

    /// Clear cached render state. Called on theme changes or resize.
    fn invalidate(&mut self) {}

    /// Whether this component wants focus (for IME cursor positioning).
    fn is_focusable(&self) -> bool {
        false
    }
}
