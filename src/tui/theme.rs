/// Composable text style. Builds ANSI escape sequences for foreground,
/// background, bold, italic, underline, and strikethrough.
///
/// Create via builder methods and apply with `apply()`:
/// ```
/// let styled = rab::tui::Style::new().bg("\x1b[48;2;52;53;65m".to_string()).bold().apply("hello");
/// assert!(styled.starts_with("\x1b[48"));
/// assert!(styled.contains("hello"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct Style {
    fg: Option<String>,
    bg: Option<String>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    reverse: bool,
}

impl Style {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set foreground ANSI escape prefix.
    pub fn fg(mut self, ansi: impl Into<String>) -> Self {
        self.fg = Some(ansi.into());
        self
    }

    /// Set background ANSI escape prefix.
    pub fn bg(mut self, ansi: impl Into<String>) -> Self {
        self.bg = Some(ansi.into());
        self
    }

    /// Enable bold.
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Enable dim.
    pub fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    /// Enable italic.
    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// Enable underline.
    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    /// Enable strikethrough.
    pub fn strikethrough(mut self) -> Self {
        self.strikethrough = true;
        self
    }

    /// Enable reverse video.
    pub fn reverse(mut self) -> Self {
        self.reverse = true;
        self
    }

    /// Apply this style to text, returning ANSI-wrapped string.
    /// The text is wrapped with opening escape sequences at the start
    /// and closing sequences at the end.
    ///
    /// Handles embedded reset codes:
    /// - `\x1b[0m` (full reset) — re-inserts the entire prefix
    /// - `\x1b[49m` (bg reset) — re-inserts the background code
    /// - `\x1b[39m` (fg reset) — re-inserts the foreground code
    /// - `\x1b[22m` (bold/dim reset) — re-inserts bold if set
    /// - `\x1b[23m`, `\x1b[24m`, `\x1b[27m`, `\x1b[29m` — similar handling
    pub fn apply(&self, text: &str) -> String {
        let mut prefix = String::new();
        let mut suffix = String::new();

        if let Some(ref fg) = self.fg {
            prefix.push_str(fg);
            suffix.push_str("\x1b[39m");
        }
        if let Some(ref bg) = self.bg {
            prefix.push_str(bg);
            suffix.push_str("\x1b[49m");
        }
        if self.bold {
            prefix.push_str("\x1b[1m");
            suffix.push_str("\x1b[22m");
        }
        if self.italic {
            prefix.push_str("\x1b[3m");
            suffix.push_str("\x1b[23m");
        }
        if self.underline {
            prefix.push_str("\x1b[4m");
            suffix.push_str("\x1b[24m");
        }
        if self.dim {
            prefix.push_str("\x1b[2m");
            suffix.push_str("\x1b[22m");
        }
        if self.strikethrough {
            prefix.push_str("\x1b[9m");
            suffix.push_str("\x1b[29m");
        }
        if self.reverse {
            prefix.push_str("\x1b[7m");
            suffix.push_str("\x1b[27m");
        }

        // Fast path: no reset codes that would clear our styles
        // Check \x1b[0m (full reset) and any attribute-specific reset that
        // we have set in the prefix.
        let has_reset = text.contains("\x1b[0m")
            || (self.bg.is_some() && text.contains("\x1b[49m"))
            || (self.fg.is_some() && text.contains("\x1b[39m"))
            || (self.bold && text.contains("\x1b[22m"))
            || (self.italic && text.contains("\x1b[23m"))
            || (self.underline && text.contains("\x1b[24m"))
            || (self.strikethrough && text.contains("\x1b[29m"))
            || (self.reverse && text.contains("\x1b[27m"));

        if !has_reset && !prefix.is_empty() {
            return format!("{}{}{}", prefix, text, suffix);
        }
        if prefix.is_empty() {
            return text.to_string();
        }

        // Walk through text, re-inserting style codes after each reset
        // that would otherwise clear our attributes.
        let mut result = String::with_capacity(prefix.len() + text.len() + suffix.len() + 64);
        result.push_str(&prefix);

        let mut i = 0;
        let bytes = text.as_bytes();
        while i < bytes.len() {
            if bytes[i] == 0x1b
                && let Some(ansi) = crate::tui::util::extract_ansi_code_at(text, i)
            {
                result.push_str(ansi);
                match ansi {
                    "\x1b[0m" => {
                        // Full reset — re-insert entire prefix
                        result.push_str(&prefix);
                    }
                    "\x1b[49m" if self.bg.is_some() => {
                        // Background reset — re-insert bg code
                        if let Some(ref bg) = self.bg {
                            result.push_str(bg);
                        }
                    }
                    "\x1b[39m" if self.fg.is_some() => {
                        // Foreground reset — re-insert fg code
                        if let Some(ref fg) = self.fg {
                            result.push_str(fg);
                        }
                    }
                    "\x1b[22m" if self.bold || self.dim => {
                        // Bold/dim reset — re-insert bold if bold was set
                        if self.bold {
                            result.push_str("\x1b[1m");
                        }
                    }
                    "\x1b[23m" if self.italic => {
                        result.push_str("\x1b[3m");
                    }
                    "\x1b[24m" if self.underline => {
                        result.push_str("\x1b[4m");
                    }
                    "\x1b[27m" if self.reverse => {
                        result.push_str("\x1b[7m");
                    }
                    "\x1b[29m" if self.strikethrough => {
                        result.push_str("\x1b[9m");
                    }
                    _ => {}
                }
                i += ansi.len();
            } else {
                // Not the start of an ANSI sequence; push a char at a time.
                // Using chars() to handle multi-byte UTF-8 correctly.
                let rest = &text[i..];
                if let Some(ch) = rest.chars().next() {
                    result.push(ch);
                    i += ch.len_utf8();
                } else {
                    i += 1;
                }
            }
        }

        result.push_str(&suffix);
        result
    }

