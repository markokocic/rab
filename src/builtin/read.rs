use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use anyhow::Context;
use async_trait::async_trait;
use base64::Engine as _;
use std::borrow::Cow;
use std::path::Path;
use tokio::sync::mpsc::UnboundedSender;
use unicode_normalization::UnicodeNormalization;

pub struct ReadExtension {
    cwd: std::path::PathBuf,
}

impl ReadExtension {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for ReadExtension {
    fn name(&self) -> Cow<'static, str> {
        "read".into()
    }

    fn tools(&self) -> Vec<Box<dyn yoagent::types::AgentTool>> {
        vec![Box::new(ReadTool {
            cwd: self.cwd.clone(),
        })]
    }

    fn tool_renderer(&self, name: &str) -> Option<Box<dyn ToolRenderer>> {
        if name == "read" {
            Some(Box::new(ReadRenderer {
                cwd: self.cwd.clone(),
            }))
        } else {
            None
        }
    }

    fn tool_snippets(&self) -> Vec<(String, std::borrow::Cow<'static, str>)> {
        vec![("read".into(), "Read file contents".into())]
    }

    fn tool_guidelines(&self) -> Vec<(String, String)> {
        vec![(
            "read".into(),
            "Use read to examine files instead of cat or sed.".into(),
        )]
    }
}

struct ReadTool {
    cwd: std::path::PathBuf,
}

// ── Constants ────────────────────────────────────────────────────

const DEFAULT_MAX_LINES: usize = 2000;
const DEFAULT_MAX_BYTES: usize = 50 * 1024; // 50KB

// ── Helpers ──────────────────────────────────────────────────────

/// Format bytes as a human-readable size string, matching pi's format.
fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Trim trailing empty lines from a slice of lines.
fn trim_trailing_empty_lines<'a>(lines: &'a [&'a str]) -> &'a [&'a str] {
    let mut end = lines.len();
    while end > 0 && lines[end - 1].is_empty() {
        end -= 1;
    }
    &lines[..end]
}

// ── macOS path variant resolution (matching pi's resolveReadPathAsync) ──

/// Try macOS filename variant: narrow no-break space before AM/PM.
fn try_macos_am_pm_variant(path: &str) -> Option<String> {
    // macOS screenshot names: "Screen Shot 2023-01-01 at 10.30.00 AM.png"
    // Users type "10.30.00 AM" with regular space, macOS stores with narrow no-break space.
    let narrow_nbsp = "\u{202F}";
    if path.contains(" AM") || path.contains(" PM") {
        let variant = path
            .replace(" AM", &format!("{}AM", narrow_nbsp))
            .replace(" PM", &format!("{}PM", narrow_nbsp));
        if variant != path && std::path::Path::new(&variant).exists() {
            return Some(variant);
        }
    }
    None
}

/// Try macOS NFD filename variant.
fn try_nfd_variant(path: &str) -> Option<String> {
    // macOS stores filenames in NFD (decomposed) form.
    // Try converting the user input to NFD.
    let nfd: String = path.nfkd().collect();
    if nfd != path && std::path::Path::new(&nfd).exists() {
        return Some(nfd);
    }
    None
}

/// Try curly quote variant for macOS screenshot names.
fn try_curly_quote_variant(path: &str) -> Option<String> {
    // macOS uses U+2019 (right single quotation mark) in screenshot names like "Capture d'écran"
    // Users typically type U+0027 (straight apostrophe)
    let variant = path.replace('\'', "\u{2019}");
    if variant != path && std::path::Path::new(&variant).exists() {
        return Some(variant);
    }
    None
}

/// Resolve a read path trying macOS filename variants if the direct path doesn't exist.
/// Matching pi's `resolveReadPath` (sync version).
fn resolve_read_path(path: &str, cwd: &Path) -> std::path::PathBuf {
    let resolved = {
        let p = std::path::Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        }
    };

    if resolved.exists() {
        return resolved;
    }

    let resolved_str = resolved.to_string_lossy();

    // Try macOS AM/PM variant
    if let Some(variant) = try_macos_am_pm_variant(&resolved_str) {
        return std::path::PathBuf::from(variant);
    }

    // Try NFD variant
    if let Some(variant) = try_nfd_variant(&resolved_str) {
        return std::path::PathBuf::from(variant);
    }

    // Try curly quote variant
    if let Some(variant) = try_curly_quote_variant(&resolved_str) {
        return std::path::PathBuf::from(variant);
    }

    resolved
}

