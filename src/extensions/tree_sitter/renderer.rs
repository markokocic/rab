//! Tree-sitter tool rendering — mirrors pi-tree-sitter's renderSymbolCall/renderSymbolResult pattern.
//!
//! Each semantic tool (list_symbols, find_definition, find_callers, get_symbol_body, find_callees)
//! gets the same call rendering (tool name + args) with a shared result rendering that shows
//! a compact summary (count + label + name) when collapsed and the full text when expanded.
//!
//! `get_symbol_body` gets a special result renderer that shows the body when expanded and
//! a summary (name + line count + path) when collapsed.

use crate::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::components::path_to_language;
use crate::tui::components::{StyledSegment, Text, highlight_code};
use crate::tui::{Component, Style, Theme};

/// Renderer for tree-sitter semantic tools.
///
/// Handles `list_symbols`, `find_definition`, `find_callers`, `find_callees` with
/// a shared rendering pattern, plus special handling for `get_symbol_body`.
pub struct TreeSitterToolRenderer {
    tool_name: &'static str,
}

impl TreeSitterToolRenderer {
    pub fn new(tool_name: &'static str) -> Self {
        Self { tool_name }
    }
}

impl ToolRenderer for TreeSitterToolRenderer {
    fn render_call(
        &self,
        args: &serde_json::Value,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Box<dyn Component> {
        let title_style = Style::new()
            .fg(theme.fg_ansi("toolTitle").to_string())
            .bold();

        // Build the call line: "toolTitle bold: toolName — accent: name  muted: in path  [dim: kind: ...]"
        let mut segments = vec![StyledSegment {
            text: self.tool_name.to_string(),
            style: Some(title_style),
        }];

        let obj = args.as_object();

        // name — accent
        if let Some(name) = obj.and_then(|o| o.get("name")).and_then(|v| v.as_str()) {
            let accent_style = Style::new().fg(theme.fg_ansi("accent").to_string());
            segments.push(StyledSegment {
                text: format!(" — {}", name),
                style: Some(accent_style),
            });
        }

        // path — muted
        if let Some(path) = obj.and_then(|o| o.get("path")).and_then(|v| v.as_str()) {
            let muted_style = Style::new().fg(theme.fg_ansi("muted").to_string());
            segments.push(StyledSegment {
                text: format!("  in {}", path),
                style: Some(muted_style),
            });
        }

        // kind — dim [bracketed]
        if let Some(kind) = obj.and_then(|o| o.get("kind")).and_then(|v| v.as_str()) {
            let dim_style = Style::new().fg(theme.fg_ansi("dim").to_string());
            segments.push(StyledSegment {
                text: format!("  [kind: {}]", kind),
                style: Some(dim_style),
            });
        }

        Box::new(Text::from_segments(segments, 0, 0, None))
    }

    fn render_result(
        &self,
        content: &str,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Option<Box<dyn Component>> {
        let details = ctx.details.as_ref();

        // ── get_symbol_body special rendering ──────────────────────
        if self.tool_name == "get_symbol_body" {
            return self.render_symbol_body_result(content, theme, ctx, details);
        }

        // ── General symbol tool result rendering ───────────────────
        let count = details
            .and_then(|d| d.get("count"))
            .and_then(|v| v.as_i64());
        let label = details
            .and_then(|d| d.get("label"))
            .and_then(|v| v.as_str());

        // No structured details — fall back to raw content text (e.g. error messages)
        if count.is_none() {
            return Some(Box::new(Text::new(content.to_string(), 0, 0, None)));
        }

        // Expanded — show full content
        if ctx.expanded {
            return Some(Box::new(Text::new(content.to_string(), 0, 0, None)));
        }

        let count = count.unwrap();
        let label = label.unwrap_or("");
        let name = details.and_then(|d| d.get("name")).and_then(|v| v.as_str());
        let file_count = details
            .and_then(|d| d.get("fileCount"))
            .and_then(|v| v.as_i64());

        // No results — dim
        if count == 0 {
            let dim_style = Style::new().fg(theme.fg_ansi("dim").to_string());
            if let Some(name) = name {
                let accent_style = Style::new().fg(theme.fg_ansi("accent").to_string());
                return Some(Box::new(Text::from_segments(
                    vec![
                        StyledSegment {
                            text: format!("No {} found", label),
                            style: Some(dim_style),
                        },
                        StyledSegment {
                            text: format!(" for '{}'", name),
                            style: Some(accent_style),
                        },
                    ],
                    0,
                    0,
                    None,
                )));
            }
            return Some(Box::new(Text::new(
                format!("No {} found", label),
                0,
                0,
                Some(dim_style),
            )));
        }

        // Results — success ✓
        let success_style = Style::new().fg(theme.fg_ansi("success").to_string());
        let accent_style = Style::new().fg(theme.fg_ansi("accent").to_string());
        let muted_style = Style::new().fg(theme.fg_ansi("muted").to_string());

        let mut segments = vec![StyledSegment {
            text: format!("\u{2713} {} {}", count, label),
            style: Some(success_style),
        }];

        if let Some(name) = name {
            segments.push(StyledSegment {
                text: format!(" for '{}'", name),
                style: Some(accent_style),
            });
        }

        if let Some(fc) = file_count {
            let plural = if fc == 1 { "" } else { "s" };
            segments.push(StyledSegment {
                text: format!(" across {} file{}", fc, plural),
                style: Some(muted_style),
            });
        }

        Some(Box::new(Text::from_segments(segments, 0, 0, None)))
    }
}

impl TreeSitterToolRenderer {
    /// Highlight and render the expanded symbol body.
    fn render_symbol_body_expanded(&self, details: &serde_json::Value) -> Box<dyn Component> {
        let body = details.get("body").and_then(|v| v.as_str()).unwrap_or("");
        let path = details.get("path").and_then(|v| v.as_str()).unwrap_or("");

        // Determine language: prefer path_to_language (proper syntect tokens),
        // fall back to details.language (raw extension).
        let lang = path_to_language(path).or_else(|| {
            details
                .get("language")
                .and_then(|v| v.as_str())
                .filter(|l| !l.is_empty())
        });

        let highlighted = highlight_code(body, lang);
        if !highlighted.is_empty() && highlighted.iter().any(|l| !l.is_empty()) {
            let segments: Vec<StyledSegment> = highlighted
                .into_iter()
                .map(|line| StyledSegment {
                    text: if line.is_empty() {
                        line
                    } else {
                        format!("{}\n", line)
                    },
                    style: None, // ANSI codes are embedded by syntect
                })
                .collect();
            return Box::new(Text::from_segments(segments, 0, 0, None));
        }

        // Fallback: no syntect or empty result — show raw body
        Box::new(Text::new(body.to_string(), 0, 0, None))
    }

    /// Special result rendering for `get_symbol_body` — mirrors pi-tree-sitter's
    /// `get_symbol_body` renderResult which shows highlighted body on expand and
    /// a summary (name + line count + path) when collapsed.
    fn render_symbol_body_result(
        &self,
        content: &str,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
        details: Option<&serde_json::Value>,
    ) -> Option<Box<dyn Component>> {
        let details = details?;

        // Error or missing body — show error text
        if ctx.is_error || details.get("body").is_none() {
            let error_style = Style::new().fg(theme.fg_ansi("error").to_string());
            return Some(Box::new(Text::new(
                content.to_string(),
                0,
                0,
                Some(error_style),
            )));
        }

        let name = details.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let line_count = details
            .get("lineCount")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let path = details.get("path").and_then(|v| v.as_str()).unwrap_or("");

        // Expanded — show highlighted body
        if ctx.expanded {
            return Some(self.render_symbol_body_expanded(details));
        }

        // Collapsed — show "✓ name (N lines) in path"
        let success_style = Style::new().fg(theme.fg_ansi("success").to_string());
        let accent_style = Style::new().fg(theme.fg_ansi("accent").to_string());
        let dim_style = Style::new().fg(theme.fg_ansi("dim").to_string());
        let muted_style = Style::new().fg(theme.fg_ansi("muted").to_string());

        Some(Box::new(Text::from_segments(
            vec![
                StyledSegment {
                    text: "\u{2713} ".to_string(),
                    style: Some(success_style),
                },
                StyledSegment {
                    text: name.to_string(),
                    style: Some(accent_style),
                },
                StyledSegment {
                    text: format!(" ({} lines) in ", line_count),
                    style: Some(dim_style),
                },
                StyledSegment {
                    text: path.to_string(),
                    style: Some(muted_style),
                },
            ],
            0,
            0,
            None,
        )))
    }
}