    /// Apply this style to text, padding to `width` visible columns.
    pub fn apply_padded(&self, text: &str, width: usize) -> String {
        let styled = self.apply(text);
        let vw = crate::tui::util::visible_width(&styled);
        if vw < width {
            format!("{}{}", styled, " ".repeat(width - vw))
        } else {
            styled
        }
    }

    /// Check if this style has any foreground color set.
    pub fn has_fg(&self) -> bool {
        self.fg.is_some()
    }

    /// Check if this style has any background color set.
    pub fn has_bg(&self) -> bool {
        self.bg.is_some()
    }
}

/// Compile-time safe theme color keys.
/// Each variant corresponds to a named color in the theme JSON files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeKey {
    Accent,
    BashMode,
    Border,
    BorderAccent,
    BorderMuted,
    CustomMessageBg,
    CustomMessageLabel,
    CustomMessageText,
    Dim,
    Error,
    MdCode,
    MdCodeBlock,
    MdCodeBlockBorder,
    MdHeading,
    MdHr,
    MdLink,
    MdLinkUrl,
    MdListBullet,
    MdQuote,
    MdQuoteBorder,
    Muted,
    SelectedBg,
    Success,
    SyntaxComment,
    SyntaxFunction,
    SyntaxKeyword,
    SyntaxNumber,
    SyntaxOperator,
    SyntaxPunctuation,
    SyntaxString,
    SyntaxType,
    SyntaxVariable,
    Text,
    ThinkingHigh,
    ThinkingLow,
    ThinkingMedium,
    ThinkingMinimal,
    ThinkingOff,
    ThinkingText,
    ThinkingXhigh,
    ToolDiffAdded,
    ToolDiffContext,
    ToolDiffRemoved,
    ToolErrorBg,
    ToolOutput,
    ToolPendingBg,
    ToolSuccessBg,
    ToolTitle,
    UserMessageBg,
    UserMessageText,
    Warning,
}

