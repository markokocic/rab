use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Key identifiers for matching keyboard input.
/// Use `Key::Enter`, `Key::Ctrl('c')`, `Key::CtrlShift('p')` etc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    Space,
    /// Single character (printable ASCII)
    Char(char),
    /// Ctrl + character
    Ctrl(char),
    /// Alt + character
    Alt(char),
    /// Shift + Tab
    ShiftTab,
    /// Ctrl + Shift + character
    CtrlShift(char),
    /// Alt + arrow
    AltLeft,
    AltRight,
    /// Ctrl + arrow
    CtrlLeft,
    CtrlRight,
}

impl Key {
    pub fn enter() -> Self {
        Key::Enter
    }
    pub fn escape() -> Self {
        Key::Escape
    }
    pub fn tab() -> Self {
        Key::Tab
    }
    pub fn space() -> Self {
        Key::Space
    }
    pub fn backspace() -> Self {
        Key::Backspace
    }
    pub fn delete() -> Self {
        Key::Delete
    }
    pub fn home() -> Self {
        Key::Home
    }
    pub fn end() -> Self {
        Key::End
    }
    pub fn up() -> Self {
        Key::Up
    }
    pub fn down() -> Self {
        Key::Down
    }
    pub fn left() -> Self {
        Key::Left
    }
    pub fn right() -> Self {
        Key::Right
    }
    pub fn page_up() -> Self {
        Key::PageUp
    }
    pub fn page_down() -> Self {
        Key::PageDown
    }
    pub fn ctrl(c: char) -> Self {
        Key::Ctrl(c.to_ascii_lowercase())
    }
    pub fn alt(c: char) -> Self {
        Key::Alt(c)
    }
    pub fn shift_tab() -> Self {
        Key::ShiftTab
    }
    pub fn ctrl_shift(c: char) -> Self {
        Key::CtrlShift(c.to_ascii_lowercase())
    }
    pub fn alt_left() -> Self {
        Key::AltLeft
    }
    pub fn alt_right() -> Self {
        Key::AltRight
    }
    pub fn ctrl_left() -> Self {
        Key::CtrlLeft
    }
    pub fn ctrl_right() -> Self {
        Key::CtrlRight
    }
}

/// Check if a crossterm KeyEvent matches a Key identifier.
pub fn matches_key(event: &KeyEvent, key: &Key) -> bool {
    match key {
        Key::Enter => event.code == KeyCode::Enter,
        Key::Escape => event.code == KeyCode::Esc,
        Key::Tab => event.code == KeyCode::Tab,
        Key::Backspace => event.code == KeyCode::Backspace,
        Key::Delete => event.code == KeyCode::Delete,
        Key::Home => event.code == KeyCode::Home,
        Key::End => event.code == KeyCode::End,
        Key::PageUp => event.code == KeyCode::PageUp,
        Key::PageDown => event.code == KeyCode::PageDown,
        Key::Up => event.code == KeyCode::Up,
        Key::Down => event.code == KeyCode::Down,
        Key::Left => event.code == KeyCode::Left,
        Key::Right => event.code == KeyCode::Right,
        Key::Space => event.code == KeyCode::Char(' '),
        Key::Char(c) => {
            event.code == KeyCode::Char(*c)
                && !event.modifiers.contains(KeyModifiers::CONTROL)
                && !event.modifiers.contains(KeyModifiers::ALT)
        }
        Key::Ctrl(c) => {
            event.code == KeyCode::Char(c.to_ascii_lowercase())
                && event.modifiers.contains(KeyModifiers::CONTROL)
                && !event.modifiers.contains(KeyModifiers::ALT)
        }
        Key::Alt(c) => {
            event.code == KeyCode::Char(*c)
                && event.modifiers.contains(KeyModifiers::ALT)
                && !event.modifiers.contains(KeyModifiers::CONTROL)
        }
        Key::ShiftTab => {
            event.code == KeyCode::BackTab
                || (event.code == KeyCode::Tab && event.modifiers.contains(KeyModifiers::SHIFT))
        }
        Key::CtrlShift(c) => {
            event.code == KeyCode::Char(c.to_ascii_lowercase())
                && event.modifiers.contains(KeyModifiers::CONTROL)
                && event.modifiers.contains(KeyModifiers::SHIFT)
        }
        Key::AltLeft => event.code == KeyCode::Left && event.modifiers.contains(KeyModifiers::ALT),
        Key::AltRight => {
            event.code == KeyCode::Right && event.modifiers.contains(KeyModifiers::ALT)
        }
        Key::CtrlLeft => {
            event.code == KeyCode::Left && event.modifiers.contains(KeyModifiers::CONTROL)
        }
        Key::CtrlRight => {
            event.code == KeyCode::Right && event.modifiers.contains(KeyModifiers::CONTROL)
        }
    }
}

/// Convert a printable KeyEvent to a string representation.
pub fn key_event_to_string(event: &KeyEvent) -> Option<String> {
    match event.code {
        KeyCode::Char(c) => {
            if event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT {
                Some(c.to_string())
            } else {
                None
            }
        }
        KeyCode::Enter => Some("\n".to_string()),
        KeyCode::Tab => Some("\t".to_string()),
        _ => None,
    }
}

/// Check if a key event is a printable character (no modifiers except shift).
pub fn is_printable(event: &KeyEvent) -> bool {
    matches!(event.code, KeyCode::Char(_))
        && !event.modifiers.contains(KeyModifiers::CONTROL)
        && !event.modifiers.contains(KeyModifiers::ALT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_enter() {
        let event = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches_key(&event, &Key::Enter));
    }

    #[test]
    fn test_matches_escape() {
        let event = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches_key(&event, &Key::Escape));
    }

    #[test]
    fn test_matches_ctrl_c() {
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches_key(&event, &Key::Ctrl('c')));
        assert!(!matches_key(&event, &Key::Char('c')));
    }

    #[test]
    fn test_matches_char() {
        let event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(matches_key(&event, &Key::Char('a')));
        assert!(!matches_key(&event, &Key::Ctrl('a')));
    }

    #[test]
    fn test_matches_arrow_keys() {
        assert!(matches_key(
            &KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &Key::Up
        ));
        assert!(matches_key(
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &Key::Down
        ));
        assert!(matches_key(
            &KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &Key::Left
        ));
        assert!(matches_key(
            &KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &Key::Right
        ));
    }

    #[test]
    fn test_shift_tab() {
        assert!(matches_key(
            &KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
            &Key::ShiftTab
        ));
    }

    #[test]
    fn test_ctrl_shift() {
        let event = KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert!(matches_key(&event, &Key::CtrlShift('p')));
    }

    #[test]
    fn test_is_printable() {
        assert!(is_printable(&KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE
        )));
        assert!(!is_printable(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_printable(&KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE
        )));
    }
}