// ── Image detection (magic bytes) ─────────────────────────────────────

/// Detect image MIME type from file magic bytes (matching pi's
/// `detectSupportedImageMimeTypeFromFile`). Uses the first 12 bytes
/// of the file to identify the format.
#[allow(clippy::redundant_guards)]
fn detect_image_mime(path: &Path) -> std::io::Result<Option<&'static str>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; 12];
    let n = file.read(&mut buf)?;
    if n < 4 {
        return Ok(None);
    }
    Ok(match &buf[..n] {
        b if b.starts_with(b"\x89PNG\r\n\x1a\n") && n >= 8 => Some("image/png"),
        b if b.starts_with(&[0xFF, 0xD8, 0xFF]) => Some("image/jpeg"),
        b if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") => Some("image/gif"),
        b if n >= 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" => Some("image/webp"),
        b if b.starts_with(b"\x89\x50\x4E\x47\x0D\x0A\x1A\x0A") => Some("image/png"),
        _ => None,
    })
}

/// Compact read classification matching pi's `CompactReadClassification`.
#[derive(Debug, PartialEq)]
enum CompactReadKind {
    Resource,
    Skill,
}

/// Build a compact classification for the read tool output, matching pi's `getCompactReadClassification`.
/// Returns `None` for regular files.
fn get_compact_read_classification(path: &str, cwd: &Path) -> Option<(CompactReadKind, String)> {
    let abs_path = if Path::new(path).is_absolute() {
        Path::new(path).to_path_buf()
    } else {
        cwd.join(path)
    };

    let file_name = abs_path.file_name()?.to_str()?;

    // AGENTS.md / CLAUDE.md → resource with path relative to cwd
    if file_name.eq_ignore_ascii_case("AGENTS.md") || file_name.eq_ignore_ascii_case("CLAUDE.md") {
        let display = abs_path
            .strip_prefix(cwd)
            .unwrap_or(&abs_path)
            .to_string_lossy()
            .to_string();
        return Some((CompactReadKind::Resource, display));
    }

    // SKILL.md → skill with parent directory name
    if file_name == "SKILL.md"
        && let Some(parent) = abs_path.parent()
        && let Some(dir_name) = parent.file_name()
    {
        let dir_name = dir_name.to_str().unwrap_or("unknown");
        return Some((CompactReadKind::Skill, dir_name.to_string()));
    }

    None
}

// ── Truncation ──────────────────────────────────────────────────

/// Truncation result, mirroring pi's `TruncationResult`.
struct TruncationResult {
    content: String,
    truncated: bool,
    truncated_by: Option<&'static str>, // None | "lines" | "bytes"
    output_lines: usize,
    total_lines: usize,
    first_line_exceeds_limit: bool,
}

/// Truncate content from the head, keeping complete lines that fit within limits.
/// Never returns partial lines. If first line exceeds the byte limit,
/// returns empty content with `first_line_exceeds_limit = true`.
fn truncate_head(content: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let total_bytes = content.len();
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Check if no truncation needed
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: None,
            output_lines: total_lines,
            total_lines,
            first_line_exceeds_limit: false,
        };
    }

    // Check if first line alone exceeds the byte limit
    if let Some(first) = lines.first()
        && first.len() > max_bytes
    {
        return TruncationResult {
            content: String::new(),
            truncated: true,
            truncated_by: Some("bytes"),
            output_lines: 0,
            total_lines,
            first_line_exceeds_limit: true,
        };
    }

    // Accumulate complete lines that fit within both limits
    let mut output: Vec<&str> = Vec::new();
    let mut byte_count: usize = 0;
    let mut truncated_by = "lines";

    for line in lines.iter().take(max_lines) {
        let line_bytes = line.len();
        let with_newline = if output.is_empty() {
            line_bytes
        } else {
            line_bytes + 1 // +1 for the preceding newline
        };

        if byte_count + with_newline > max_bytes {
            truncated_by = "bytes";
            break;
        }

        output.push(line);
        byte_count += with_newline;
    }

    if output.len() >= max_lines && byte_count <= max_bytes {
        truncated_by = "lines";
    }

    TruncationResult {
        content: output.join("\n"),
        truncated: true,
        truncated_by: Some(truncated_by),
        output_lines: output.len(),
        total_lines,
        first_line_exceeds_limit: false,
    }
}

