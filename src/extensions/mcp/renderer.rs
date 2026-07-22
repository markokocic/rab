//! MCP tool rendering — mirrors pi-mcp-adapter's tool-result-renderer.ts.
//!
//! Each direct MCP tool gets a renderer that shows the tool name + args on the call
//! and truncated text output on the result. The proxy `mcp` tool gets its own renderer
//! that formats the operation type differently.

use crate::agent::default_renderer::{format_jsonish, has_useful_args, render_compact_result};
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
