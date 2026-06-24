use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use async_trait::async_trait;
use std::borrow::Cow;
use std::path::Path;
use tokio::sync::mpsc::UnboundedSender;

pub struct LsExtension {
    cwd: std::path::PathBuf,
}

impl LsExtension {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for LsExtension {
    fn name(&self) -> Cow<'static, str> {
        "ls".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(LsTool {
            cwd: self.cwd.clone(),
        })]
    }
}

struct LsTool {
    cwd: std::path::PathBuf,
}

const DEFAULT_LIMIT: usize = 500;

#[async_trait]
impl AgentTool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        "List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. \
         Includes dotfiles. Output is truncated to 500 entries or 50KB (whichever is hit first)."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list (default: current directory)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of entries to return (default: 500)"
                }
            }
        })
    }

    fn renderer(&self) -> Option<Box<dyn ToolRenderer>> {
        Some(Box::new(LsRenderer))
    }

    async fn execute(
        &self,
        _tool_call_id: String,
        args: serde_json::Value,
        _cancel: Cancel,
        _on_update: Option<UnboundedSender<ToolOutput>>,
    ) -> anyhow::Result<ToolOutput> {
        let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIMIT as u64) as usize;

        let dir_path = if Path::new(path_str).is_absolute() {
            path_str.to_string()
        } else {
            self.cwd.join(path_str).to_string_lossy().to_string()
        };

        let dir = Path::new(&dir_path);
        if !dir.exists() {
            return Ok(ToolOutput::err(format!("Path not found: {}", dir_path)));
        }
        if !dir.is_dir() {
            return Ok(ToolOutput::err(format!("Not a directory: {}", dir_path)));
        }

        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    if is_dir { format!("{}/", name) } else { name }
                })
                .collect(),
            Err(e) => return Ok(ToolOutput::err(format!("Cannot read directory: {}", e))),
        };

        // Sort alphabetically, case-insensitive
        entries.sort_by_key(|a| a.to_lowercase());

        // Apply limit
        let entry_limit_reached = entries.len() > limit;
        entries.truncate(limit);

        if entries.is_empty() {
            return Ok(ToolOutput::ok("(empty directory)"));
        }

        let mut output = entries.join("\n");

        let mut notices: Vec<String> = Vec::new();
        if entry_limit_reached {
            notices.push(format!(
                "{} entries limit reached. Use limit={} for more",
                limit,
                limit * 2
            ));
        }

        // Apply byte truncation
        const MAX_BYTES: usize = 50 * 1024;
        if output.len() > MAX_BYTES {
            output.truncate(MAX_BYTES);
            notices.push("50KB limit reached".to_string());
        }

        if !notices.is_empty() {
            output.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }

        Ok(ToolOutput::ok(output))
    }
}

// ── Renderer ────────────────────────────────────────────────────

pub struct LsRenderer;

impl Default for LsRenderer {
    fn default() -> Self {
        Self
    }
}

impl LsRenderer {
    pub fn new() -> Self {
        Self
    }
}

impl ToolRenderer for LsRenderer {
    fn render_call(
        &self,
        args: &serde_json::Value,
        _width: usize,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Vec<String> {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let limit = args.get("limit").and_then(|v| v.as_u64());
        let mut text = format!(
            "{}{}",
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold("ls")),
            theme.fg_key(ThemeKey::ToolOutput, &format!(" {}", path)),
        );
        if let Some(l) = limit {
            text.push_str(&theme.fg_key(ThemeKey::Muted, &format!(" (limit {})", l)));
        }
        vec![text]
    }

    fn render_result(
        &self,
        content: &str,
        _width: usize,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let clean = content.trim_end();
        if clean.is_empty() {
            return lines;
        }

        let all_lines: Vec<&str> = clean.split('\n').collect();
        let preview_count = 20;
        let (preview, hidden) = if ctx.expanded {
            (all_lines.clone(), 0)
        } else {
            let take = preview_count.min(all_lines.len());
            (
                all_lines[..take].to_vec(),
                all_lines.len().saturating_sub(take),
            )
        };

        if hidden > 0 {
            let hint = if ctx.expand_key.is_empty() {
                theme.fg_key(ThemeKey::Muted, &format!("... {} earlier lines", hidden))
            } else {
                theme.fg(
                    "muted",
                    &format!(
                        "... ({} earlier lines, {} to expand)",
                        hidden, ctx.expand_key
                    ),
                )
            };
            lines.push(hint);
        }

        for line in &preview {
            if line.is_empty() {
                lines.push(String::new());
            } else {
                lines.push(theme.fg_key(ThemeKey::ToolOutput, line));
            }
        }

        lines
    }
}