// ── AgentTool implementation ─────────────────────────────────────

#[async_trait]
impl AgentTool for ReadTool {
    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
        cancel: Cancel,
        _on_update: Option<UnboundedSender<ToolOutput>>,
    ) -> anyhow::Result<ToolOutput> {
        let _ = tool_call_id;
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;
        let offset = args["offset"].as_u64().map(|o| o as usize).unwrap_or(0);
        let limit = args["limit"].as_u64().map(|l| l as usize);

        let abs_path = resolve_read_path(path, &self.cwd);

        cancel.check()?;

        // Check if the file is an image (magic byte detection, matching pi)
        if let Ok(Some(mime)) = detect_image_mime(&abs_path) {
            // Image file detected — read binary data and include as base64 data URL
            let file_name = abs_path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
                .to_string();
            let file_len = std::fs::metadata(&abs_path)
                .ok()
                .map(|m| m.len() as usize)
                .unwrap_or(0);
            let binary = std::fs::read(&abs_path)
                .with_context(|| format!("Failed to read image {}", abs_path.display()))?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&binary);
            let data_url = format!("data:{};base64,{}", mime, b64);
            let msg = format!(
                "Read image file [{}] - {} ({})\n{}",
                mime,
                file_name,
                format_size(file_len),
                data_url,
            );
            return Ok(ToolOutput {
                content: msg,
                compact: None,
                is_error: false,
                terminate: false,
                details: Some(serde_json::json!({
                    "mimeType": mime,
                    "fileName": file_name,
                    "fileSize": file_len,
                    "imageData": b64,
                })),
            });
        }

        let content = std::fs::read_to_string(&abs_path)
            .with_context(|| format!("Failed to read {}", abs_path.display()))?;

        let all_lines: Vec<&str> = content.split('\n').collect();
        let total_file_lines = if content.ends_with('\n') {
            all_lines.len() - 1
        } else {
            all_lines.len()
        };

        // Apply offset (1-indexed → 0-indexed)
        let start_line = if offset > 0 { offset - 1 } else { 0 };
        if start_line >= total_file_lines {
            return Err(anyhow::anyhow!(
                "Offset {} is beyond end of file ({} lines total)",
                offset,
                total_file_lines
            ));
        }

        cancel.check()?;

        // Build the selected content based on offset/limit
        let selected_content: String;
        let user_limited_lines: Option<usize>;

        if let Some(lim) = limit {
            let end_line = (start_line + lim).min(total_file_lines);
            let selected_lines = &all_lines[start_line..end_line];
            selected_content = selected_lines.join("\n");
            user_limited_lines = Some(end_line - start_line);
        } else {
            let selected_lines = &all_lines[start_line..];
            selected_content = selected_lines.join("\n");
            user_limited_lines = None;
        }

        // Compute compact classification label for the legacy DisplayMsg path
        let compact =
            get_compact_read_classification(path, &self.cwd).map(|(kind, label)| match kind {
                CompactReadKind::Resource => format!("read resource {}", label),
                CompactReadKind::Skill => format!("read skill {}", label),
            });

        // Apply truncation
        let trunc = truncate_head(&selected_content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);

        if trunc.first_line_exceeds_limit {
            let first_line_bytes = format_size(all_lines[start_line].len());
            let msg = format!(
                "[Line {} is {}, exceeds {} limit. Use bash: sed -n '{}p' {} | head -c {}]",
                start_line + 1,
                first_line_bytes,
                format_size(DEFAULT_MAX_BYTES),
                start_line + 1,
                path,
                DEFAULT_MAX_BYTES,
            );
            let details = Some(serde_json::json!({
                "truncation": {
                    "truncated": true,
                    "truncatedBy": "bytes",
                    "totalLines": trunc.total_lines,
                    "outputLines": 0,
                    "firstLineExceedsLimit": true,
                    "maxLines": DEFAULT_MAX_LINES,
                    "maxBytes": DEFAULT_MAX_BYTES,
                }
            }));
            if let Some(label) = compact {
                return Ok(ToolOutput {
                    content: msg,
                    compact: Some(label),
                    is_error: false,
                    terminate: false,
                    details,
                });
            } else {
                return Ok(ToolOutput {
                    content: msg,
                    compact: None,
                    is_error: false,
                    terminate: false,
                    details,
                });
            }
        }

        let output: String;
        let mut details: Option<serde_json::Value> = None;

        if trunc.truncated {
            let start_display = start_line + 1;
            let end_display = start_display + trunc.output_lines - 1;
            let next_offset = end_display + 1;

            if trunc.truncated_by == Some("lines") {
                output = format!(
                    "{}\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                    trunc.content, start_display, end_display, total_file_lines, next_offset,
                );
            } else {
                output = format!(
                    "{}\n\n[Showing lines {}-{} of {} ({} limit). Use offset={} to continue.]",
                    trunc.content,
                    start_display,
                    end_display,
                    total_file_lines,
                    format_size(DEFAULT_MAX_BYTES),
                    next_offset,
                );
            }
            details = Some(serde_json::json!({
                "truncation": {
                    "truncated": true,
                    "truncatedBy": trunc.truncated_by,
                    "totalLines": trunc.total_lines,
                    "outputLines": trunc.output_lines,
                    "firstLineExceedsLimit": false,
                    "maxLines": DEFAULT_MAX_LINES,
                    "maxBytes": DEFAULT_MAX_BYTES,
                }
            }));
        } else if let Some(ul) = user_limited_lines {
            if start_line + ul < total_file_lines {
                let remaining = total_file_lines - (start_line + ul);
                let next_offset = start_line + ul + 1;
                output = format!(
                    "{}\n\n[{} more lines in file. Use offset={} to continue.]",
                    trunc.content, remaining, next_offset,
                );
            } else {
                let lines: Vec<&str> = trunc.content.lines().collect();
                let trimmed = trim_trailing_empty_lines(&lines);
                output = trimmed.join("\n");
            }
        } else {
            let lines: Vec<&str> = trunc.content.lines().collect();
            let trimmed = trim_trailing_empty_lines(&lines);
            output = trimmed.join("\n");
        }

        if let Some(label) = compact {
            Ok(ToolOutput {
                content: output,
                compact: Some(label),
                is_error: false,
                terminate: false,
                details,
            })
        } else {
            Ok(ToolOutput {
                content: output,
                compact: None,
                is_error: false,
                terminate: false,
                details,
            })
        }
    }
}

