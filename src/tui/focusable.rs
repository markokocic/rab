use crate::tui::Component;

/// Components that display a text cursor and need IME support.
pub trait Focusable: Component {
    fn set_focused(&mut self, focused: bool);
    fn focused(&self) -> bool;
}

/// Zero-width APC sequence marking cursor position for IME.
/// Components emit this at the cursor position when focused.
/// The Screen finds and strips this marker, then positions the hardware cursor there.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";
