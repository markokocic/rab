use crate::tui::Theme;
use crate::tui::components::markdown::{MarkdownTheme, StyleFn, create_highlight_fn};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU16;

// ── Color Types ──────────────────────────────────────────────────

/// A color value in a theme config: hex string "#ff0000", var reference "accent",
/// 256-color index (0-255), or empty string for default terminal color.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ColorValue {
    HexOrVar(String),
    Index(u8),
}

/// Theme JSON structure matching pi's theme format.
#[derive(Debug, Clone, Deserialize)]
pub struct ThemeConfig {
    pub name: String,
    #[serde(default)]
    pub vars: HashMap<String, String>,
    pub colors: HashMap<String, ColorValue>,
}

/// Terminal color capability mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
}

// ── RabTheme ─────────────────────────────────────────────────────

/// The concrete theme used by the rab UI.
/// Wraps resolved ANSI escape codes for foregrounds, backgrounds, and text styling.
#[derive(Debug, Clone)]
pub struct RabTheme {
    pub name: String,
    mode: ColorMode,
    fg_ansi: HashMap<String, String>,
    bg_ansi: HashMap<String, String>,
}

impl RabTheme {
    /// Parse a hex color like "#ff0000" into (r,g,b).
    fn hex_to_rgb(hex: &str) -> Option<(u8, u8, u8)> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some((r, g, b))
    }

    /// Convert an (r,g,b) to the closest 256-color ANSI index.
    fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
        const CUBE_VALUES: [u8; 6] = [0, 95, 135, 175, 215, 255];
        const GRAY_VALUES: [u8; 24] = [
            8, 18, 28, 38, 48, 58, 68, 78, 88, 98, 108, 118, 128, 138, 148, 158, 168, 178, 188,
            198, 208, 218, 228, 238,
        ];

        let find_closest = |value: u8, table: &[u8]| -> usize {
            let mut min_dist = u16::MAX;
            let mut min_idx = 0;
            for (i, &v) in table.iter().enumerate() {
                let dist = value.abs_diff(v);
                if (dist as u16) < min_dist {
                    min_dist = dist as u16;
                    min_idx = i;
                }
            }
            min_idx
        };

        let ri = find_closest(r, &CUBE_VALUES);
        let gi = find_closest(g, &CUBE_VALUES);
        let bi = find_closest(b, &CUBE_VALUES);
        let cube_index = 16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8;

        // Check grayscale
        let gray = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
        let gi = find_closest(gray as u8, &GRAY_VALUES);
        let gray_index = 232 + gi as u8;

        let spread = r.max(g).max(b) - r.min(g).min(b);
        if spread < 10 {
            return gray_index;
        }
        cube_index
    }

    /// Build the ANSI escape code for a foreground color.
    fn fg_escape(color: &str, mode: ColorMode) -> String {
        if color.is_empty() {
            return "\x1b[39m".to_string();
        }
        if let Ok(idx) = color.parse::<u8>() {
            return format!("\x1b[38;5;{}m", idx);
        }
        if let Some((r, g, b)) = Self::hex_to_rgb(color) {
            return match mode {
                ColorMode::TrueColor => format!("\x1b[38;2;{};{};{}m", r, g, b),
                ColorMode::Ansi256 => format!("\x1b[38;5;{}m", Self::rgb_to_256(r, g, b)),
            };
        }
        "\x1b[39m".to_string()
    }

    /// Build the ANSI escape code for a background color.
    fn bg_escape(color: &str, mode: ColorMode) -> String {
        if color.is_empty() {
            return "\x1b[49m".to_string();
        }
        if let Ok(idx) = color.parse::<u8>() {
            return format!("\x1b[48;5;{}m", idx);
        }
        if let Some((r, g, b)) = Self::hex_to_rgb(color) {
            return match mode {
                ColorMode::TrueColor => format!("\x1b[48;2;{};{};{}m", r, g, b),
                ColorMode::Ansi256 => format!("\x1b[48;5;{}m", Self::rgb_to_256(r, g, b)),
            };
        }
        "\x1b[49m".to_string()
    }

    /// Resolve variable references and color values to hex strings.
    pub fn resolve_colors(config: &ThemeConfig) -> HashMap<String, String> {
        let mut resolved: HashMap<String, String> = HashMap::new();

        for (name, value) in &config.colors {
            let hex = match value {
                ColorValue::HexOrVar(s) => {
                    if s.starts_with('#') {
                        s.clone()
                    } else if let Some(v) = config.vars.get(s) {
                        v.clone()
                    } else {
                        s.clone()
                    }
                }
                ColorValue::Index(idx) => idx.to_string(),
            };
            resolved.insert(name.clone(), hex);
        }
        resolved
    }

    /// Background color keys (matching pi's bgColorKeys set).
    const BG_KEYS: &'static [&'static str] = &[
        "selectedBg",
        "userMessageBg",
        "customMessageBg",
        "toolPendingBg",
        "toolSuccessBg",
        "toolErrorBg",
        "thinking_bg",
    ];

    /// Build a RabTheme from a ThemeConfig.
    pub fn from_config(config: &ThemeConfig, mode: ColorMode) -> Self {
        let colors = Self::resolve_colors(config);

        let mut fg_ansi = HashMap::new();
        let mut bg_ansi = HashMap::new();

        for (key, value) in &colors {
            if Self::BG_KEYS.contains(&key.as_str()) {
                bg_ansi.insert(key.clone(), Self::bg_escape(value, mode));
            } else {
                fg_ansi.insert(key.clone(), Self::fg_escape(value, mode));
            }
        }

        // Add thinking_bg as a derived background from thinkingText
        if let Some(text_color) = colors.get("thinkingText")
            && !bg_ansi.contains_key("thinking_bg")
        {
            // Darken thinkingText for background
            let bg_color = if let Some((r, g, b)) = Self::hex_to_rgb(text_color) {
                let dr = (r as f64 * 0.7) as u8;
                let dg = (g as f64 * 0.7) as u8;
                let db = (b as f64 * 0.7) as u8;
                format!("#{:02x}{:02x}{:02x}", dr, dg, db)
            } else {
                text_color.clone()
            };
            bg_ansi.insert("thinking_bg".to_string(), Self::bg_escape(&bg_color, mode));
        }

        Self {
            name: config.name.clone(),
            mode,
            fg_ansi,
            bg_ansi,
        }
    }

    /// Get the ANSI foreground escape code for a color name.
    pub fn fg_ansi(&self, color: &str) -> &str {
        self.fg_ansi
            .get(color)
            .map(|s| s.as_str())
            .unwrap_or("\x1b[39m")
    }

    /// Get the ANSI background escape code for a color name.
    pub fn bg_ansi(&self, color: &str) -> &str {
        self.bg_ansi
            .get(color)
            .map(|s| s.as_str())
            .unwrap_or("\x1b[49m")
    }

    /// Apply a foreground color to text.
    pub fn fg(&self, color: &str, text: &str) -> String {
        format!("{}{}\x1b[39m", self.fg_ansi(color), text)
    }

    /// Apply a background color to text.
    pub fn bg(&self, color: &str, text: &str) -> String {
        format!("{}{}\x1b[49m", self.bg_ansi(color), text)
    }

    /// Apply bold styling.
    pub fn bold(&self, text: &str) -> String {
        format!("\x1b[1m{}\x1b[22m", text)
    }

    /// Apply italic styling.
    pub fn italic(&self, text: &str) -> String {
        format!("\x1b[3m{}\x1b[23m", text)
    }

    /// Apply reverse/inverse video styling (used for intra-line diff highlighting).
    pub fn inverse(&self, text: &str) -> String {
        format!("\x1b[7m{}\x1b[27m", text)
    }

    /// Apply underline styling.
    pub fn underline(&self, text: &str) -> String {
        format!("\x1b[4m{}\x1b[24m", text)
    }

    /// Apply strikethrough styling.
    pub fn strikethrough(&self, text: &str) -> String {
        format!("\x1b[9m{}\x1b[29m", text)
    }

    /// Get the color mode.
    pub fn color_mode(&self) -> ColorMode {
        self.mode
    }

    /// Convenience: apply bold + fg
    pub fn bold_fg(&self, color: &str, text: &str) -> String {
        format!("\x1b[1m{}{}\x1b[22m\x1b[39m", self.fg_ansi(color), text)
    }

    // ── Convenience helpers matching the old RabTheme API ──

    /// Apply accent foreground color.
    pub fn accent(&self, text: &str) -> String {
        self.fg("accent", text)
    }

    /// Apply dim foreground color.
    pub fn dim(&self, text: &str) -> String {
        self.fg("dim", text)
    }

    /// Apply muted foreground color.
    pub fn muted(&self, text: &str) -> String {
        self.fg("muted", text)
    }

    /// Apply success foreground color.
    pub fn success(&self, text: &str) -> String {
        self.fg("success", text)
    }

    /// Apply error foreground color.
    pub fn error(&self, text: &str) -> String {
        self.fg("error", text)
    }

    /// Apply text foreground color.
    pub fn text_color(&self, text: &str) -> String {
        self.fg("text", text)
    }

    /// Apply border foreground color.
    pub fn border(&self, text: &str) -> String {
        self.fg("border", text)
    }

    /// Apply user message background.
    pub fn user_msg_bg(&self, text: &str) -> String {
        self.bg("userMessageBg", text)
    }

    /// Apply thinking block background.
    pub fn thinking_bg(&self, text: &str) -> String {
        self.bg("thinking_bg", text)
    }

    /// Bold + accent foreground.
    pub fn bold_accent(&self, text: &str) -> String {
        self.bold_fg("accent", text)
    }

    // ── Style API ──

    /// Create a `Style` with a foreground color resolved from a color name.
    pub fn fg_style(&self, color: &str) -> crate::tui::Style {
        crate::tui::Style::new().fg(self.fg_ansi(color).to_string())
    }

    /// Create a `Style` with a background color resolved from a color name.
    pub fn bg_style(&self, color: &str) -> crate::tui::Style {
        crate::tui::Style::new().bg(self.bg_ansi(color).to_string())
    }
}

