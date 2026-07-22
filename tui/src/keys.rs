use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

// =============================================================================
// Key ID string helpers — pi-compatible string-based key identifiers
// =============================================================================

/// Format an F-key number into a name like "f1", "f12" etc.
fn f_key_name(n: u8) -> String {
    format!("f{}", n)
}

/// Parse an F-key name like "f1", "f12" into the number.
fn parse_f_key(key_name: &str) -> Option<u8> {
    if let Some(rest) = key_name.strip_prefix('f') {
        rest.parse().ok().filter(|&n: &u8| (1..=24).contains(&n))
    } else if let Some(rest) = key_name.strip_prefix('F') {
        rest.parse().ok().filter(|&n: &u8| (1..=24).contains(&n))
    } else {
        None
    }
}

/// Format a key name with modifiers into a canonical key ID string.
/// Order: ctrl > shift > alt > super (matching pi convention).
fn format_key_id(key_name: &str, mods: KeyModifiers) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if mods.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl");
    }
    if mods.contains(KeyModifiers::SHIFT) {
        parts.push("shift");
    }
    if mods.contains(KeyModifiers::ALT) {
        parts.push("alt");
    }
    if mods.contains(KeyModifiers::SUPER) {
        parts.push("super");
    }
    if parts.is_empty() {
        key_name.to_string()
    } else {
        parts.push(key_name);
        parts.join("+")
    }
}

/// Convert a crossterm KeyEvent to a pi-compatible key ID string.
/// Returns None for non-input keys (CapsLock, NumLock, etc.).
///
/// Examples: "enter", "escape", "ctrl+c", "shift+tab", "alt+left", "ctrl+shift+p"
pub fn key_event_to_id(event: &KeyEvent) -> Option<String> {
    let mods = event.modifiers;

    match event.code {
        KeyCode::Enter => Some(format_key_id("enter", mods)),
        KeyCode::Esc => Some("escape".to_string()),
        KeyCode::Tab => {
            if mods.contains(KeyModifiers::SHIFT) {
                Some("shift+tab".to_string())
            } else {
                Some(format_key_id("tab", mods))
            }
        }
        KeyCode::Backspace => Some(format_key_id("backspace", mods)),
        KeyCode::Delete => Some(format_key_id("delete", mods)),
        KeyCode::Home => Some(format_key_id("home", mods)),
        KeyCode::End => Some(format_key_id("end", mods)),
        KeyCode::PageUp => Some(format_key_id("pageUp", mods)),
        KeyCode::PageDown => Some(format_key_id("pageDown", mods)),
        KeyCode::Up => Some(format_key_id("up", mods)),
        KeyCode::Down => Some(format_key_id("down", mods)),
        KeyCode::Left => Some(format_key_id("left", mods)),
        KeyCode::Right => Some(format_key_id("right", mods)),
        KeyCode::BackTab => Some("shift+tab".to_string()),
        KeyCode::Insert => Some(format_key_id("insert", mods)),
        KeyCode::F(n) => Some(format_key_id(&f_key_name(n), mods)),
        KeyCode::Char(c) => {
            if mods.is_empty() || mods == KeyModifiers::SHIFT {
                // Plain character (possibly shifted for uppercase)
                Some(c.to_string())
            } else if mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) {
                // Ctrl+key, possibly with shift or super
                let mut parts: Vec<String> = Vec::new();
                parts.push("ctrl".into());
                if mods.contains(KeyModifiers::SHIFT) {
                    parts.push("shift".into());
                }
                if mods.contains(KeyModifiers::SUPER) {
                    parts.push("super".into());
                }
                let lower = c.to_ascii_lowercase();
                Some(format!("{}+{}", parts.join("+"), lower))
            } else if mods.contains(KeyModifiers::ALT) {
                let mut parts: Vec<String> = Vec::new();
                if mods.contains(KeyModifiers::CONTROL) {
                    parts.push("ctrl".into());
                }
                parts.push("alt".into());
                if mods.contains(KeyModifiers::SHIFT) {
                    parts.push("shift".into());
                }
                if mods.contains(KeyModifiers::SUPER) {
                    parts.push("super".into());
                }
                Some(format!("{}+{}", parts.join("+"), c))
            } else {
                Some(c.to_string())
            }
        }

        KeyCode::Null
        | KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => None,
    }
}

