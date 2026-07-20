// ── Pi-compatible /export and /import commands ─────────────────────
//
// Full parity with pi's export functionality:
//
//   /export [path]      — Export session to HTML (default) or JSONL (.jsonl)
//   /import <path>      — Import and resume a session from a JSONL file
//
// JSONL export writes the session header + all branch entries as JSONL,
// re-chaining parentId to form a linear sequence (pi-compatible).
//
// HTML export generates a self-contained HTML file using pi's template
// assets (template.html, template.css, template.js, marked.min.js,
// highlight.min.js), with session data embedded as base64 JSON.

use crate::agent::extension::{CommandHandler, CommandResult};
use crate::agent::session::Session;
use std::path::{Path, PathBuf};

// ── Template assets ────────────────────────────────────────────────
// Embedded from pi's export-html/ directory. Using include_bytes! to
// avoid escaping issues with JS/CSS content.

mod templates {
    pub const TEMPLATE_HTML: &[u8] = include_bytes!("export/templates/template.html");
    pub const TEMPLATE_CSS: &[u8] = include_bytes!("export/templates/template.css");
    pub const TEMPLATE_JS: &[u8] = include_bytes!("export/templates/template.js");
    pub const MARKED_JS: &[u8] = include_bytes!("export/templates/vendor/marked.min.js");
    pub const HIGHLIGHT_JS: &[u8] = include_bytes!("export/templates/vendor/highlight.min.js");
}

// ── Path argument parsing (pi-compatible) ────────────────────────
//
// Matches pi's `getPathCommandArgument()` exactly:
//   /export            → None
//   /export path.html  → Some("path.html")
//   /export "path with spaces.html" → Some("path with spaces.html")
//   /export 'path with spaces.html' → Some("path with spaces.html")

/// Parse the path argument from a command text like `/export path` or `/import path`.
/// Returns `None` if no argument is given (command used bare).
pub fn get_path_command_argument(text: &str, command: &str) -> Option<String> {
    if text == command {
        return None;
    }
    let prefix = format!("{} ", command);
    if !text.starts_with(&prefix) {
        return None;
    }

    let args_string = text[prefix.len()..].trim_start();
    if args_string.is_empty() {
        return None;
    }

    let first_char = args_string.chars().next().unwrap();
    if first_char == '"' || first_char == '\'' {
        let closing = args_string[1..].find(first_char)?;
        return Some(args_string[1..=closing].to_string());
    }

    let first_whitespace = args_string.find(char::is_whitespace);
    match first_whitespace {
        Some(idx) => Some(args_string[..idx].to_string()),
        None => Some(args_string.to_string()),
    }
}

// ── Export error type ──────────────────────────────────────────────

#[derive(Debug)]
pub enum ExportError {
    NoSession,
    InMemorySession,
    IoError(std::io::Error),
    JsonError(serde_json::Error),
    TemplateError(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::NoSession => write!(f, "No active session"),
            ExportError::InMemorySession => {
                write!(
                    f,
                    "Cannot export an in-memory session without a session file"
                )
            }
            ExportError::IoError(e) => write!(f, "IO error: {}", e),
            ExportError::JsonError(e) => write!(f, "JSON error: {}", e),
            ExportError::TemplateError(e) => write!(f, "Template error: {}", e),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        ExportError::IoError(e)
    }
}

impl From<serde_json::Error> for ExportError {
    fn from(e: serde_json::Error) -> Self {
        ExportError::JsonError(e)
    }
}

// ── JSONL export ───────────────────────────────────────────────────
//
// Pi-compatible: writes session header + branch entries as JSONL,
// re-chaining parentId to form a linear sequence.