/// Agent-specific color key constants.
///
/// Each constant corresponds to a named color in the theme JSON files.
/// These are separate from the generic `tui::Theme` trait to keep
/// agent-specific concepts out of the reusable TUI layer.
#[allow(non_upper_case_globals)]
pub mod color {
    pub const Accent: &str = "accent";
    pub const BashMode: &str = "bashMode";
    pub const Border: &str = "border";
    pub const BorderAccent: &str = "borderAccent";
    pub const BorderMuted: &str = "borderMuted";
    pub const CustomMessageBg: &str = "customMessageBg";
    pub const CustomMessageLabel: &str = "customMessageLabel";
    pub const CustomMessageText: &str = "customMessageText";
    pub const Dim: &str = "dim";
    pub const Error: &str = "error";
    pub const MdCode: &str = "mdCode";
    pub const MdCodeBlock: &str = "mdCodeBlock";
    pub const MdCodeBlockBorder: &str = "mdCodeBlockBorder";
    pub const MdHeading: &str = "mdHeading";
    pub const MdHr: &str = "mdHr";
    pub const MdLink: &str = "mdLink";
    pub const MdLinkUrl: &str = "mdLinkUrl";
    pub const MdListBullet: &str = "mdListBullet";
    pub const MdQuote: &str = "mdQuote";
    pub const MdQuoteBorder: &str = "mdQuoteBorder";
    pub const Muted: &str = "muted";
    pub const SelectedBg: &str = "selectedBg";
    pub const Success: &str = "success";
    pub const SyntaxComment: &str = "syntaxComment";
    pub const SyntaxFunction: &str = "syntaxFunction";
    pub const SyntaxKeyword: &str = "syntaxKeyword";
    pub const SyntaxNumber: &str = "syntaxNumber";
    pub const SyntaxOperator: &str = "syntaxOperator";
    pub const SyntaxPunctuation: &str = "syntaxPunctuation";
    pub const SyntaxString: &str = "syntaxString";
    pub const SyntaxType: &str = "syntaxType";
    pub const SyntaxVariable: &str = "syntaxVariable";
    pub const Text: &str = "text";
    pub const ThinkingHigh: &str = "thinkingHigh";
    pub const ThinkingLow: &str = "thinkingLow";
    pub const ThinkingMedium: &str = "thinkingMedium";
    pub const ThinkingMinimal: &str = "thinkingMinimal";
    pub const ThinkingOff: &str = "thinkingOff";
    pub const ThinkingText: &str = "thinkingText";
    pub const ThinkingXhigh: &str = "thinkingXhigh";
    pub const ToolDiffAdded: &str = "toolDiffAdded";
    pub const ToolDiffContext: &str = "toolDiffContext";
    pub const ToolDiffRemoved: &str = "toolDiffRemoved";
    pub const ToolErrorBg: &str = "toolErrorBg";
    pub const ToolOutput: &str = "toolOutput";
    pub const ToolPendingBg: &str = "toolPendingBg";
    pub const ToolSuccessBg: &str = "toolSuccessBg";
    pub const ToolTitle: &str = "toolTitle";
    pub const UserMessageBg: &str = "userMessageBg";
    pub const UserMessageText: &str = "userMessageText";
    pub const Warning: &str = "warning";

