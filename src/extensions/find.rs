use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use async_trait::async_trait;
use std::borrow::Cow;
use std::path::Path;
use tokio::sync::mpsc::UnboundedSender;

pub struct FindExtension {
    cwd: std::path::PathBuf,
}

impl FindExtension {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for FindExtension {
    fn name(&self) -> Cow<'static, str> {
        "find".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(FindTool {
            cwd: self.cwd.clone(),
        })]
    }
}

struct FindTool {
    cwd: std::path::PathBuf,
}

const DEFAULT_LIMIT: usize = 1000;

#[async_trait]
impl AgentTool for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        "Search for files by glob pattern. Returns matching file paths relative to the search directory. \
         Respects .gitignore. Output is truncated to 1000 results or 50KB (whichever is hit first)."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files, e.g. '*.rs', '**/*.json', or 'src/**/*.ts'"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of results (default: 1000)"
                }
            }
        })
    }

    fn renderer(&self) -> Option<Box<dyn ToolRenderer>> {
        Some(Box::new(FindRenderer))
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
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIMIT as u64) as usize;

        let search_path = if Path::new(path_str).is_absolute() {
            path_str.to_string()
        } else {
            self.cwd.join(path_str).to_string_lossy().to_string()
        };

        if !Path::new(&search_path).exists() {
            return Ok(ToolOutput::err(format!("Path not found: {}", search_path)));
        }

        // Try fd first, fall back to find
        let result = try_fd(pattern, &search_path, limit).await;

        match result {
            Ok(Some(output)) => Ok(output),
            Ok(None) => {
                // fd not available, fall back to find
                fallback_find(pattern, &search_path, limit).await
            }
            Err(e) => Ok(ToolOutput::err(e.to_string())),
        }
    }
}

/// Try to use `fd` for fast glob-based file search.
async fn try_fd(
    pattern: &str,
    search_path: &str,
    limit: usize,
) -> Result<Option<ToolOutput>, anyhow::Error> {
    let mut cmd = tokio::process::Command::new("fd");
    cmd.arg("--glob")
        .arg("--color=never")
        .arg("--hidden")
        .arg("--max-results")
        .arg(limit.to_string());

    // Check if inside a git repo; if not, add --no-require-git
    let git_dir = Path::new(search_path).join(".git");
    if !git_dir.exists() {
        cmd.arg("--no-require-git");
    }

    // Handle path-containing patterns
    let effective_pattern = if pattern.contains('/') {
        cmd.arg("--full-path");
        if !pattern.starts_with('/') && !pattern.starts_with("**/") && pattern != "**" {
            format!("**/{}", pattern)
        } else {
            pattern.to_string()
        }
    } else {
        pattern.to_string()
    };

    cmd.arg("--");
    cmd.arg(&effective_pattern);
    cmd.arg(search_path);

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = cmd.output().await;

    match output {
        Ok(out) => {
            if out.status.success() || (out.status.code() == Some(1) && out.stdout.is_empty()) {
                // fd exit code 1 means no results
                let stdout = String::from_utf8_lossy(&out.stdout);
                if stdout.trim().is_empty() {
                    return Ok(Some(ToolOutput::ok("No files found matching pattern")));
                }
                Ok(Some(format_find_output(&stdout, search_path, limit)))
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if stderr.contains("not found") || stderr.contains("No such file") {
                    // fd not installed, return None to trigger fallback
                    Ok(None)
                } else {
                    Ok(Some(ToolOutput::err(stderr.trim().to_string())))
                }
            }
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(None) // fd not installed, fallback
            } else {
                Err(anyhow::anyhow!("Failed to run fd: {}", e))
            }
        }
    }
}

/// Fallback to `find` command when fd is not available.
async fn fallback_find(
    pattern: &str,
    search_path: &str,
    limit: usize,
) -> Result<ToolOutput, anyhow::Error> {
    let mut cmd = tokio::process::Command::new("find");
    cmd.arg(search_path)
        .arg("-name")
        .arg(pattern)
        .arg("-type")
        .arg("f")
        .arg("-not")
        .arg("-path")
        .arg("*/node_modules/*")
        .arg("-not")
        .arg("-path")
        .arg("*/.git/*");

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = cmd.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Neither fd nor find are available. Please install fd: https://github.com/sharkdp/fd"
            )
        } else {
            anyhow::anyhow!("Failed to run find: {}", e)
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(ToolOutput::err(stderr.trim().to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Ok(ToolOutput::ok("No files found matching pattern"));
    }

    Ok(format_find_output(&stdout, search_path, limit))
}

/// Format find/fd output with relativized paths and truncation.
fn format_find_output(stdout: &str, search_path: &str, limit: usize) -> ToolOutput {
    let search_path = Path::new(search_path);

    let mut results: Vec<String> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let line = l.trim_end_matches('\r');
            let had_trailing_slash = line.ends_with('/') || line.ends_with('\\');
            let mut rel = if let Ok(relative) = Path::new(line).strip_prefix(search_path) {
                relative.to_string_lossy().to_string()
            } else {
                line.to_string()
            };
            // Normalize path separators
            rel = rel.replace('\\', "/");
            if had_trailing_slash && !rel.ends_with('/') {
                rel.push('/');
            }
            rel
        })
        .filter(|l| !l.is_empty())
        .collect();

    // Sort
    results.sort_by_key(|a| a.to_lowercase());

    let result_limit_reached = results.len() > limit;
    results.truncate(limit);

    let mut output = results.join("\n");
    let mut notices: Vec<String> = Vec::new();

    if result_limit_reached {
        notices.push(format!(
            "{} results limit reached. Use limit={} for more, or refine pattern",
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

    ToolOutput::ok(output)
}

// ── Renderer ────────────────────────────────────────────────────

pub struct FindRenderer;

impl Default for FindRenderer {
    fn default() -> Self {
        Self
    }
}

impl FindRenderer {
    pub fn new() -> Self {
        Self
    }
}

impl ToolRenderer for FindRenderer {
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
        let limit = args.get("limit").and_then(|v| v.as_u64());

        let mut text = format!(
            "{} {} {}",
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold("find")),
            theme.fg_key(ThemeKey::Accent, pattern),
            theme.fg_key(ThemeKey::ToolOutput, &format!("in {}", path)),
        );
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