/// Export the current session branch to a JSONL file.
///
/// * `session` — The session to export
/// * `cwd` — Current working directory (for resolving relative paths)
/// * `output_path` — Target file path. If omitted, generates a timestamped name in cwd
///
/// Returns the resolved output file path.
pub fn export_to_jsonl(
    session: &Session,
    cwd: &Path,
    output_path: Option<&str>,
) -> Result<PathBuf, ExportError> {
    let file_path = match output_path {
        Some(p) => crate::builtin::resolve_path(p, cwd),
        None => {
            let ts = chrono::Utc::now()
                .format("session-%Y-%m-%dT%H-%M-%S")
                .to_string();
            cwd.join(format!("{}.jsonl", ts))
        }
    };

    // Create parent directory
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Build header (pi-compatible: uses fresh timestamp, not original createdAt)
    let _meta = session.metadata();
    let header = serde_json::json!({
        "id": session.session_id(),
        "cwd": session.cwd(),
        "createdAt": chrono::Utc::now().to_rfc3339(),
    });

    // Get branch entries (pi-compatible: linearized path from root to leaf)
    let branch_entries = session.get_branch(None);

    // Build JSONL content
    let mut lines = Vec::with_capacity(branch_entries.len() + 1);
    lines.push(serde_json::to_string(&header)?);

    // Re-chain parentIds to form a linear sequence (pi-compatible)
    let mut prev_id: Option<String> = None;
    for entry in &branch_entries {
        let mut value = serde_json::to_value(entry)?;
        if let Some(obj) = value.as_object_mut() {
            match prev_id {
                Some(ref pid) => {
                    obj.insert(
                        "parentId".to_string(),
                        serde_json::Value::String(pid.clone()),
                    );
                }
                None => {
                    obj.insert("parentId".to_string(), serde_json::Value::Null);
                }
            }
        }
        prev_id = Some(entry.id.clone());
        lines.push(serde_json::to_string(&value)?);
    }

    let content = lines.join("\n") + "\n";
    std::fs::write(&file_path, content)?;

    Ok(file_path)
}

// ── HTML export ────────────────────────────────────────────────────
//
// Pi-compatible: generates a self-contained HTML file using pi's
// template assets with session data embedded as base64 JSON.

/// Theme color mapping for export CSS variables.
struct ExportThemeColors {
    /// CSS custom property declarations
    theme_vars: String,
    /// Body background color
    body_bg: String,
    /// Card/container background color
    container_bg: String,
    /// Info block background color
    info_bg: String,
}

/// Load theme colors for export from the current theme.
/// Mirrors pi's `generateThemeVars()` and `deriveExportColors()`.
fn load_export_theme_colors(theme_name: Option<&str>) -> ExportThemeColors {
    // Try to load the theme config and resolve hex colors
    let colors = resolve_theme_hex_colors(theme_name.unwrap_or("dark"));

    // Build CSS custom property declarations (pi-compatible)
    let mut lines: Vec<String> = Vec::new();
    // Explicitly list the keys we want in the export (matching pi's approach)
    let export_keys = [
        "text",
        "dim",
        "muted",
        "accent",
        "success",
        "error",
        "warning",
        "border",
        "borderAccent",
        "selectedBg",
        "hover",
        "userMessageBg",
        "userMessageText",
        "thinkingText",
        "customMessageBg",
        "customMessageLabel",
        "customMessageText",
        "toolPendingBg",
        "toolSuccessBg",
        "toolErrorBg",
        "toolOutput",
        "toolTitle",
        "toolDiffAdded",
        "toolDiffRemoved",
        "toolDiffContext",
        "mdHeading",
        "mdLink",
        "mdLinkUrl",
        "mdCode",
        "mdCodeBlock",
        "mdCodeBlockBorder",
        "mdQuote",
        "mdQuoteBorder",
        "mdHr",
        "mdListBullet",
        "syntaxComment",
        "syntaxKeyword",
        "syntaxNumber",
        "syntaxString",
        "syntaxFunction",
        "syntaxType",
        "syntaxVariable",
        "syntaxOperator",
        "syntaxPunctuation",
    ];

    for key in &export_keys {
        if let Some(value) = colors.get(*key) {
            lines.push(format!("--{}: {};", key, value));
        }
    }

    // For any remaining colors from the config that aren't in export_keys, include them too
    for (key, value) in &colors {
        if !export_keys.contains(&key.as_str()) {
            lines.push(format!("--{}: {};", key, value));
        }
    }

    let theme_vars = lines.join("\n      ");

    // Derive export background colors from userMessageBg (pi-compatible)
    let user_message_bg = colors
        .get("userMessageBg")
        .map(|s| s.as_str())
        .unwrap_or("#343541");

    let derived = derive_export_colors(user_message_bg);

    ExportThemeColors {
        theme_vars,
        body_bg: derived.page_bg,
        container_bg: derived.card_bg,
        info_bg: derived.info_bg,
    }
}

