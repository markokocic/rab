//! MCP tool rendering — mirrors pi-mcp-adapter's tool-result-renderer.ts.
//!
//! Each direct MCP tool gets a renderer that shows the tool name + args on the call
//! and truncated text output on the result. The proxy `mcp` tool gets its own renderer
//! that formats the operation type differently.

use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Component;
use crate::tui::Theme;
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
        let title = theme.bold(&self.display_name);
        let header = theme.fg("toolTitle", &title);
        if !has_useful_args(args) {
            return std::boxed::Box::new(crate::tui::components::Text::new(header, 0, 0, None));
        }
        let args_str = format_jsonish(args);
        std::boxed::Box::new(crate::tui::components::Text::new(
            format!("{}\n{}", header, theme.fg("muted", &args_str)),
            0,
            0,
            None,
        ))
    }

    fn render_result(
        &self,
        content: &str,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Option<Box<dyn Component>> {
        if ctx.is_partial {
            return Some(std::boxed::Box::new(crate::tui::components::Text::new(
                theme.fg("warning", "Running MCP tool..."),
                0,
                0,
                None,
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
        return Some(std::boxed::Box::new(crate::tui::components::Text::new(
            theme.fg("muted", "(empty result)"),
            0,
            0,
            None,
        )));
    }

    let is_truncated = !ctx.is_error && !ctx.expanded && text_lines.len() > 3;
    let display_lines: &[&str] = if is_truncated {
        &text_lines[..3]
    } else {
        &text_lines[..]
    };

    let mut lines: Vec<String> = Vec::new();
    for line in display_lines {
        if line.is_empty() {
            lines.push(String::new());
        } else {
            lines.push(theme.fg("toolOutput", line));
        }
    }

    if is_truncated {
        lines.push(theme.fg("muted", "…"));
        let expand_key = if ctx.expand_key.is_empty() {
            "Ctrl+O"
        } else {
            &ctx.expand_key
        };
        lines.push(theme.fg("muted", &format!("({} to expand)", expand_key)));
    }

    Some(std::boxed::Box::new(crate::tui::components::Text::new(
        lines.join("\n"),
        0,
        0,
        None,
    )))
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
        let header = theme.fg("toolTitle", &theme.bold(&line));
        std::boxed::Box::new(crate::tui::components::Text::new(header, 0, 0, None))
    }

    fn render_result(
        &self,
        content: &str,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Option<Box<dyn Component>> {
        if ctx.is_partial {
            return Some(std::boxed::Box::new(crate::tui::components::Text::new(
                theme.fg("warning", "Running MCP tool..."),
                0,
                0,
                None,
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
