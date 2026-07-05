//! MCP tool rendering — mirrors pi-mcp-adapter's tool-result-renderer.ts.
//!
//! Each direct MCP tool gets a renderer that shows the tool name + args on the call
//! and truncated text output on the result. The proxy `mcp` tool gets its own renderer
//! that formats the operation type differently.

use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Style;
use crate::tui::components::StyledSegment;
use crate::tui::{Component, Theme};
use serde_json::Value;

/// Renderer for individual direct MCP tools (matching pi's createMcpDirectToolCallRenderer).
pub struct McpToolRenderer {
    display_name: String,
}

impl McpToolRenderer {
    pub fn new(display_name: &str) -> Self {
        Self {
            display_name: display_name.to_string(),
        }
    }
}

impl ToolRenderer for McpToolRenderer {
    fn render_call(
        &self,
        args: &Value,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Box<dyn Component> {
        let title_style = Style::new()
            .fg(theme.fg_ansi("toolTitle").to_string())
            .bold();
        if !has_useful_args(args) {
            return std::boxed::Box::new(crate::tui::components::Text::new(
                self.display_name.clone(),
                0,
                0,
                Some(title_style),
            ));
        }
        let args_str = format_jsonish(args);
        let muted_style = Style::new().fg(theme.fg_ansi("muted").to_string());
        let segments = vec![
            StyledSegment {
                text: self.display_name.clone(),
                style: Some(title_style),
            },
            StyledSegment {
                text: format!("\n{}", args_str),
                style: Some(muted_style),
            },
        ];
        std::boxed::Box::new(crate::tui::components::Text::from_segments(
            segments, 0, 0, None,
        ))
    }

    fn render_result(
        &self,
        content: &str,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Option<Box<dyn Component>> {
        if ctx.is_partial {
            let warn_style = Style::new().fg(theme.fg_ansi("warning").to_string());
            return Some(std::boxed::Box::new(crate::tui::components::Text::new(
                "Running MCP tool...".to_string(),
                0,
                0,
                Some(warn_style),
            )));
        }
        render_compact_result(content, theme, ctx)
    }
}

/// Shared result rendering for both proxy and direct MCP tools.
/// Returns None when there's nothing to show.
fn render_compact_result(
    content: &str,
    theme: &dyn Theme,
    ctx: &ToolRenderContext,
) -> Option<Box<dyn Component>> {
    let text_lines: Vec<&str> = content.lines().collect();
    if text_lines.is_empty() || (text_lines.len() == 1 && text_lines[0].is_empty()) {
        let muted_style = Style::new().fg(theme.fg_ansi("muted").to_string());
        return Some(std::boxed::Box::new(crate::tui::components::Text::new(
            "(empty result)".to_string(),
            0,
            0,
            Some(muted_style),
        )));
    }

    let is_truncated = !ctx.is_error && !ctx.expanded && text_lines.len() > 3;
    let display_lines: &[&str] = if is_truncated {
        &text_lines[..3]
    } else {
        &text_lines[..]
    };

    let output_style = Style::new().fg(theme.fg_ansi("toolOutput").to_string());
    let muted_style = Style::new().fg(theme.fg_ansi("muted").to_string());
    let mut segments = Vec::new();

    for line in display_lines {
        if !line.is_empty() {
            segments.push(StyledSegment {
                text: line.to_string(),
                style: Some(output_style.clone()),
            });
        }
        segments.push(StyledSegment {
            text: "\n".to_string(),
            style: None,
        });
    }

    if is_truncated {
        segments.push(StyledSegment {
            text: "…".to_string(),
            style: Some(muted_style.clone()),
        });
        let expand_key = if ctx.expand_key.is_empty() {
            "Ctrl+O"
        } else {
            &ctx.expand_key
        };
        segments.push(StyledSegment {
            text: format!("({} to expand)", expand_key),
            style: Some(muted_style.clone()),
        });
    }

    Some(std::boxed::Box::new(
        crate::tui::components::Text::from_segments(segments, 0, 0, None),
    ))
}

/// Renderer for the proxy `mcp` tool (multi-purpose gateway, matching pi's renderMcpProxyToolCall).
pub struct McpProxyToolRenderer;

impl ToolRenderer for McpProxyToolRenderer {
    fn render_call(
        &self,
        args: &Value,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Box<dyn Component> {
        let line = format_mcp_proxy_call(args);
        let title_style = Style::new()
            .fg(theme.fg_ansi("toolTitle").to_string())
            .bold();
        std::boxed::Box::new(crate::tui::components::Text::new(
            line,
            0,
            0,
            Some(title_style),
        ))
    }

    fn render_result(
        &self,
        content: &str,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Option<Box<dyn Component>> {
        if ctx.is_partial {
            let warn_style = Style::new().fg(theme.fg_ansi("warning").to_string());
            return Some(std::boxed::Box::new(crate::tui::components::Text::new(
                "Running MCP tool...".to_string(),
                0,
                0,
                Some(warn_style),
            )));
        }
        render_compact_result(content, theme, ctx)
    }
}

/// Format the proxy tool call line based on which operation is being performed.
fn format_mcp_proxy_call(args: &Value) -> String {
    let obj = match args.as_object() {
        Some(o) => o,
        None => return "mcp status".to_string(),
    };

    // action mode
    if let Some(action) = obj.get("action").and_then(|v| v.as_str()) {
        return format!("mcp {}", action);
    }

    // tool call
    if let Some(tool) = obj.get("tool").and_then(|v| v.as_str()) {
        if let Some(server) = obj.get("server").and_then(|v| v.as_str()) {
            return format!("mcp call {} @ {}", tool, server);
        }
        return format!("mcp call {}", tool);
    }

    // connect
    if let Some(connect) = obj.get("connect").and_then(|v| v.as_str()) {
        return format!("mcp connect {}", connect);
    }

    // describe
    if let Some(describe) = obj.get("describe").and_then(|v| v.as_str()) {
        return format!("mcp describe {}", describe);
    }

    // search
    if let Some(search) = obj.get("search").and_then(|v| v.as_str()) {
        let mut line = format!("mcp search {}", search);
        if let Some(server) = obj.get("server").and_then(|v| v.as_str()) {
            line.push_str(&format!(" @ {}", server));
        }
        if obj.get("regex") == Some(&Value::Bool(true)) {
            line.push_str(" (regex)");
        }
        return line;
    }

    // list server
    if let Some(server) = obj.get("server").and_then(|v| v.as_str()) {
        return format!("mcp list {}", server);
    }

    "mcp status".to_string()
}

/// Check if args have useful (non-empty) content to display.
fn has_useful_args(args: &Value) -> bool {
    match args {
        Value::Object(m) => !m.is_empty(),
        _ => false,
    }
}

/// Format a value as pretty JSON for display (matching pi's formatJsonish).
fn format_jsonish(value: &Value) -> String {
    match value {
        Value::String(s) => {
            // Try to parse as JSON and re-pretty-print
            if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| s.clone())
            } else {
                truncate_text(s, 1500)
            }
        }
        other => {
            let s = serde_json::to_string_pretty(other).unwrap_or_default();
            truncate_text(&s, 1500)
        }
    }
}

/// Truncate text to max_chars with ellipsis.
fn truncate_text(value: &str, max_chars: usize) -> String {
    if value.len() <= max_chars {
        return value.to_string();
    }
    format!("{}…", &value[..max_chars.saturating_sub(1)])
}