/// Parse a hex color string to RGB components.
fn parse_hex_color(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Calculate relative luminance of an sRGB color.
fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
    let to_linear = |c: u8| {
        let s = c as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * to_linear(r) + 0.7152 * to_linear(g) + 0.0722 * to_linear(b)
}

/// Adjust color brightness — factor > 1 lightens, < 1 darkens.
fn adjust_brightness(hex: &str, factor: f64) -> String {
    let (r, g, b) = match parse_hex_color(hex) {
        Some(c) => c,
        None => return hex.to_string(),
    };
    let adj = |c: u8| (c as f64 * factor).clamp(0.0, 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", adj(r), adj(g), adj(b))
}

/// Derive export background colors from a base color (pi-compatible).
struct DerivedExportColors {
    page_bg: String,
    card_bg: String,
    info_bg: String,
}

fn derive_export_colors(base_color: &str) -> DerivedExportColors {
    let rgb = match parse_hex_color(base_color) {
        Some(c) => c,
        None => {
            return DerivedExportColors {
                page_bg: "#18181e".to_string(),
                card_bg: "#1e1e24".to_string(),
                info_bg: "#3c3728".to_string(),
            };
        }
    };

    let luminance = relative_luminance(rgb.0, rgb.1, rgb.2);
    let is_light = luminance > 0.5;

    if is_light {
        DerivedExportColors {
            page_bg: adjust_brightness(base_color, 0.96),
            card_bg: base_color.to_string(),
            info_bg: format!(
                "#{:02x}{:02x}{:02x}",
                (rgb.0 as u16 + 10).min(255) as u8,
                (rgb.1 as u16 + 5).min(255) as u8,
                (rgb.2 as u16).max(20).saturating_sub(20) as u8,
            ),
        }
    } else {
        DerivedExportColors {
            page_bg: adjust_brightness(base_color, 0.7),
            card_bg: adjust_brightness(base_color, 0.85),
            info_bg: format!(
                "#{:02x}{:02x}{:02x}",
                (rgb.0 as u16 + 20).min(255) as u8,
                (rgb.1 as u16 + 15).min(255) as u8,
                rgb.2,
            ),
        }
    }
}

/// Resolve theme hex colors by name.
/// Falls back to dark theme if the requested theme can't be loaded.
fn resolve_theme_hex_colors(theme_name: &str) -> std::collections::HashMap<String, String> {
    // Use the theme system's own resolution
    let config = crate::agent::ui::theme::load_theme_config(theme_name)
        .or_else(|_| crate::agent::ui::theme::load_theme_config("dark"))
        .unwrap_or_else(|_| {
            // Ultimate fallback: construct minimal dark theme config
            use crate::agent::ui::theme::{ColorValue, ThemeConfig};
            let mut colors = std::collections::HashMap::new();
            let entries: Vec<(&str, &str)> = vec![
                ("text", "#d4d4d4"),
                ("dim", "#666666"),
                ("muted", "#808080"),
                ("accent", "#8abeb7"),
                ("success", "#b5bd68"),
                ("error", "#cc6666"),
                ("warning", "#e8a838"),
                ("border", "#333"),
                ("borderAccent", "#8abeb7"),
                ("selectedBg", "#2a2a2a"),
                ("hover", "#333"),
                ("userMessageBg", "#343541"),
                ("userMessageText", "#d4d4d4"),
                ("thinkingText", "#808080"),
                ("customMessageBg", "#1e1e24"),
                ("customMessageLabel", "#8abeb7"),
                ("customMessageText", "#d4d4d4"),
                ("toolPendingBg", "#282832"),
                ("toolSuccessBg", "#283228"),
                ("toolErrorBg", "#3c2828"),
                ("toolOutput", "#808080"),
                ("toolTitle", "#d4d4d4"),
                ("toolDiffAdded", "#22c55e"),
                ("toolDiffRemoved", "#ef4444"),
                ("toolDiffContext", "#808080"),
                ("mdHeading", "#e8a838"),
                ("mdLink", "#8abeb7"),
                ("mdLinkUrl", "#5f87af"),
                ("mdCode", "#e8a838"),
                ("mdCodeBlock", "#808080"),
                ("mdCodeBlockBorder", "#444"),
                ("mdQuote", "#808080"),
                ("mdQuoteBorder", "#555"),
                ("mdHr", "#555"),
                ("mdListBullet", "#8abeb7"),
                ("syntaxComment", "#6a9955"),
                ("syntaxKeyword", "#569cd6"),
                ("syntaxNumber", "#b5cea8"),
                ("syntaxString", "#ce9178"),
                ("syntaxFunction", "#dcdcaa"),
                ("syntaxType", "#4ec9b0"),
                ("syntaxVariable", "#9cdcfe"),
                ("syntaxOperator", "#d4d4d4"),
                ("syntaxPunctuation", "#d4d4d4"),
            ];
            for (k, v) in entries {
                colors.insert(k.to_string(), ColorValue::HexOrVar(v.to_string()));
            }
            ThemeConfig {
                name: "dark".to_string(),
                vars: std::collections::HashMap::new(),
                colors,
            }
        });

    crate::agent::ui::theme::RabTheme::resolve_colors(&config)
}

/// Serialize session data to the JSON format expected by pi's template.
///
/// Returns a serde_json::Value matching pi's SessionData interface:
/// ```typescript
/// { header, entries, leafId, systemPrompt?, tools?, renderedTools? }
/// ```
fn build_session_data_json(
    session: &Session,
    system_prompt: Option<&str>,
) -> Result<serde_json::Value, ExportError> {
    let mut header = serde_json::json!({
        "id": session.session_id(),
        "cwd": session.cwd(),
        "createdAt": session.created_at(),
    });
    if let Some(name) = session.session_name() {
        header["name"] = serde_json::json!(name);
    }
    if let Some(ps) = session.parent_session_path()
        && let Some(obj) = header.as_object_mut()
    {
        obj.insert("parentSession".to_string(), serde_json::json!(ps));
    }

    let leaf_id = session.get_leaf_id();

    let mut data = serde_json::json!({
        "header": header,
        "entries": session.get_entries(),
        "leafId": leaf_id,
    });

    // Add optional systemPrompt
    if let Some(sp) = system_prompt
        && let Some(obj) = data.as_object_mut()
    {
        obj.insert(
            "systemPrompt".to_string(),
            serde_json::Value::String(sp.to_string()),
        );
    }

    // Note: `tools` and `renderedTools` are omitted for now.
    // The template JS handles their absence gracefully.
    // Tools can be added later when we have a tool renderer system.

    Ok(data)
}

/// Export the session to a self-contained HTML file.
///
/// * `session` — The session to export
/// * `system_prompt` — Optional system prompt text
/// * `cwd` — Current working directory (for resolving relative paths)
/// * `output_path` — Target file path. If omitted, generates a name in cwd
/// * `theme_name` — Theme name for colors (defaults to current theme)
///
/// Returns the resolved output file path.
pub fn export_to_html(
    session: &Session,
    system_prompt: Option<&str>,
    cwd: &Path,
    output_path: Option<&str>,
    theme_name: Option<&str>,
) -> Result<PathBuf, ExportError> {
    // Determine output path (pi-compatible default naming)
    let file_path = match output_path {
        Some(p) => crate::builtin::resolve_path(p, cwd),
        None => {
            let session_id = session.session_id();
            let short_id = if session_id.len() > 8 {
                session_id.chars().take(8).collect::<String>()
            } else {
                session_id.to_string()
            };
            cwd.join(format!("rab-session-{}.html", short_id))
        }
    };

    // Create parent directory
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Resolve theme name (default to current theme)
    let theme_name = theme_name
        .map(|s| s.to_string())
        .unwrap_or_else(|| crate::agent::ui::theme::current_theme().name.clone());

    // Build session data JSON
    let session_data = build_session_data_json(session, system_prompt)?;
    let session_data_json = serde_json::to_string(&session_data)?;

    // Base64 encode session data (pi-compatible)
    use base64::Engine as _;
    let session_data_base64 =
        base64::engine::general_purpose::STANDARD.encode(session_data_json.as_bytes());

    // Load theme colors
    let export_colors = load_export_theme_colors(Some(&theme_name));

    // Read template files
    let template_html = String::from_utf8(templates::TEMPLATE_HTML.to_vec()).map_err(|e| {
        ExportError::TemplateError(format!("Invalid UTF-8 in template.html: {}", e))
    })?;
    let template_css = String::from_utf8(templates::TEMPLATE_CSS.to_vec())
        .map_err(|e| ExportError::TemplateError(format!("Invalid UTF-8 in template.css: {}", e)))?;
    let template_js = String::from_utf8(templates::TEMPLATE_JS.to_vec())
        .map_err(|e| ExportError::TemplateError(format!("Invalid UTF-8 in template.js: {}", e)))?;
    let marked_js = String::from_utf8(templates::MARKED_JS.to_vec()).map_err(|e| {
        ExportError::TemplateError(format!("Invalid UTF-8 in marked.min.js: {}", e))
    })?;
    let highlight_js = String::from_utf8(templates::HIGHLIGHT_JS.to_vec()).map_err(|e| {
        ExportError::TemplateError(format!("Invalid UTF-8 in highlight.min.js: {}", e))
    })?;

    // Build CSS with theme variables injected (pi-compatible)
    let css = template_css
        .replace("{{THEME_VARS}}", &export_colors.theme_vars)
        .replace("{{BODY_BG}}", &export_colors.body_bg)
        .replace("{{CONTAINER_BG}}", &export_colors.container_bg)
        .replace("{{INFO_BG}}", &export_colors.info_bg);

    // Build final HTML (pi-compatible template injection)
    let html = template_html
        .replace("{{CSS}}", &css)
        .replace("{{JS}}", &template_js)
        .replace("{{SESSION_DATA}}", &session_data_base64)
        .replace("{{MARKED_JS}}", &marked_js)
        .replace("{{HIGHLIGHT_JS}}", &highlight_js);

    std::fs::write(&file_path, html)?;

    Ok(file_path)
}

// ── /export command handler ────────────────────────────────────────

/// Handler for `/export` command.
/// Parses the path argument (pi-compatible) and returns ExportSession result.
pub struct ExportCommand;

impl CommandHandler for ExportCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let text = if args.is_empty() {
            "/export".to_string()
        } else {
            format!("/export {}", args)
        };

        let path = get_path_command_argument(&text, "/export");
        Ok(CommandResult::ExportSession { path })
    }
}