    /// All color key strings, for iteration over available theme keys.
    pub const ALL: &[&str] = &[
        "accent",
        "bashMode",
        "border",
        "borderAccent",
        "borderMuted",
        "customMessageBg",
        "customMessageLabel",
        "customMessageText",
        "dim",
        "error",
        "mdCode",
        "mdCodeBlock",
        "mdCodeBlockBorder",
        "mdHeading",
        "mdHr",
        "mdLink",
        "mdLinkUrl",
        "mdListBullet",
        "mdQuote",
        "mdQuoteBorder",
        "muted",
        "selectedBg",
        "success",
        "syntaxComment",
        "syntaxFunction",
        "syntaxKeyword",
        "syntaxNumber",
        "syntaxOperator",
        "syntaxPunctuation",
        "syntaxString",
        "syntaxType",
        "syntaxVariable",
        "text",
        "thinkingHigh",
        "thinkingLow",
        "thinkingMedium",
        "thinkingMinimal",
        "thinkingOff",
        "thinkingText",
        "thinkingXhigh",
        "toolDiffAdded",
        "toolDiffContext",
        "toolDiffRemoved",
        "toolErrorBg",
        "toolOutput",
        "toolPendingBg",
        "toolSuccessBg",
        "toolTitle",
        "userMessageBg",
        "userMessageText",
        "warning",
    ];
}

