use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use async_trait::async_trait;
use std::borrow::Cow;
use std::path::Path;
use tokio::sync::mpsc::UnboundedSender;

pub struct GrepExtension {
    cwd: std::path::PathBuf,
}

impl GrepExtension {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for GrepExtension {
    fn name(&self) -> Cow<'static, str> {
        "grep".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(GrepTool {
            cwd: self.cwd.clone(),
        })]
    }
}

struct GrepTool {
    cwd: std::path::PathBuf,
}

const DEFAULT_LIMIT: usize = 100;
const GREP_MAX_LINE_LENGTH: usize = 2000;

#[async_trait]
impl AgentTool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents for a pattern. Returns matching lines with file paths and line numbers. \
         Respects .gitignore. Output is truncated to 100 matches or 50KB (whichever is hit first). \
         Long lines are truncated to 2000 chars."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (regex or literal string)"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search (default: current directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "Filter files by glob pattern, e.g. '*.rs' or 'src/**/*.ts'"
                },
                "ignoreCase": {
                    "type": "boolean",
                    "description": "Case-insensitive search (default: false)"
                },
                "literal": {
                    "type": "boolean",
                    "description": "Treat pattern as literal string instead of regex (default: false)"
                },
                "context": {
                    "type": "number",
                    "description": "Number of lines to show before and after each match (default: 0)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of matches to return (default: 100)"
                }
            }
        })
    }

    fn renderer(&self) -> Option<Box<dyn ToolRenderer>> {
        Some(Box::new(GrepRenderer))
    }

    async fn execute(
        &self,
        _tool_call_id: String,
        args: serde_json::Value,
        _cancel: Cancel,
        _on_update: Option<UnboundedSender<ToolOutput>>,
    ) -> anyhow::Result<ToolOutput> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' argument"))?;
        let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let glob = args.get("glob").and_then(|v| v.as_str());
        let ignore_case = args
            .get("ignoreCase")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let literal = args
            .get("literal")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let context = args.get("context").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIMIT as u64) as usize;

        let search_path = if Path::new(path_str).is_absolute() {
            path_str.to_string()
        } else {
            self.cwd.join(path_str).to_string_lossy().to_string()
        };

        // Build the rg command
        let mut cmd = tokio::process::Command::new("rg");
        cmd.arg("--json")
            .arg("--line-number")
            .arg("--color=never")
            .arg("--hidden")
            .arg("--max-count")
            .arg(limit.to_string());

        if ignore_case {
            cmd.arg("--ignore-case");
        }
        if literal {
            cmd.arg("--fixed-strings");
        }
        if let Some(g) = glob {
            cmd.arg("--glob");
            cmd.arg(g);
        }
        if context > 0 {
            cmd.arg("-C");
            cmd.arg(context.to_string());
        }

        cmd.arg("--");
        cmd.arg(pattern);
        cmd.arg(&search_path);

        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let output = cmd.output().await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "ripgrep (rg) is not installed. Please install it: https://github.com/BurntSushi/ripgrep"
                )
            } else {
                anyhow::anyhow!("Failed to run rg: {}", e)
            }
        })?;

        if !output.stderr.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // rg exits with code 2 on error, but let's check
            if !output.status.success() && output.stdout.is_empty() {
                return Ok(ToolOutput::err(stderr.trim().to_string()));
            }
        }

        if output.stdout.is_empty() {
            return Ok(ToolOutput::ok("No matches found"));
        }

        // Parse JSON output
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut result_lines: Vec<String> = Vec::new();
        let mut match_count = 0;
        let mut match_limit_reached = false;
        let mut lines_truncated = false;

        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if match_count >= limit {
                match_limit_reached = true;
                break;
            }
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
            match parsed {
                Ok(val) if val["type"] == "match" => {
                    match_count += 1;
                    let file_path = val["data"]["path"]["text"].as_str().unwrap_or("unknown");
                    let line_number = val["data"]["line_number"].as_u64().unwrap_or(0);
                    let line_text = val["data"]["lines"]["text"].as_str().unwrap_or("");

                    // Relativize file path
                    let rel_path = if file_path.starts_with(&search_path) {
                        &file_path[search_path.len() + 1..]
                    } else {
                        file_path
                    };

                    let sanitized = line_text
                        .replace("\r\n", "\n")
                        .replace('\r', "")
                        .trim_end_matches('\n')
                        .to_string();

                    // Truncate long lines
                    let (display_text, was_truncated) = truncate_line_text(&sanitized);
                    if was_truncated {
                        lines_truncated = true;
                    }

                    if context > 0 {
                        result_lines
                            .push(format!("{}:{}: {}", rel_path, line_number, display_text));
                    } else {
                        result_lines.push(format!("{}:{}:{}", rel_path, line_number, display_text));
                    }
                }
                Ok(val) if val["type"] == "summary" => {
                    // Skip summary lines
                }
                _ => {}
            }
        }

        if result_lines.is_empty() {
            return Ok(ToolOutput::ok("No matches found"));
        }

        let mut output_text = result_lines.join("\n");

        // Apply byte truncation
        const MAX_BYTES: usize = 50 * 1024;
        let mut notices: Vec<String> = Vec::new();
        if match_limit_reached {
            notices.push(format!(
                "{} matches limit reached. Use limit={} for more, or refine pattern",
                limit,
                limit * 2
            ));
        }
        if output_text.len() > MAX_BYTES {
            output_text.truncate(MAX_BYTES);
            notices.push("50KB limit reached".to_string());
        }
        if lines_truncated {
            notices.push(format!(
                "Some lines truncated to {} chars. Use read tool to see full lines",
                GREP_MAX_LINE_LENGTH
            ));
        }

        if !notices.is_empty() {
            output_text.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }

        Ok(ToolOutput::ok(output_text))
    }
}

/// Truncate a single line of text for grep output.
fn truncate_line_text(text: &str) -> (String, bool) {
    if text.len() > GREP_MAX_LINE_LENGTH {
        let mut truncated = text.chars().take(GREP_MAX_LINE_LENGTH).collect::<String>();
        truncated.push_str("...");
        (truncated, true)
    } else {
        (text.to_string(), false)
    }
}

// ── Renderer ────────────────────────────────────────────────────

pub struct GrepRenderer;

impl Default for GrepRenderer {
    fn default() -> Self {
        Self
    }
}

impl GrepRenderer {
    pub fn new() -> Self {
        Self
    }
}

impl ToolRenderer for GrepRenderer {
    fn render_call(
        &self,
        args: &serde_json::Value,
        _width: usize,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Vec<String> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("...");
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let glob = args.get("glob").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64());

        let mut text = format!(
            "{} /{}/ {}",
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold("grep")),
            theme.fg_key(ThemeKey::Accent, pattern),
            theme.fg_key(ThemeKey::ToolOutput, &format!("in {}", path)),
        );
        if let Some(g) = glob {
            text.push_str(&theme.fg_key(ThemeKey::ToolOutput, &format!(" ({})", g)));
        }
        if let Some(l) = limit {
            text.push_str(&theme.fg_key(ThemeKey::Muted, &format!(" limit {}", l)));
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
        let preview_count = 15;
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
                theme.fg_key(ThemeKey::Muted, &format!("... {} more lines", hidden))
            } else {
                theme.fg(
                    "muted",
                    &format!("... ({} more lines, {} to expand)", hidden, ctx.expand_key),
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
