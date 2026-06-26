use crate::agent::extension::{Extension, ToolRenderContext, ToolRenderer, ToolWithMeta};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use async_trait::async_trait;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// Combined filesystem extension providing grep, find, and ls tools.
pub struct FilesystemExtension {
    cwd: PathBuf,
}

impl FilesystemExtension {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for FilesystemExtension {
    fn name(&self) -> Cow<'static, str> {
        "filesystem".into()
    }

    fn tools(&self) -> Vec<ToolWithMeta> {
        vec![
            ToolWithMeta {
                tool: Box::new(GrepTool {
                    cwd: self.cwd.clone(),
                }),
                snippet: "Search file contents for patterns (respects .gitignore)",
                guidelines: &["Use grep for searching file contents with patterns"],
                prepare_arguments: None,
            },
            ToolWithMeta {
                tool: Box::new(FindTool {
                    cwd: self.cwd.clone(),
                }),
                snippet: "Find files by glob pattern (respects .gitignore)",
                guidelines: &["Use find for locating files by pattern"],
                prepare_arguments: None,
            },
            ToolWithMeta {
                tool: Box::new(LsTool {
                    cwd: self.cwd.clone(),
                }),
                snippet: "List directory contents",
                guidelines: &["Use ls for exploring directory structure"],
                prepare_arguments: None,
            },
        ]
    }

    fn tool_renderer(&self, name: &str) -> Option<Box<dyn ToolRenderer>> {
        match name {
            "grep" => Some(Box::new(GrepRenderer {})),
            "find" => Some(Box::new(FindRenderer {})),
            "ls" => Some(Box::new(LsRenderer {})),
            _ => None,
        }
    }
}

// ── Constants ────────────────────────────────────────────────────

const GREP_DEFAULT_LIMIT: u64 = 100;
const GREP_MAX_LINE_LENGTH: usize = 500;
const FIND_DEFAULT_LIMIT: u64 = 1000;
const LS_DEFAULT_LIMIT: u64 = 500;

// =====================================================================
// grep tool
// =====================================================================

struct GrepTool {
    cwd: PathBuf,
}