impl Theme for RabTheme {
    fn fg(&self, color: &str, text: &str) -> String {
        self.fg(color, text)
    }

    fn bg(&self, color: &str, text: &str) -> String {
        self.bg(color, text)
    }

    fn bold(&self, text: &str) -> String {
        self.bold(text)
    }

    fn italic(&self, text: &str) -> String {
        self.italic(text)
    }

    fn inverse(&self, text: &str) -> String {
        self.inverse(text)
    }

    fn fg_ansi(&self, color: &str) -> &str {
        self.fg_ansi(color)
    }

    fn bg_ansi(&self, color: &str) -> &str {
        self.bg_ansi(color)
    }
}

// ── Global Theme State ───────────────────────────────────────────

use std::sync::{Mutex, OnceLock};

static THEME: OnceLock<Mutex<RabTheme>> = OnceLock::new();
static THEME_MODE: AtomicU16 = AtomicU16::new(1); // 1=truecolor

fn get_theme_lock() -> &'static Mutex<RabTheme> {
    THEME.get_or_init(|| Mutex::new(fallback_theme()))
}

/// Initialize the theme system. Call once at startup.
pub fn init_theme(theme_name: Option<&str>, force_256: bool) {
    let mode = if force_256 {
        ColorMode::Ansi256
    } else {
        ColorMode::TrueColor
    };
    THEME_MODE.store(
        if force_256 { 2 } else { 1 },
        std::sync::atomic::Ordering::Relaxed,
    );

    let name = theme_name.unwrap_or("dark");
    match load_theme_config(name) {
        Ok(config) => {
            let theme = RabTheme::from_config(&config, mode);
            if let Ok(mut t) = get_theme_lock().lock() {
                *t = theme;
            }
        }
        Err(_) => {
            // Fall back to dark
            if name != "dark"
                && let Ok(config) = load_theme_config("dark")
            {
                let theme = RabTheme::from_config(&config, mode);
                if let Ok(mut t) = get_theme_lock().lock() {
                    *t = theme;
                }
            }
        }
    }
}

