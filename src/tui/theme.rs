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

        format!("{}{}{}", prefix, text, suffix)
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
}