/// Parse a key ID string into its components: (key_name, ctrl, shift, alt, super).
/// Returns None if the string is not a valid key ID.
fn parse_key_id(key_id: &str) -> Option<(&str, bool, bool, bool, bool)> {
    if key_id.is_empty() {
        return None;
    }
    let parts: Vec<&str> = key_id.split('+').collect();
    if parts.is_empty() {
        return None;
    }
    let key = parts[parts.len() - 1];
    let mut ctrl = false;
    let mut shift = false;
    let mut alt = false;
    let mut super_mod = false;

    for p in &parts[..parts.len() - 1] {
        match *p {
            "ctrl" => ctrl = true,
            "shift" => shift = true,
            "alt" => alt = true,
            "super" => super_mod = true,
            _ => return None, // Unknown modifier
        }
    }

    Some((key, ctrl, shift, alt, super_mod))
}

/// Match a crossterm KeyEvent against a key ID string.
/// Handles relaxed modifier matching — extra modifiers don't cause non-match
/// if the component doesn't need them (e.g., "enter" matches Shift+Enter too).
pub fn match_key_id(event: &KeyEvent, key_id: &str) -> bool {
    let Some((key, wants_ctrl, wants_shift, wants_alt, wants_super)) = parse_key_id(key_id) else {
        return false;
    };

    let mods = event.modifiers;
    let has_ctrl = mods.contains(KeyModifiers::CONTROL);
    let has_shift = mods.contains(KeyModifiers::SHIFT);
    let has_alt = mods.contains(KeyModifiers::ALT);
    let has_super = mods.contains(KeyModifiers::SUPER);

    // Special case: BackTab is inherently Shift+Tab. If the key ID wants
    // shift and the event is BackTab, treat it as having the shift modifier.
    let is_backtab = event.code == KeyCode::BackTab;
    let wants_tab = key == "tab";

    // Treat BackTab as having an implicit shift modifier (only for the
    // "wants_shift" check — actual has_shift is used for rejecting extra shift).
    let wanted_shift = has_shift || (is_backtab && wants_tab);

    // ── Required-modifier check ──
    // If the key ID requests a modifier, the event must have it.
    if wants_ctrl && !has_ctrl {
        return false;
    }
    if wants_shift && !wanted_shift {
        return false;
    }
    if wants_alt && !has_alt {
        return false;
    }
    if wants_super && !has_super {
        return false;
    }

    // ── Extra-modifier rejection ──
    // If the key ID does NOT request a modifier, extra instances of that
    // modifier on the event cause a non-match.  The only exception is shift
    // when it only changes case (uppercase letter or shifted symbol).
    if !wants_ctrl && has_ctrl {
        return false;
    }
    if !wants_alt && has_alt {
        return false;
    }
    if !wants_super && has_super {
        return false;
    }
    // Shift is special: lowercase key "p" with shift modifier could just be
    // the user pressing Shift+P (uppercase). Allow shift when the expected
    // key is an uppercase letter or shifted symbol.  For BackTab we already
    // handle it via effective_shift — it counts as having shift implicitly.
    if !wants_shift && has_shift && !is_backtab {
        // Allow shift only for letters where key_name is the uppercase version
        // or for symbols that require shift
        let shiftable = key.len() == 1 && {
            let c = key.chars().next().unwrap();
            c.is_ascii_uppercase()
                || c.is_ascii_digit()
                || matches!(
                    c,
                    '!' | '@'
                        | '#'
                        | '$'
                        | '%'
                        | '^'
                        | '&'
                        | '*'
                        | '('
                        | ')'
                        | '_'
                        | '+'
                        | '|'
                        | '~'
                        | '{'
                        | '}'
                        | ':'
                        | '"'
                        | '<'
                        | '>'
                        | '?'
                )
        };
        if !shiftable {
            return false;
        }
    }

    // BackTab (shift+tab) should only match key IDs that explicitly request shift
    if event.code == KeyCode::BackTab && !wants_shift {
        return false;
    }

    // Match the key name against the event code
    matches_key_name(&event.code, key)
}