/// Load a theme by name. Checks built-in themes first, then custom themes directory.
pub fn load_theme_config(name: &str) -> Result<ThemeConfig, String> {
    match name {
        "dark" => {
            let json = include_str!("themes/dark.json");
            serde_json::from_str::<ThemeConfig>(json).map_err(|e| e.to_string())
        }
        "light" => {
            let json = include_str!("themes/light.json");
            serde_json::from_str::<ThemeConfig>(json).map_err(|e| e.to_string())
        }
        _ => {
            let themes_dir = get_themes_dir();
            let theme_path = themes_dir.join(format!("{}.json", name));
            if theme_path.exists() {
                let content = std::fs::read_to_string(&theme_path).map_err(|e| e.to_string())?;
                serde_json::from_str::<ThemeConfig>(&content).map_err(|e| e.to_string())
            } else {
                Err(format!("Theme not found: {}", name))
            }
        }
    }
}

/// Get the custom themes directory (~/.rab/themes).
fn get_themes_dir() -> PathBuf {
    let base = directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".rab"))
        .unwrap_or_else(|| PathBuf::from("/tmp/.rab"));
    let dir = base.join("themes");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Get available theme names.
pub fn get_available_themes() -> Vec<String> {
    let mut themes: Vec<String> = vec!["dark".to_string(), "light".to_string()];

    let themes_dir = get_themes_dir();
    if let Ok(entries) = std::fs::read_dir(&themes_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
                && name != "dark"
                && name != "light"
            {
                themes.push(name.to_string());
            }
        }
    }

    themes.sort();
    themes.dedup();
    themes
}

/// Get the current theme.
pub fn current_theme() -> std::sync::MutexGuard<'static, RabTheme> {
    get_theme_lock().lock().expect("Theme lock poisoned")
}

/// Set a new theme by name. Returns success/error.
pub fn set_theme(name: &str) -> Result<(), String> {
    let mode = match THEME_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        2 => ColorMode::Ansi256,
        _ => ColorMode::TrueColor,
    };
    let config = load_theme_config(name)?;
    let theme = RabTheme::from_config(&config, mode);
    if let Ok(mut t) = get_theme_lock().lock() {
        *t = theme;
    }
    Ok(())
}

/// Detect terminal background theme using environment variables.
/// Returns "dark" or "light".
pub fn detect_terminal_theme() -> &'static str {
    if let Ok(colorfgbg) = std::env::var("COLORFGBG")
        && let Some(bg_str) = colorfgbg.split(';').next_back()
        && let Ok(bg) = bg_str.trim().parse::<u8>()
    {
        let luminance = match bg {
            0..=7 => 0.2,
            8..=15 => 0.8,
            _ => {
                // 256-color: approximate luminance

                (bg - 16) as f64 / 239.0
            }
        };
        return if luminance > 0.5 { "light" } else { "dark" };
    }
    "dark"
}

/// Fallback theme for when no theme is loaded yet.
fn fallback_theme() -> RabTheme {
    let mut config = ThemeConfig {
        name: "dark".into(),
        vars: HashMap::new(),
        colors: HashMap::new(),
    };
    let entries: Vec<(&str, &str)> = vec![
        ("text", "#d4d4d4"),
        ("dim", "#666666"),
        ("muted", "#808080"),
        ("accent", "#8abeb7"),
        ("success", "#b5bd68"),
        ("error", "#cc6666"),
        ("warning", "#ffff00"),
        ("thinkingText", "#808080"),
        ("thinking_level_low", "#5f87af"),
        ("thinking_level_medium", "#81a2be"),
        ("thinking_level_high", "#b294bb"),
        ("thinking_level_xhigh", "#d183e8"),
        ("userMessageBg", "#343541"),
        ("toolPendingBg", "#282832"),
        ("toolSuccessBg", "#283228"),
        ("toolErrorBg", "#3c2828"),
        ("toolTitle", "#d4d4d4"),
        ("toolOutput", "#808080"),
    ];
    for (k, v) in entries {
        config
            .colors
            .insert(k.to_string(), ColorValue::HexOrVar(v.to_string()));
    }
    RabTheme::from_config(&config, ColorMode::TrueColor)
}

