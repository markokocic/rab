/// Theme trait for components that need color styling.
///
/// Implementations provide foreground and background color functions
/// that take text and return ANSI-styled strings.
pub trait Theme {
    /// Apply a foreground color to text.
    /// `color` is a color name (e.g., "accent", "text", "success", "error", "muted").
    fn fg(&self, color: &str, text: &str) -> String;

    /// Apply a background color to text.
    fn bg(&self, color: &str, text: &str) -> String;

    /// Apply bold styling.
    fn bold(&self, text: &str) -> String;

    /// Apply italic styling (used for thinking blocks, matching pi).
    fn italic(&self, text: &str) -> String;
}

/// A no-op theme that returns text unchanged.
/// Useful for testing components without needing a real theme.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopTheme;

impl Theme for NoopTheme {
    fn fg(&self, _color: &str, text: &str) -> String {
        text.to_string()
    }

    fn bg(&self, _color: &str, text: &str) -> String {
        text.to_string()
    }

    fn bold(&self, text: &str) -> String {
        text.to_string()
    }

    fn italic(&self, text: &str) -> String {
        text.to_string()
    }
}