/// Check if a KeyCode matches a key name string.
fn matches_key_name(code: &KeyCode, key_name: &str) -> bool {
    match code {
        KeyCode::Enter => key_name == "enter" || key_name == "return",
        KeyCode::Esc => key_name == "escape" || key_name == "esc",
        KeyCode::Tab | KeyCode::BackTab => key_name == "tab",
        KeyCode::Backspace => key_name == "backspace",
        KeyCode::Delete => key_name == "delete",
        KeyCode::Home => key_name == "home",
        KeyCode::End => key_name == "end",
        KeyCode::PageUp => key_name == "pageUp" || key_name == "pageup",
        KeyCode::PageDown => key_name == "pageDown" || key_name == "pagedown",
        KeyCode::Up => key_name == "up",
        KeyCode::Down => key_name == "down",
        KeyCode::Left => key_name == "left",
        KeyCode::Right => key_name == "right",
        KeyCode::Insert => key_name == "insert",
        KeyCode::F(n) => Some(*n) == parse_f_key(key_name),
        KeyCode::Char(c) if key_name.len() == 1 => {
            // Single character — compare case-insensitively
            let key_char = key_name.chars().next().unwrap();
            c.eq_ignore_ascii_case(&key_char)
        }
        _ => false,
    }
}

/// Check if a key event is a release event (Kitty keyboard protocol flag 2).
pub fn is_key_release(event: &KeyEvent) -> bool {
    event.kind == KeyEventKind::Release
}

/// Check if a key event is a repeat event (Kitty keyboard protocol flag 2).
pub fn is_key_repeat(event: &KeyEvent) -> bool {
    event.kind == KeyEventKind::Repeat
}

/// Decode a printable character from a key event.
/// Since crossterm already decodes CSI-u sequences, this is equivalent to
/// `key_event_to_string` for printable characters.
pub fn decode_kitty_printable(event: &KeyEvent) -> Option<String> {
    match event.code {
        KeyCode::Char(c)
            if !event.modifiers.contains(KeyModifiers::CONTROL)
                && !event.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(c.to_string())
        }
        _ => None,
    }
}

// =============================================================================
// Legacy Key enum (backward compat) — will be removed after full migration
// =============================================================================

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
    fn test_key_event_to_id_enter() {
        let event = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(key_event_to_id(&event), Some("enter".into()));
    }

    #[test]
    fn test_key_event_to_id_escape() {
        let event = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(key_event_to_id(&event), Some("escape".into()));
    }

    #[test]
    fn test_key_event_to_id_ctrl_c() {
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_id(&event), Some("ctrl+c".into()));
    }

    #[test]
    fn test_key_event_to_id_char() {
        let event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(key_event_to_id(&event), Some("a".into()));
    }

    #[test]
    fn test_key_event_to_id_shift_tab() {
        let event = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        assert_eq!(key_event_to_id(&event), Some("shift+tab".into()));
    }

    #[test]
    fn test_key_event_to_id_ctrl_shift() {
        let event = KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(key_event_to_id(&event), Some("ctrl+shift+p".into()));
    }

    #[test]
    fn test_key_event_to_id_alt_left() {
        let event = KeyEvent::new(KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(key_event_to_id(&event), Some("alt+left".into()));
    }

    #[test]
    fn test_key_event_to_id_ctrl_left() {
        let event = KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(key_event_to_id(&event), Some("ctrl+left".into()));
    }

    #[test]
    fn test_match_key_id_exact() {
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(match_key_id(&event, "ctrl+c"));
        assert!(!match_key_id(&event, "ctrl+x"));
    }

    #[test]
    fn test_match_key_id_no_extra_modifiers() {
        // "enter" should not match ctrl+enter
        let event = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(!match_key_id(&event, "enter"));
    }

    // Legacy tests below ===============================================

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

    #[test]
    fn test_key_event_to_id_up() {
        let event = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(key_event_to_id(&event), Some("up".into()));
    }

    #[test]
    fn test_key_event_to_id_backspace() {
        let event = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(key_event_to_id(&event), Some("backspace".into()));
    }
}