// ── /import command handler ────────────────────────────────────────

/// Handler for `/import` command.
/// Parses the path argument (pi-compatible) and returns ImportSession result.
pub struct ImportCommand;

impl CommandHandler for ImportCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let text = if args.is_empty() {
            "/import".to_string()
        } else {
            format!("/import {}", args)
        };

        let path = get_path_command_argument(&text, "/import");
        match path {
            Some(p) => Ok(CommandResult::ImportSession { path: p }),
            None => Ok(CommandResult::Info(
                "Usage: /import <path.jsonl>".to_string(),
            )),
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_path_no_arg() {
        assert_eq!(get_path_command_argument("/export", "/export"), None);
        assert_eq!(get_path_command_argument("/import", "/import"), None);
    }

    #[test]
    fn test_get_path_simple() {
        assert_eq!(
            get_path_command_argument("/export output.html", "/export"),
            Some("output.html".to_string())
        );
    }

    #[test]
    fn test_get_path_quoted_double() {
        assert_eq!(
            get_path_command_argument("/export \"my session.html\"", "/export"),
            Some("my session.html".to_string())
        );
    }

    #[test]
    fn test_get_path_quoted_single() {
        assert_eq!(
            get_path_command_argument("/export 'my session.html'", "/export"),
            Some("my session.html".to_string())
        );
    }

    #[test]
    fn test_get_path_no_close_quote() {
        assert_eq!(
            get_path_command_argument("/export \"no close", "/export"),
            None
        );
    }

    #[test]
    fn test_get_path_command_prefix_mismatch() {
        assert_eq!(
            get_path_command_argument("/exporter out.html", "/export"),
            None
        );
    }

    #[test]
    fn test_get_path_only_whitespace() {
        assert_eq!(get_path_command_argument("/export  ", "/export"), None);
        assert_eq!(get_path_command_argument("/import  ", "/import"), None);
    }

    #[test]
    fn test_export_command_no_args() {
        let cmd = ExportCommand;
        let result = cmd.execute("").unwrap();
        match result {
            CommandResult::ExportSession { path } => assert_eq!(path, None),
            _ => panic!("Expected ExportSession with None"),
        }
    }

    #[test]
    fn test_export_command_with_path() {
        let cmd = ExportCommand;
        let result = cmd.execute("test.html").unwrap();
        match result {
            CommandResult::ExportSession { path } => {
                assert_eq!(path, Some("test.html".to_string()));
            }
            _ => panic!("Expected ExportSession with path"),
        }
    }

    #[test]
    fn test_import_command_no_args() {
        let cmd = ImportCommand;
        let result = cmd.execute("").unwrap();
        match result {
            CommandResult::Info(msg) => assert!(msg.contains("Usage:")),
            _ => panic!("Expected Info message"),
        }
    }

    #[test]
    fn test_import_command_with_path() {
        let cmd = ImportCommand;
        let result = cmd.execute("session.jsonl").unwrap();
        match result {
            CommandResult::ImportSession { path } => {
                assert_eq!(path, "session.jsonl");
            }
            _ => panic!("Expected ImportSession"),
        }
    }

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(parse_hex_color("#ff0000"), Some((255, 0, 0)));
        assert_eq!(parse_hex_color("00ff00"), Some((0, 255, 0)));
        assert_eq!(parse_hex_color("#fff"), None);
        assert_eq!(parse_hex_color("invalid"), None);
    }

    #[test]
    fn test_relative_luminance() {
        let black = relative_luminance(0, 0, 0);
        let white = relative_luminance(255, 255, 255);
        assert!(black < 0.1);
        assert!(white > 0.9);
    }

    #[test]
    fn test_derive_export_colors_dark() {
        let derived = derive_export_colors("#343541");
        // Dark theme: pageBg should be darker than cardBg
        let page_rgb = parse_hex_color(&derived.page_bg).unwrap();
        let card_rgb = parse_hex_color(&derived.card_bg).unwrap();
        assert!(page_rgb.0 < card_rgb.0); // Darker page background
    }

    #[test]
    fn test_derive_export_colors_light() {
        let derived = derive_export_colors("#ffffff");
        // Light theme: pageBg should be slightly darker than white
        let page_rgb = parse_hex_color(&derived.page_bg).unwrap();
        assert!(page_rgb.0 < 255);
    }

    #[test]
    fn test_adjust_brightness() {
        let result = adjust_brightness("#808080", 0.5);
        let rgb = parse_hex_color(&result).unwrap();
        // 128 * 0.5 = 64
        assert!(rgb.0 <= 70 && rgb.0 >= 60);
    }
}