#[async_trait]
impl yoagent::types::AgentTool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    fn label(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search file contents for a pattern. Returns matching lines with file paths and line numbers. \
         Respects .gitignore. Output is truncated to 100 matches. \
         Long lines are truncated to 500 chars."
    }
    fn parameters_schema(&self) -> serde_json::Value {
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
                    "description": "Filter files by glob pattern, e.g. '*.rs' or '**/*.spec.rs'"
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

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: yoagent::types::ToolContext,
    ) -> Result<yoagent::types::ToolResult, yoagent::types::ToolError> {
        let pattern = params["pattern"].as_str().ok_or_else(|| {
            yoagent::types::ToolError::InvalidArgs("Missing 'pattern' argument".into())
        })?;
        let search_path = params["path"].as_str().unwrap_or(".");
        let glob = params["glob"].as_str();
        let ignore_case = params["ignoreCase"].as_bool().unwrap_or(false);
        let literal = params["literal"].as_bool().unwrap_or(false);
        let context = params["context"].as_u64().unwrap_or(0);
        let limit = params["limit"].as_u64().unwrap_or(GREP_DEFAULT_LIMIT);

        let abs_search = resolve_path(search_path, &self.cwd);

        if !abs_search.exists() {
            return Err(yoagent::types::ToolError::Failed(format!(
                "Path not found: {}",
                abs_search.display()
            )));
        }

        if ctx.cancel.is_cancelled() {
            return Err(yoagent::types::ToolError::Cancelled);
        }

        // Try ripgrep first, fall back to grep
        let output = if let Some(rg) = which("rg") {
            run_rg(
                &rg,
                pattern,
                &abs_search,
                glob,
                ignore_case,
                literal,
                context,
                limit,
            )
            .await?
        } else {
            run_grep(pattern, &abs_search, ignore_case, literal, context, limit).await?
        };

        if ctx.cancel.is_cancelled() {
            return Err(yoagent::types::ToolError::Cancelled);
        }

        Ok(yoagent::types::ToolResult {
            content: vec![yoagent::types::Content::Text { text: output }],
            details: serde_json::Value::Null,
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_rg(
    rg: &Path,
    pattern: &str,
    search_path: &Path,
    glob: Option<&str>,
    ignore_case: bool,
    literal: bool,
    context: u64,
    limit: u64,
) -> Result<String, yoagent::types::ToolError> {
    let mut cmd = tokio::process::Command::new(rg);
    cmd.args(["--json", "--line-number", "--color=never", "--hidden"]);
    if ignore_case {
        cmd.arg("--ignore-case");
    }
    if literal {
        cmd.arg("--fixed-strings");
    }
    if let Some(g) = glob {
        cmd.args(["--glob", g]);
    }
    if context > 0 {
        cmd.arg("-C").arg(context.to_string());
    }
    cmd.arg("--max-count").arg(limit.to_string());
    cmd.arg("--").arg(pattern).arg(search_path);

    let output = cmd
        .output()
        .await
        .map_err(|e| yoagent::types::ToolError::Failed(format!("Failed to run rg: {}", e)))?;

    let exit_code = output.status.code().unwrap_or(-1);
    if exit_code == 2 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(yoagent::types::ToolError::Failed(format!(
            "ripgrep error: {}",
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results: Vec<String> = Vec::new();
    let mut line_count = 0u64;

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line_count >= limit {
            break;
        }

        if let Ok(event) = serde_json::from_str::<serde_json::Value>(line)
            && event["type"] == "match"
            && let (Some(file_path), Some(line_number), Some(line_text)) = (
                event["data"]["path"]["text"].as_str(),
                event["data"]["line_number"].as_u64(),
                event["data"]["lines"]["text"].as_str(),
            )
        {
            let relative = relativize_path(file_path, search_path);
            let sanitized = line_text
                .replace('\r', "")
                .trim_end_matches('\n')
                .to_string();
            results.push(format!(
                "{}:{}: {}",
                relative,
                line_number,
                truncate_line(&sanitized, GREP_MAX_LINE_LENGTH)
            ));
            line_count += 1;
        }
    }

    if results.is_empty() {
        return Ok("No matches found".to_string());
    }

    Ok(results.join("\n"))
}

async fn run_grep(
    pattern: &str,
    search_path: &Path,
    ignore_case: bool,
    literal: bool,
    context: u64,
    limit: u64,
) -> Result<String, yoagent::types::ToolError> {
    let mut cmd = tokio::process::Command::new("grep");
    cmd.args([
        "--line-number",
        "--color=never",
        "--binary-files=without-match",
    ]);
    if ignore_case {
        cmd.arg("-i");
    }
    if literal {
        cmd.arg("-F");
    }
    if context > 0 {
        cmd.arg("-C").arg(context.to_string());
    }
    cmd.arg("--max-count").arg(limit.to_string());
    cmd.arg("-r").arg("--").arg(pattern).arg(search_path);

    let output = cmd
        .output()
        .await
        .map_err(|e| yoagent::types::ToolError::Failed(format!("Failed to run grep: {}", e)))?;

    let exit_code = output.status.code().unwrap_or(-1);
    if exit_code == 2 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(yoagent::types::ToolError::Failed(format!(
            "grep error: {}",
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok("No matches found".to_string());
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    let truncated: Vec<String> = lines
        .iter()
        .take(limit as usize)
        .map(|l| truncate_line(l, GREP_MAX_LINE_LENGTH))
        .collect();

    Ok(truncated.join("\n"))
}

// =====================================================================
// find tool
// =====================================================================

struct FindTool {
    cwd: PathBuf,
}

#[async_trait]
impl yoagent::types::AgentTool for FindTool {
    fn name(&self) -> &str {
        "find"
    }
    fn label(&self) -> &str {
        "find"
    }
    fn description(&self) -> &str {
        "Search for files by glob pattern. Returns matching file paths relative to the search directory. \
         Respects .gitignore. Output is truncated to 1000 results."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files, e.g. '*.rs', '**/*.json', or 'src/**/*.spec.rs'"
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

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: yoagent::types::ToolContext,
    ) -> Result<yoagent::types::ToolResult, yoagent::types::ToolError> {
        let pattern = params["pattern"].as_str().ok_or_else(|| {
            yoagent::types::ToolError::InvalidArgs("Missing 'pattern' argument".into())
        })?;
        let search_path = params["path"].as_str().unwrap_or(".");
        let limit = params["limit"].as_u64().unwrap_or(FIND_DEFAULT_LIMIT);

        let abs_search = resolve_path(search_path, &self.cwd);

        if !abs_search.exists() {
            return Err(yoagent::types::ToolError::Failed(format!(
                "Path not found: {}",
                abs_search.display()
            )));
        }

        if ctx.cancel.is_cancelled() {
            return Err(yoagent::types::ToolError::Cancelled);
        }

        let output = if let Some(fd_path) = which("fd") {
            run_fd(&fd_path, pattern, &abs_search, limit).await?
        } else {
            run_find(pattern, &abs_search, limit).await?
        };

        if ctx.cancel.is_cancelled() {
            return Err(yoagent::types::ToolError::Cancelled);
        }

        Ok(yoagent::types::ToolResult {
            content: vec![yoagent::types::Content::Text { text: output }],
            details: serde_json::Value::Null,
        })
    }
}

async fn run_fd(
    fd: &Path,
    pattern: &str,
    search_path: &Path,
    limit: u64,
) -> Result<String, yoagent::types::ToolError> {
    let mut cmd = tokio::process::Command::new(fd);
    cmd.args([
        "--glob",
        "--color=never",
        "--hidden",
        "--no-require-git",
        "--max-results",
        &limit.to_string(),
    ]);

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

    cmd.arg("--").arg(&effective_pattern).arg(search_path);

    let output = cmd
        .output()
        .await
        .map_err(|e| yoagent::types::ToolError::Failed(format!("Failed to run fd: {}", e)))?;

    let exit_code = output.status.code().unwrap_or(-1);
    if exit_code != 0 && exit_code != 1 {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(yoagent::types::ToolError::Failed(format!(
                "fd error: {}",
                stderr.trim()
            )));
        }
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let results: Vec<String> = stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if results.is_empty() {
        return Ok("No files found matching pattern".to_string());
    }

    let relativized: Vec<String> = results
        .into_iter()
        .map(|line| relativize_path(&line, search_path))
        .collect();

    let mut output = relativized.join("\n");
    if relativized.len() >= limit as usize {
        output.push_str(&format!(
            "\n\n[{} results limit reached. Use limit={} for more, or refine pattern]",
            limit,
            limit * 2,
        ));
    }

    Ok(output)
}

async fn run_find(
    pattern: &str,
    search_path: &Path,
    limit: u64,
) -> Result<String, yoagent::types::ToolError> {
    let name_pattern = pattern.trim_start_matches("**/").trim_start_matches("*/");

    let mut cmd = tokio::process::Command::new("find");
    cmd.arg(search_path)
        .arg("-name")
        .arg(name_pattern)
        .arg("-not")
        .arg("-path")
        .arg("*/node_modules/*")
        .arg("-not")
        .arg("-path")
        .arg("*/.git/*");

    let output = cmd
        .output()
        .await
        .map_err(|e| yoagent::types::ToolError::Failed(format!("Failed to run find: {}", e)))?;

    let exit_code = output.status.code().unwrap_or(-1);
    if exit_code != 0 && exit_code != 1 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(yoagent::types::ToolError::Failed(format!(
            "find error: {}",
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<String> = stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.is_empty() {
        return Ok("No files found matching pattern".to_string());
    }

    let relativized: Vec<String> = lines
        .into_iter()
        .take(limit as usize)
        .map(|line| relativize_path(&line, search_path))
        .collect();

    let mut output = relativized.join("\n");
    if relativized.len() >= limit as usize {
        output.push_str(&format!(
            "\n\n[{} results limit reached. Use limit={} for more, or refine pattern]",
            limit,
            limit * 2,
        ));
    }

    Ok(output)
}

// =====================================================================
// ls tool
// =====================================================================

struct LsTool {
    cwd: PathBuf,
}

#[async_trait]
impl yoagent::types::AgentTool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }
    fn label(&self) -> &str {
        "ls"
    }
    fn description(&self) -> &str {
        "List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. \
         Includes dotfiles. Output is truncated to 500 entries."
    }
    fn parameters_schema(&self) -> serde_json::Value {
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

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: yoagent::types::ToolContext,
    ) -> Result<yoagent::types::ToolResult, yoagent::types::ToolError> {
        let search_path = params["path"].as_str().unwrap_or(".");
        let limit = params["limit"].as_u64().unwrap_or(LS_DEFAULT_LIMIT);

        let abs_path = resolve_path(search_path, &self.cwd);

        if !abs_path.exists() {
            return Err(yoagent::types::ToolError::Failed(format!(
                "Path not found: {}",
                abs_path.display()
            )));
        }
        if !abs_path.is_dir() {
            return Err(yoagent::types::ToolError::Failed(format!(
                "Not a directory: {}",
                abs_path.display()
            )));
        }

        if ctx.cancel.is_cancelled() {
            return Err(yoagent::types::ToolError::Cancelled);
        }

        let entries: Vec<String> = match std::fs::read_dir(&abs_path) {
            Ok(rd) => {
                let mut items: Vec<(String, bool)> = rd
                    .flatten()
                    .map(|entry| {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        (name, is_dir)
                    })
                    .collect();
                items.sort_by_key(|(a, _)| a.to_lowercase());
                items
                    .into_iter()
                    .take(limit as usize)
                    .map(
                        |(name, is_dir)| {
                            if is_dir { format!("{}/", name) } else { name }
                        },
                    )
                    .collect()
            }
            Err(e) => {
                return Err(yoagent::types::ToolError::Failed(format!(
                    "Cannot read directory: {}",
                    e
                )));
            }
        };

        if ctx.cancel.is_cancelled() {
            return Err(yoagent::types::ToolError::Cancelled);
        }

        if entries.is_empty() {
            return Ok(yoagent::types::ToolResult {
                content: vec![yoagent::types::Content::Text {
                    text: "(empty directory)".to_string(),
                }],
                details: serde_json::Value::Null,
            });
        }

        let mut output = entries.join("\n");
        if entries.len() >= limit as usize {
            output.push_str(&format!(
                "\n\n[{} entries limit reached. Use limit={} for more]",
                limit,
                limit * 2,
            ));
        }

        Ok(yoagent::types::ToolResult {
            content: vec![yoagent::types::Content::Text { text: output }],
            details: serde_json::Value::Null,
        })
    }
}

// =====================================================================
// Helpers
// =====================================================================

fn which(name: &str) -> Option<PathBuf> {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| PathBuf::from(name))
}

fn resolve_path(path: &str, cwd: &Path) -> PathBuf {
    if Path::new(path).is_absolute() {
        Path::new(path).to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn relativize_path(path: &str, search_root: &Path) -> String {
    let p = Path::new(path);
    if let Ok(rel) = p.strip_prefix(search_root) {
        rel.to_string_lossy().replace('\\', "/")
    } else {
        p.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string())
    }
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.len() <= max_chars {
        line.to_string()
    } else {
        format!("{}... [truncated]", &line[..max_chars])
    }
}

fn shorten_path_str(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        path.replacen(&home, "~", 1)
    } else if path == "." || path.is_empty() {
        ".".to_string()
    } else {
        path.to_string()
    }
}

// =====================================================================
// Renderers
// =====================================================================

struct GrepRenderer {}

impl ToolRenderer for GrepRenderer {
    fn render_call(
        &self,
        args: &serde_json::Value,
        _width: usize,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Vec<String> {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let search_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let glob = args.get("glob").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64());

        let path_display = shorten_path_str(search_path);

        let mut text = format!(
            "{} /{}/ in {}",
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold("grep")),
            theme.fg_key(ThemeKey::Accent, pattern),
            theme.fg_key(ThemeKey::ToolOutput, &path_display),
        );

        if let Some(g) = glob {
            text.push_str(&theme.fg_key(ThemeKey::ToolOutput, &format!(" ({})", g)));
        }
        if let Some(l) = limit {
            text.push_str(&theme.fg_key(ThemeKey::ToolOutput, &format!(" limit {}", l)));
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
        if content.is_empty() {
            return vec![];
        }
        if !ctx.expanded && !ctx.is_error {
            return vec![];
        }

        let output = content.trim();
        if output.is_empty() || output == "No matches found" {
            return vec![theme.fg_key(ThemeKey::ToolOutput, output)];
        }

        let lines: Vec<&str> = output.lines().collect();
        let max_lines = if ctx.expanded { usize::MAX } else { 15 };
        let display: Vec<&str> = lines.iter().copied().take(max_lines).collect();
        let remaining = lines.len().saturating_sub(display.len());

        let mut result = vec![String::new()];
        for line in &display {
            result.push(theme.fg_key(ThemeKey::ToolOutput, line));
        }
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
        result
    }
}

struct FindRenderer {}

impl ToolRenderer for FindRenderer {
    fn render_call(
        &self,
        args: &serde_json::Value,
        _width: usize,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Vec<String> {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let search_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let limit = args.get("limit").and_then(|v| v.as_u64());

        let path_display = shorten_path_str(search_path);
        let mut text = format!(
            "{} {} in {}",
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold("find")),
            theme.fg_key(ThemeKey::Accent, pattern),
            theme.fg_key(ThemeKey::ToolOutput, &path_display),
        );
        if let Some(l) = limit {
            text.push_str(&theme.fg_key(ThemeKey::ToolOutput, &format!(" (limit {})", l)));
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
        if content.is_empty() {
            return vec![];
        }
        if !ctx.expanded && !ctx.is_error {
            return vec![];
        }

        let output = content.trim();
        if output.is_empty() || output == "No files found matching pattern" {
            return vec![theme.fg_key(ThemeKey::ToolOutput, output)];
        }

        let lines: Vec<&str> = output.lines().collect();
        let max_lines = if ctx.expanded { usize::MAX } else { 20 };
        let display: Vec<&str> = lines.iter().copied().take(max_lines).collect();
        let remaining = lines.len().saturating_sub(display.len());

        let mut result = vec![String::new()];
        for line in &display {
            result.push(theme.fg_key(ThemeKey::ToolOutput, line));
        }
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
        result
    }
}

struct LsRenderer {}

impl ToolRenderer for LsRenderer {
    fn render_call(
        &self,
        args: &serde_json::Value,
        _width: usize,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Vec<String> {
        let search_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let limit = args.get("limit").and_then(|v| v.as_u64());

        let path_display = shorten_path_str(search_path);
        let mut text = format!(
            "{} {}",
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold("ls")),
            theme.fg_key(ThemeKey::Accent, &path_display),
        );
        if let Some(l) = limit {
            text.push_str(&theme.fg_key(ThemeKey::ToolOutput, &format!(" (limit {})", l)));
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
        if content.is_empty() {
            return vec![];
        }
        if !ctx.expanded && !ctx.is_error {
            return vec![];
        }

        let output = content.trim();
        if output.is_empty() || output == "(empty directory)" {
            return vec![theme.fg_key(ThemeKey::ToolOutput, output)];
        }

        let lines: Vec<&str> = output.lines().collect();
        let max_lines = if ctx.expanded { usize::MAX } else { 20 };
        let display: Vec<&str> = lines.iter().copied().take(max_lines).collect();
        let remaining = lines.len().saturating_sub(display.len());

        let mut result = vec![String::new()];
        for line in &display {
            result.push(theme.fg_key(ThemeKey::ToolOutput, line));
        }
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
        result
    }
}