impl ThemeKey {
    /// Return the string key used in theme JSON configuration.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Accent => "accent",
            Self::BashMode => "bashMode",
            Self::Border => "border",
            Self::BorderAccent => "borderAccent",
            Self::BorderMuted => "borderMuted",
            Self::CustomMessageBg => "customMessageBg",
            Self::CustomMessageLabel => "customMessageLabel",
            Self::CustomMessageText => "customMessageText",
            Self::Dim => "dim",
            Self::Error => "error",
            Self::MdCode => "mdCode",
            Self::MdCodeBlock => "mdCodeBlock",
            Self::MdCodeBlockBorder => "mdCodeBlockBorder",
            Self::MdHeading => "mdHeading",
            Self::MdHr => "mdHr",
            Self::MdLink => "mdLink",
            Self::MdLinkUrl => "mdLinkUrl",
            Self::MdListBullet => "mdListBullet",
            Self::MdQuote => "mdQuote",
            Self::MdQuoteBorder => "mdQuoteBorder",
            Self::Muted => "muted",
            Self::SelectedBg => "selectedBg",
            Self::Success => "success",
            Self::SyntaxComment => "syntaxComment",
            Self::SyntaxFunction => "syntaxFunction",
            Self::SyntaxKeyword => "syntaxKeyword",
            Self::SyntaxNumber => "syntaxNumber",
            Self::SyntaxOperator => "syntaxOperator",
            Self::SyntaxPunctuation => "syntaxPunctuation",
            Self::SyntaxString => "syntaxString",
            Self::SyntaxType => "syntaxType",
            Self::SyntaxVariable => "syntaxVariable",
            Self::Text => "text",
            Self::ThinkingHigh => "thinkingHigh",
            Self::ThinkingLow => "thinkingLow",
            Self::ThinkingMedium => "thinkingMedium",
            Self::ThinkingMinimal => "thinkingMinimal",
            Self::ThinkingOff => "thinkingOff",
            Self::ThinkingText => "thinkingText",
            Self::ThinkingXhigh => "thinkingXhigh",
            Self::ToolDiffAdded => "toolDiffAdded",
            Self::ToolDiffContext => "toolDiffContext",
            Self::ToolDiffRemoved => "toolDiffRemoved",
            Self::ToolErrorBg => "toolErrorBg",
            Self::ToolOutput => "toolOutput",
            Self::ToolPendingBg => "toolPendingBg",
            Self::ToolSuccessBg => "toolSuccessBg",
            Self::ToolTitle => "toolTitle",
            Self::UserMessageBg => "userMessageBg",
            Self::UserMessageText => "userMessageText",
            Self::Warning => "warning",
        }
    }

    /// All theme keys, for iteration.
    pub fn all() -> &'static [ThemeKey] {
        use ThemeKey::*;
        &[
            Accent,
            BashMode,
            Border,
            BorderAccent,
            BorderMuted,
            CustomMessageBg,
            CustomMessageLabel,
            CustomMessageText,
            Dim,
            Error,
            MdCode,
            MdCodeBlock,
            MdCodeBlockBorder,
            MdHeading,
            MdHr,
            MdLink,
            MdLinkUrl,
            MdListBullet,
            MdQuote,
            MdQuoteBorder,
            Muted,
            SelectedBg,
            Success,
            SyntaxComment,
            SyntaxFunction,
            SyntaxKeyword,
            SyntaxNumber,
            SyntaxOperator,
            SyntaxPunctuation,
            SyntaxString,
            SyntaxType,
            SyntaxVariable,
            Text,
            ThinkingHigh,
            ThinkingLow,
            ThinkingMedium,
            ThinkingMinimal,
            ThinkingOff,
            ThinkingText,
            ThinkingXhigh,
            ToolDiffAdded,
            ToolDiffContext,
            ToolDiffRemoved,
            ToolErrorBg,
            ToolOutput,
            ToolPendingBg,
            ToolSuccessBg,
            ToolTitle,
            UserMessageBg,
            UserMessageText,
            Warning,
        ]
    }
}

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

    /// Apply reverse/inverse video styling (used for intra-line diff highlighting).
    fn inverse(&self, text: &str) -> String;

    /// Apply a foreground color from a `ThemeKey`.
    fn fg_key(&self, key: ThemeKey, text: &str) -> String {
        self.fg(key.as_str(), text)
    }

    /// Apply a background color from a `ThemeKey`.
    fn bg_key(&self, key: ThemeKey, text: &str) -> String {
        self.bg(key.as_str(), text)
    }

    /// Return the ANSI escape code for a named color (without text).
    /// Default implementation returns empty string — override in concrete themes.
    fn fg_ansi(&self, _color: &str) -> &str {
        ""
    }

    /// Return the ANSI escape code for a background color (without text).
    /// Default implementation returns empty string — override in concrete themes.
    fn bg_ansi(&self, _color: &str) -> &str {
        ""
    }

    /// Return the ANSI escape code for a `ThemeKey` color (without text).
    fn fg_ansi_key(&self, key: ThemeKey) -> &str {
        self.fg_ansi(key.as_str())
    }
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

    fn inverse(&self, text: &str) -> String {
        text.to_string()
    }
}