#[async_trait::async_trait]
impl yoagent::types::AgentTool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }
    fn label(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "Read the contents of a file. For text files, output is truncated to 2000 lines or \
         50KB (whichever is hit first). Use offset/limit for large files. When you need the \
         full file, continue with offset until complete."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (relative or absolute)"
                },
                "offset": {
                    "type": "number",
                    "description": "Line number to start reading from (1-indexed)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of lines to read"
                }
            }
        })
    }
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: yoagent::types::ToolContext,
    ) -> std::result::Result<yoagent::types::ToolResult, yoagent::types::ToolError> {
        crate::agent::extension::execute_via_rab_tool(self, params, ctx).await
    }
}

/// Tool renderer for the `read` tool.
/// Formats call headers with compact labels and result content with syntax highlighting.
struct ReadRenderer {
    cwd: std::path::PathBuf,
}

impl ToolRenderer for ReadRenderer {
    fn render_call(
        &self,
        args: &serde_json::Value,
        _width: usize,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Vec<String> {
        use std::path::Path;
        let path = args
            .get("file_path")
            .or_else(|| args.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
        let limit = args.get("limit").and_then(|v| v.as_u64());

        // Compute compact classification
        let classification = if !ctx.expanded {
            get_compact_read_classification(path, Path::new(&self.cwd))
        } else {
            None
        };

        // Format line range (matching pi's formatReadLineRange)
        let range = if offset > 0 || limit.is_some() {
            let start = if offset > 0 { offset } else { 1 };
            let range_str = match limit {
                Some(l) => format!(":{}-{}", start, start + l - 1),
                None => format!(":{}", start),
            };
            theme.fg_key(ThemeKey::Warning, &range_str)
        } else {
            String::new()
        };

        // Expand hint (matching pi's compact read: `theme.fg("dim", " (${keyText} to expand)")`)
        let expand_hint = if !ctx.expanded && !ctx.expand_key.is_empty() {
            theme.fg_key(ThemeKey::Dim, &format!(" ({} to expand)", ctx.expand_key))
        } else {
            String::new()
        };

        if let Some((kind, label)) = classification {
            match kind {
                CompactReadKind::Skill => {
                    // Pi: `[skill] name:range (Ctrl+O to expand)`
                    // [skill] in customMessageLabel bold, name in customMessageText
                    let prefix =
                        theme.fg_key(ThemeKey::CustomMessageLabel, "\x1b[1m[skill]\x1b[22m ");
                    let name = theme.fg_key(ThemeKey::CustomMessageText, &label);
                    vec![format!("{}{}{}{}", prefix, name, range, expand_hint)]
                }
                CompactReadKind::Resource => {
                    // Pi: `read resource  path:range (Ctrl+O to expand)`
                    // "read resource" in bold toolTitle, path in accent
                    let title_styled =
                        theme.fg_key(ThemeKey::ToolTitle, &theme.bold("read resource"));
                    let path_styled = theme.fg_key(ThemeKey::Accent, &label);
                    vec![format!(
                        "{} {}{}{}",
                        title_styled, path_styled, range, expand_hint
                    )]
                }
            }
        } else {
            // Regular call: `read  path:range`
            let short = if let Ok(home) = std::env::var("HOME") {
                path.replacen(&home, "~", 1)
            } else {
                path.to_string()
            };
            let path_disp = if short.is_empty() {
                String::new()
            } else {
                theme.fg_key(ThemeKey::Accent, &short)
            };
            vec![format!(
                "{} {}{}",
                theme.fg_key(ThemeKey::ToolTitle, &theme.bold("read")),
                path_disp,
                range,
            )]
        }
    }

    fn render_result(
        &self,
        content: &str,
        _width: usize,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Vec<String> {
        if content.is_empty() {
            return vec![];
        }

        // Pi: return empty when collapsed and not error (result is hidden until expanded)
        if !ctx.expanded && !ctx.is_error {
            return vec![];
        }

        // If this is an image read, show image inline (Kitty protocol) or text fallback
        if let Some(ref details) = ctx.details
            && let Some(mime) = details.get("mimeType").and_then(|v| v.as_str())
        {
            let file_name = details
                .get("fileName")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let file_size = details
                .get("fileSize")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let size_str = format_size(file_size as usize);

            // Try Kitty protocol image display (inline)
            if crate::tui::components::markdown::kitty_images_supported()
                && let Some(b64) = details.get("imageData").and_then(|v| v.as_str())
                && let Ok(binary) = crate::builtin::base64_decode(b64)
            {
                let kitty_seq =
                    crate::tui::components::markdown::kitty_image_sequence(&binary, mime);
                return vec![
                    String::new(),
                    kitty_seq,
                    theme.fg_key(
                        ThemeKey::ToolOutput,
                        &format!("Read image file [{}] - {} ({})", mime, file_name, size_str),
                    ),
                ];
            }

            // Fallback: text summary
            return vec![
                String::new(),
                theme.fg_key(ThemeKey::ToolOutput, &format!("Read image file [{}]", mime)),
                theme.fg_key(ThemeKey::ToolOutput, &format!("  File: {}", file_name)),
                theme.fg_key(ThemeKey::ToolOutput, &format!("  Size: {}", size_str)),
            ];
        }

        let path = ctx.file_path.as_deref().unwrap_or("");
        let lang = if !path.is_empty() {
            crate::tui::components::path_to_language(path)
        } else {
            None
        };

        // Pi: trim trailing empty lines from the full content
        let all_lines: Vec<&str> = content.lines().collect();
        let mut end = all_lines.len();
        while end > 0 && all_lines[end - 1].is_empty() {
            end -= 1;
        }
        let trimmed_lines = &all_lines[..end];

        // Pi: show up to 10 lines when collapsed, full when expanded
        let max_lines = if ctx.expanded { usize::MAX } else { 10 };
        let display_lines: Vec<&str> = trimmed_lines.iter().copied().take(max_lines).collect();
        let remaining = trimmed_lines.len().saturating_sub(display_lines.len());

        // Pi: start with blank line (`\n` before content)
        let mut result = vec![String::new()];

        // Pi: apply syntax highlighting when a language is detected (syntect feature)
        #[cfg(feature = "syntect")]
        {
            if let Some(lang) = lang {
                let combined = display_lines.join("\n");
                let highlighted = crate::tui::components::highlight_code(&combined, Some(lang));
                for line in highlighted {
                    result.push(line.replace('\t', "   "));
                }
            } else {
                for line in &display_lines {
                    let processed = line.replace('\t', "   ");
                    result.push(theme.fg_key(ThemeKey::ToolOutput, &processed));
                }
            }
        }

        #[cfg(not(feature = "syntect"))]
        for line in &display_lines {
            let processed = line.replace('\t', "   ");
            result.push(theme.fg_key(ThemeKey::ToolOutput, &processed));
        }

        // Pi: remaining lines hint
        if remaining > 0 {
            let hint = if !ctx.expand_key.is_empty() {
                format!(
                    "... ({} more lines, {} to expand)",
                    remaining, ctx.expand_key
                )
            } else {
                format!("... ({} more lines)", remaining)
            };
            result.push(theme.fg_key(ThemeKey::Muted, &hint));
        }

        // Pi: truncation warnings from details (matching pi's formatReadResult)
        if let Some(ref details) = ctx.details
            && let Some(truncation) = details.get("truncation")
        {
            let truncated = truncation
                .get("truncated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if truncated {
                let warning_color = |s: String| theme.fg_key(ThemeKey::Warning, &s);
                let first_line_exceeds = truncation
                    .get("firstLineExceedsLimit")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if first_line_exceeds {
                    let max_bytes = truncation
                        .get("maxBytes")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(DEFAULT_MAX_BYTES as u64)
                        as usize;
                    result.push(warning_color(format!(
                        "[First line exceeds {} limit]",
                        format_size(max_bytes),
                    )));
                } else if let Some(truncated_by) =
                    truncation.get("truncatedBy").and_then(|v| v.as_str())
                {
                    let output_lines = truncation
                        .get("outputLines")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let total_lines = truncation
                        .get("totalLines")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    if truncated_by == "lines" {
                        let max_lines = truncation
                            .get("maxLines")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(DEFAULT_MAX_LINES as u64)
                            as usize;
                        result.push(warning_color(format!(
                            "[Truncated: showing {} of {} lines ({} line limit)]",
                            output_lines, total_lines, max_lines,
                        )));
                    } else {
                        let max_bytes = truncation
                            .get("maxBytes")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(DEFAULT_MAX_BYTES as u64)
                            as usize;
                        result.push(warning_color(format!(
                            "[Truncated: {} lines shown ({} limit)]",
                            output_lines,
                            format_size(max_bytes),
                        )));
                    }
                }
            }
        }

        result
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rab-read-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_tool() -> (ReadTool, std::path::PathBuf) {
        let tmp = tmp_dir();
        (ReadTool { cwd: tmp.clone() }, tmp)
    }

    async fn exec_ok(tool: &ReadTool, args: serde_json::Value) -> String {
        tool.execute("id".into(), args, Cancel::new(), None)
            .await
            .unwrap()
            .content
    }

    async fn exec_full(tool: &ReadTool, args: serde_json::Value) -> ToolOutput {
        tool.execute("id".into(), args, Cancel::new(), None)
            .await
            .unwrap()
    }

    // ── Truncation unit tests ─────────────────────────────────

    #[test]
    fn test_no_truncation_needed() {
        let result = truncate_head("hello\nworld\n", 2000, 50000);
        assert!(!result.truncated);
        assert!(!result.first_line_exceeds_limit);
        assert_eq!(result.content, "hello\nworld\n");
    }

    #[test]
    fn test_truncates_by_lines() {
        let content: String = (1..=5000).map(|i| format!("line {}\n", i)).collect();
        let result = truncate_head(&content, 2000, 50000);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some("lines"));
        assert_eq!(result.output_lines, 2000);
        assert!(result.content.ends_with("line 2000"));
    }

    #[test]
    fn test_truncates_by_bytes() {
        let content: String = (1..=100)
            .map(|i| format!("line {} {}\n", i, "x".repeat(1000)))
            .collect();
        let result = truncate_head(&content, 2000, 50000);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some("bytes"));
        assert!(result.output_lines < 100);
    }

    #[test]
    fn test_first_line_exceeds_limit() {
        let content = format!("{}\nshort\n", "x".repeat(60000));
        let result = truncate_head(&content, 2000, 50000);
        assert!(result.truncated);
        assert!(result.first_line_exceeds_limit);
        assert!(result.content.is_empty());
    }

    #[test]
    fn test_empty_content() {
        let result = truncate_head("", 2000, 50000);
        assert!(!result.truncated);
        assert_eq!(result.content, "");
    }

    #[test]
    fn test_exact_fit() {
        let line = "a".repeat(50000);
        let result = truncate_head(&line, 2000, 50000);
        assert!(!result.truncated);
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500B");
        assert_eq!(format_size(1024), "1.0KB");
        assert_eq!(format_size(50 * 1024), "50.0KB");
        assert_eq!(format_size(1024 * 1024), "1.0MB");
    }

    #[test]
    fn test_trim_trailing_empty_lines() {
        let lines = vec!["a", "b", "", ""];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert_eq!(trimmed, &["a", "b"]);
    }

    #[test]
    fn test_trim_no_trailing_empty_lines() {
        let lines = vec!["a", "b"];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert_eq!(trimmed, &["a", "b"]);
    }

    #[test]
    fn test_trim_all_empty() {
        let lines: Vec<&str> = vec!["", "", ""];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert!(trimmed.is_empty());
    }

    #[test]
    fn test_trim_empty_input() {
        let lines: Vec<&str> = vec![];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert!(trimmed.is_empty());
    }

    // ── Compact classification tests ─────────────────────────

    #[test]
    fn test_compact_classification_agents_md() {
        let result = get_compact_read_classification("path/to/AGENTS.md", Path::new("path"));
        assert!(result.is_some());
        let (kind, label) = result.unwrap();
        assert_eq!(kind, CompactReadKind::Resource);
        assert!(label.contains("to/AGENTS.md"));
    }

    #[test]
    fn test_compact_classification_claude_md() {
        let result = get_compact_read_classification("CLAUDE.md", Path::new("path"));
        assert!(result.is_some());
        let (kind, label) = result.unwrap();
        assert_eq!(kind, CompactReadKind::Resource);
        assert_eq!(label, "CLAUDE.md");
    }

    #[test]
    fn test_compact_classification_skill() {
        let result = get_compact_read_classification("skills/my-skill/SKILL.md", Path::new("."));
        assert!(result.is_some());
        let (kind, label) = result.unwrap();
        assert_eq!(kind, CompactReadKind::Skill);
        assert_eq!(label, "my-skill");
    }

    #[test]
    fn test_compact_classification_regular_file() {
        let result = get_compact_read_classification("src/main.rs", Path::new("."));
        assert!(result.is_none());
    }

    // ── Integration tests ────────────────────────────────────
    #[tokio::test]
    async fn reads_file_content() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("test.txt");
        std::fs::write(&path, "hello world\nline two\n").unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(result.contains("hello world"));
        assert!(result.contains("line two"));
    }

    #[tokio::test]
    async fn read_respects_offset() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("test.txt");
        let content: Vec<String> = (1..=10).map(|i| format!("line {}", i)).collect();
        std::fs::write(&path, content.join("\n")).unwrap();

        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "offset": 5}),
        )
        .await;

        assert!(result.contains("line 5"), "should contain line 5: {result}");
        assert!(
            !result.lines().any(|l| l == "line 1"),
            "should not contain line 1: {result}"
        );
    }

    #[tokio::test]
    async fn read_respects_limit() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("test.txt");
        let content: Vec<String> = (1..=10).map(|i| format!("line {}", i)).collect();
        std::fs::write(&path, content.join("\n")).unwrap();

        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "offset": 1, "limit": 3}),
        )
        .await;

        assert!(result.contains("line 1"));
        assert!(result.contains("line 3"));
        assert!(!result.contains("line 4"));
    }

    #[tokio::test]
    async fn read_nonexistent_file_errors() {
        let (tool, _tmp) = make_tool();

        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"path": "nonexistent.txt"}),
                Cancel::new(),
                None,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn offset_beyond_end_errors() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("short.txt");
        std::fs::write(&path, "only one line\n").unwrap();

        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"path": path.to_str().unwrap(), "offset": 100}),
                Cancel::new(),
                None,
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("beyond end of file"));
    }

    #[tokio::test]
    async fn large_file_truncation_by_lines() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("large.txt");
        let content: String = (1..=5000).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(&path, &content).unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(result.contains("Showing lines 1-"));
        assert!(result.contains("offset="));
        assert!(result.contains("of 5000."));
    }

    #[tokio::test]
    async fn large_file_truncation_by_bytes() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("wide.txt");
        let content: String = (1..=100)
            .map(|i| format!("line {} {}\n", i, "x".repeat(1190)))
            .collect();
        std::fs::write(&path, &content).unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(result.contains("KB limit"));
        assert!(result.contains("offset="));
    }

    #[tokio::test]
    async fn first_line_exceeds_limit_shows_bash_hint() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("huge_first_line.txt");
        let content = format!("{}\nshort line\n", "x".repeat(60000));
        std::fs::write(&path, &content).unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(result.contains("bash"));
        assert!(result.contains("sed"));
        assert!(result.contains("head -c"));
    }

    #[tokio::test]
    async fn limit_honored_without_truncation() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("limited.txt");
        let content: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(&path, &content).unwrap();

        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "limit": 5}),
        )
        .await;

        assert!(result.contains("line 1"));
        assert!(result.contains("line 5"));
        assert!(!result.contains("line 6"));
        assert!(result.contains("more lines"));
    }

    #[tokio::test]
    async fn limit_exactly_covers_file() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("exact.txt");
        let content: String = (1..=3).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(&path, &content).unwrap();

        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "limit": 3}),
        )
        .await;

        assert!(result.contains("line 1"));
        assert!(result.contains("line 2"));
        assert!(result.contains("line 3"));
        assert!(!result.contains("more lines"));
    }

    #[tokio::test]
    async fn trims_trailing_empty_lines() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("trailing_empties.txt");
        std::fs::write(&path, "hello\nworld\n\n\n").unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(result.contains("hello"));
        assert!(result.contains("world"));
        assert!(!result.ends_with("\n\n\n"));
    }

    #[tokio::test]
    async fn relative_path_resolves_to_cwd() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("relative.txt");
        std::fs::write(&path, "hello\n").unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": "relative.txt"})).await;

        assert!(result.contains("hello"));
    }

    #[tokio::test]
    async fn compact_label_for_agents_md() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("AGENTS.md");
        std::fs::write(&path, "some instructions\n").unwrap();

        let output = exec_full(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(output.compact.is_some());
        let label = output.compact.unwrap();
        assert!(label.contains("read resource"));
        assert!(label.contains("AGENTS.md"));
    }

    #[tokio::test]
    async fn no_compact_label_for_regular_file() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();

        let output = exec_full(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(output.compact.is_none());
    }

    #[tokio::test]
    async fn cancel_aborts_read() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("cancel_test.txt");
        std::fs::write(&path, "hello\n").unwrap();

        let cancel = Cancel::new();
        cancel.cancel();

        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"path": path.to_str().unwrap()}),
                cancel,
                None,
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cancell") || err.contains("Cancel"));
    }
}