/// Build a `MarkdownTheme` from the current `RabTheme`.
/// Wires all existing `md*` colors and text decorations.
pub fn get_markdown_theme() -> MarkdownTheme {
    let theme = current_theme();

    let heading = mk_style(theme.fg_ansi("mdHeading"));
    let link = mk_style(theme.fg_ansi("mdLink"));
    let link_url = mk_style(theme.fg_ansi("mdLinkUrl"));
    let code = mk_style(theme.fg_ansi("mdCode"));
    let code_block = mk_style(theme.fg_ansi("mdCodeBlock"));
    let code_block_border = mk_style(theme.fg_ansi("mdCodeBlockBorder"));
    let quote = mk_style(theme.fg_ansi("mdQuote"));
    let quote_border = mk_style(theme.fg_ansi("mdQuoteBorder"));
    let hr = mk_style(theme.fg_ansi("mdHr"));
    let list_bullet = mk_style(theme.fg_ansi("mdListBullet"));

    // Release the lock before building closures
    drop(theme);

    let mut md = MarkdownTheme::new(
        heading,
        link,
        link_url,
        code,
        code_block,
        code_block_border,
        quote,
        quote_border,
        hr,
        list_bullet,
        style_bold(),
        style_italic(),
        style_strikethrough(),
        style_underline(),
    );
    md.highlight_code = create_highlight_fn();
    md
}

/// Build a style function that wraps text with a foreground ANSI prefix and reset suffix.
fn mk_style(prefix: &str) -> StyleFn {
    let p = prefix.to_string();
    Arc::new(move |text: &str| format!("{}{}\x1b[39m", p, text))
}

/// Build a bold style function.
fn style_bold() -> StyleFn {
    Arc::new(|text: &str| format!("\x1b[1m{}\x1b[22m", text))
}

/// Build an italic style function.
fn style_italic() -> StyleFn {
    Arc::new(|text: &str| format!("\x1b[3m{}\x1b[23m", text))
}

/// Build a strikethrough style function.
fn style_strikethrough() -> StyleFn {
    Arc::new(|text: &str| format!("\x1b[9m{}\x1b[29m", text))
}

/// Build an underline style function.
fn style_underline() -> StyleFn {
    Arc::new(|text: &str| format!("\x1b[4m{}\x1b[24m", text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_dark_theme() {
        let config = load_theme_config("dark").unwrap();
        assert_eq!(config.name, "dark");
        assert!(config.colors.contains_key("accent"));
        assert!(config.colors.contains_key("text"));
    }

    #[test]
    fn test_load_light_theme() {
        let config = load_theme_config("light").unwrap();
        assert_eq!(config.name, "light");
        assert!(config.colors.contains_key("accent"));
    }

    #[test]
    fn test_resolve_colors() {
        let config = load_theme_config("dark").unwrap();
        let colors = RabTheme::resolve_colors(&config);
        assert!(colors.contains_key("accent"));
        assert!(colors.contains_key("text"));
        assert!(colors.get("accent").unwrap().starts_with('#'));
    }

    #[test]
    fn test_theme_from_config() {
        let config = load_theme_config("dark").unwrap();
        let theme = RabTheme::from_config(&config, ColorMode::TrueColor);
        let colored = theme.fg("accent", "hello");
        assert!(colored.contains("hello"));
        assert!(colored.contains("\x1b[38;2;"));
        assert!(colored.ends_with("\x1b[39m"));
    }

    #[test]
    fn test_theme_256_fallback() {
        let config = load_theme_config("dark").unwrap();
        let theme = RabTheme::from_config(&config, ColorMode::Ansi256);
        let colored = theme.fg("accent", "hello");
        assert!(colored.contains("hello"));
        assert!(colored.contains("\x1b[38;5;"));
    }

    #[test]
    fn test_bold_italic() {
        let config = load_theme_config("dark").unwrap();
        let theme = RabTheme::from_config(&config, ColorMode::TrueColor);
        assert_eq!(theme.bold("x"), "\x1b[1mx\x1b[22m");
        assert_eq!(theme.italic("x"), "\x1b[3mx\x1b[23m");
    }

    #[test]
    fn test_hex_to_rgb() {
        assert_eq!(RabTheme::hex_to_rgb("#ff0000"), Some((255, 0, 0)));
        assert_eq!(RabTheme::hex_to_rgb("00ff00"), Some((0, 255, 0)));
        assert_eq!(RabTheme::hex_to_rgb("#zzz"), None);
    }

    #[test]
    fn test_fallback_theme() {
        let theme = fallback_theme();
        assert_eq!(theme.name, "dark");
        let text = theme.fg("text", "test");
        assert!(text.contains("test"));
    }

    #[test]
    fn test_set_and_get() {
        init_theme(Some("dark"), false);
        let theme = current_theme();
        assert_eq!(theme.name, "dark");
    }
}
