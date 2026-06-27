use crate::agent::extension::{Extension, ToolDefinition, ToolRenderContext, ToolRenderer};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use async_trait::async_trait;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ── Shared command execution ─────────────────────────────────────

/// Output of a shell command execution.
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// Run a shell command in the given working directory (shared helper for default ops).
async fn run_shell_command(command: &str, cwd: &Path) -> anyhow::Result<ExecOutput> {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .output()
        .await?;
    Ok(ExecOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code(),
    })
}

// ── GrepOperations (pluggable) ──────────────────────────────────

/// Pluggable operations for the grep tool (matching pi's GrepOperations).
/// Override these to delegate command execution to remote systems.
#[async_trait]
pub trait GrepOperations: Send + Sync {
    /// Execute a shell command in the given working directory.
    /// Returns stdout, stderr, and exit code.
    async fn exec(&self, command: &str, cwd: &Path) -> anyhow::Result<ExecOutput>;
}

struct DefaultGrepOperations;

#[async_trait]
impl GrepOperations for DefaultGrepOperations {
    async fn exec(&self, command: &str, cwd: &Path) -> anyhow::Result<ExecOutput> {
        run_shell_command(command, cwd).await
    }
}

// ── FindOperations (pluggable) ───────────────────────────────────

/// Pluggable operations for the find tool (matching pi's FindOperations).
/// Override these to delegate command execution to remote systems.
#[async_trait]
pub trait FindOperations: Send + Sync {
    /// Execute a shell command in the given working directory.
    /// Returns stdout, stderr, and exit code.
    async fn exec(&self, command: &str, cwd: &Path) -> anyhow::Result<ExecOutput>;
}

struct DefaultFindOperations;

#[async_trait]
impl FindOperations for DefaultFindOperations {
    async fn exec(&self, command: &str, cwd: &Path) -> anyhow::Result<ExecOutput> {
        run_shell_command(command, cwd).await
    }
}

// ── LsOperations (pluggable) ─────────────────────────────────────

/// A directory entry for the ls tool.
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Pluggable operations for the ls tool (matching pi's LsOperations).
/// Override these to delegate directory listing to remote systems.
pub trait LsOperations: Send + Sync {
    /// List entries in a directory, returning name and type.
    fn read_dir(&self, path: &Path) -> anyhow::Result<Vec<DirEntry>>;
    /// Check if path is a directory.
    fn is_dir(&self, path: &Path) -> anyhow::Result<bool>;
    /// Check if path exists.
    fn path_exists(&self, path: &Path) -> anyhow::Result<bool>;
}

struct DefaultLsOperations;

impl LsOperations for DefaultLsOperations {
    fn read_dir(&self, path: &Path) -> anyhow::Result<Vec<DirEntry>> {
        let rd = std::fs::read_dir(path)?;
        let mut items: Vec<DirEntry> = rd
            .flatten()
            .map(|entry| DirEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                is_dir: entry.file_type().map(|t| t.is_dir()).unwrap_or(false),
            })
            .collect();
        items.sort_by_key(|e| e.name.to_lowercase());
        Ok(items)
    }
    fn is_dir(&self, path: &Path) -> anyhow::Result<bool> {
        Ok(std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false))
    }
    fn path_exists(&self, path: &Path) -> anyhow::Result<bool> {
        Ok(path.exists())
    }
}

/// Combined filesystem extension providing grep, find, and ls tools.
pub struct FilesystemExtension {
    cwd: PathBuf,
    grep_operations: Arc<dyn GrepOperations>,
    find_operations: Arc<dyn FindOperations>,
    ls_operations: Arc<dyn LsOperations>,
}

impl FilesystemExtension {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            grep_operations: Arc::new(DefaultGrepOperations),
            find_operations: Arc::new(DefaultFindOperations),
            ls_operations: Arc::new(DefaultLsOperations),
        }
    }

    /// Set custom grep operations (e.g. for SSH targets).
    pub fn with_grep_operations(mut self, ops: Arc<dyn GrepOperations>) -> Self {
        self.grep_operations = ops;
        self
    }

    /// Set custom find operations (e.g. for SSH targets).
    pub fn with_find_operations(mut self, ops: Arc<dyn FindOperations>) -> Self {
        self.find_operations = ops;
        self
    }

    /// Set custom ls operations (e.g. for SSH targets).
    pub fn with_ls_operations(mut self, ops: Arc<dyn LsOperations>) -> Self {
        self.ls_operations = ops;
        self
    }
}

impl Extension for FilesystemExtension {
    fn name(&self) -> Cow<'static, str> {
        "filesystem".into()
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                tool: Box::new(GrepTool {
                    cwd: self.cwd.clone(),
                    operations: self.grep_operations.clone(),
                }),
                snippet: "Search file contents for patterns (respects .gitignore)",
                guidelines: &["Use grep for searching file contents with patterns"],
                prepare_arguments: None,
                before_tool_call: None,
                after_tool_call: None,
                renderer: Some(std::sync::Arc::new(ListRenderer::grep())),
            },
            ToolDefinition {
                tool: Box::new(FindTool {
                    cwd: self.cwd.clone(),
                    operations: self.find_operations.clone(),
                }),
                snippet: "Find files by glob pattern (respects .gitignore)",
                guidelines: &["Use find for locating files by pattern"],
                prepare_arguments: None,
                before_tool_call: None,
                after_tool_call: None,
                renderer: Some(std::sync::Arc::new(ListRenderer::find())),
            },
            ToolDefinition {
                tool: Box::new(LsTool {
                    cwd: self.cwd.clone(),
                    operations: self.ls_operations.clone(),
                }),
                snippet: "List directory contents",
                guidelines: &["Use ls for exploring directory structure"],
                prepare_arguments: None,
                before_tool_call: None,
                after_tool_call: None,
                renderer: Some(std::sync::Arc::new(ListRenderer::ls())),
            },
        ]
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
    operations: Arc<dyn GrepOperations>,
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
        let search_owned = resolve_path(search_path, &self.cwd);
        let abs_search = &search_owned;

        let glob = params["glob"].as_str();
        let ignore_case = params["ignoreCase"].as_bool().unwrap_or(false);
        let literal = params["literal"].as_bool().unwrap_or(false);
        let context = params["context"].as_u64().unwrap_or(0);
        let limit = params["limit"].as_u64().unwrap_or(GREP_DEFAULT_LIMIT);

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
            run_rg_with_ops(
                self.operations.as_ref(),
                &self.cwd,
                &rg,
                pattern,
                abs_search,
                glob,
                ignore_case,
                literal,
                context,
                limit,
            )
            .await?
        } else {
            run_grep_with_ops(
                self.operations.as_ref(),
                &self.cwd,
                pattern,
                abs_search,
                ignore_case,
                literal,
                context,
                limit,
            )
            .await?
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

/// Build a ripgrep command string and execute via operations.
#[allow(clippy::too_many_arguments)]
async fn run_rg_with_ops(
    ops: &dyn GrepOperations,
    cwd: &Path,
    rg: &Path,
    pattern: &str,
    search_path: &Path,
    glob: Option<&str>,
    ignore_case: bool,
    literal: bool,
    context: u64,
    limit: u64,
) -> Result<String, yoagent::types::ToolError> {
    let mut cmd_parts: Vec<String> = vec![
        rg.to_string_lossy().to_string(),
        "--json".into(),
        "--line-number".into(),
        "--color=never".into(),
        "--hidden".into(),
    ];
    if ignore_case {
        cmd_parts.push("--ignore-case".into());
    }
    if literal {
        cmd_parts.push("--fixed-strings".into());
    }
    if let Some(g) = glob {
        cmd_parts.push("--glob".into());
        cmd_parts.push(g.to_string());
    }
    if context > 0 {
        cmd_parts.push("-C".into());
        cmd_parts.push(context.to_string());
    }
    cmd_parts.push("--max-count".into());
    cmd_parts.push(limit.to_string());
    cmd_parts.push("--".into());
    cmd_parts.push(pattern.to_string());
    cmd_parts.push(search_path.to_string_lossy().to_string());

    let command = cmd_parts.join(" ");
    let exec_output = ops
        .exec(&command, cwd)
        .await
        .map_err(|e| yoagent::types::ToolError::Failed(format!("Failed to run rg: {}", e)))?;

    let exit_code = exec_output.exit_code.unwrap_or(-1);
    if exit_code == 2 {
        return Err(yoagent::types::ToolError::Failed(format!(
            "ripgrep error: {}",
            exec_output.stderr.trim()
        )));
    }

    let stdout = &exec_output.stdout;
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

#[allow(clippy::too_many_arguments)]
async fn run_grep_with_ops(
    ops: &dyn GrepOperations,
    cwd: &Path,
    pattern: &str,
    search_path: &Path,
    ignore_case: bool,
    literal: bool,
    context: u64,
    limit: u64,
) -> Result<String, yoagent::types::ToolError> {
    let mut cmd_parts: Vec<String> = vec![
        "grep".into(),
        "--line-number".into(),
        "--color=never".into(),
        "--binary-files=without-match".into(),
    ];
    if ignore_case {
        cmd_parts.push("-i".into());
    }
    if literal {
        cmd_parts.push("-F".into());
    }
    if context > 0 {
        cmd_parts.push("-C".into());
        cmd_parts.push(context.to_string());
    }
    cmd_parts.push("--max-count".into());
    cmd_parts.push(limit.to_string());
    cmd_parts.push("-r".into());
    cmd_parts.push("--".into());
    cmd_parts.push(pattern.to_string());
    cmd_parts.push(search_path.to_string_lossy().to_string());

    let command = cmd_parts.join(" ");
    let exec_output = ops
        .exec(&command, cwd)
        .await
        .map_err(|e| yoagent::types::ToolError::Failed(format!("Failed to run grep: {}", e)))?;

    let exit_code = exec_output.exit_code.unwrap_or(-1);
    if exit_code == 2 {
        return Err(yoagent::types::ToolError::Failed(format!(
            "grep error: {}",
            exec_output.stderr.trim()
        )));
    }

    let trimmed = exec_output.stdout.trim();
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
    operations: Arc<dyn FindOperations>,
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
        let search_owned = resolve_path(search_path, &self.cwd);
        let abs_search = &search_owned;
        let limit = params["limit"].as_u64().unwrap_or(FIND_DEFAULT_LIMIT);

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
            run_fd_with_ops(
                self.operations.as_ref(),
                &self.cwd,
                &fd_path,
                pattern,
                abs_search,
                limit,
            )
            .await?
        } else {
            run_find_with_ops(
                self.operations.as_ref(),
                &self.cwd,
                pattern,
                abs_search,
                limit,
            )
            .await?
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

async fn run_fd_with_ops(
    ops: &dyn FindOperations,
    cwd: &Path,
    fd: &Path,
    pattern: &str,
    search_path: &Path,
    limit: u64,
) -> Result<String, yoagent::types::ToolError> {
    // Build effective pattern for fd
    let effective_pattern = if pattern.contains('/') {
        if !pattern.starts_with('/') && !pattern.starts_with("**/") && pattern != "**" {
            format!("**/{}", pattern)
        } else {
            pattern.to_string()
        }
    } else {
        pattern.to_string()
    };

    // Build fd command string
    let mut cmd_parts: Vec<String> = vec![
        fd.to_string_lossy().to_string(),
        "--glob".into(),
        "--color=never".into(),
        "--hidden".into(),
        "--no-require-git".into(),
        "--max-results".into(),
        limit.to_string(),
    ];
    if pattern.contains('/') {
        cmd_parts.push("--full-path".into());
    }
    cmd_parts.push("--".into());
    cmd_parts.push(effective_pattern);
    cmd_parts.push(search_path.to_string_lossy().to_string());

    let command = cmd_parts.join(" ");
    let exec_output = ops
        .exec(&command, cwd)
        .await
        .map_err(|e| yoagent::types::ToolError::Failed(format!("Failed to run fd: {}", e)))?;

    let exit_code = exec_output.exit_code.unwrap_or(-1);
    if exit_code != 0 && exit_code != 1 && exec_output.stdout.trim().is_empty() {
        return Err(yoagent::types::ToolError::Failed(format!(
            "fd error: {}",
            exec_output.stderr.trim()
        )));
    }

    let results: Vec<String> = exec_output
        .stdout
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

async fn run_find_with_ops(
    ops: &dyn FindOperations,
    cwd: &Path,
    pattern: &str,
    search_path: &Path,
    limit: u64,
) -> Result<String, yoagent::types::ToolError> {
    let name_pattern = pattern.trim_start_matches("**/").trim_start_matches("*/");

    let cmd_parts: Vec<String> = vec![
        "find".into(),
        search_path.to_string_lossy().to_string(),
        "-name".into(),
        name_pattern.to_string(),
        "-not".into(),
        "-path".into(),
        "*/node_modules/*".into(),
        "-not".into(),
        "-path".into(),
        "*/.git/*".into(),
    ];

    let command = cmd_parts.join(" ");
    let exec_output = ops
        .exec(&command, cwd)
        .await
        .map_err(|e| yoagent::types::ToolError::Failed(format!("Failed to run find: {}", e)))?;

    let exit_code = exec_output.exit_code.unwrap_or(-1);
    if exit_code != 0 && exit_code != 1 {
        return Err(yoagent::types::ToolError::Failed(format!(
            "find error: {}",
            exec_output.stderr.trim()
        )));
    }

    let lines: Vec<String> = exec_output
        .stdout
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
    operations: Arc<dyn LsOperations>,
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

        if !self.operations.path_exists(&abs_path).unwrap_or(false) {
            return Err(yoagent::types::ToolError::Failed(format!(
                "Path not found: {}",
                abs_path.display()
            )));
        }
        if !self.operations.is_dir(&abs_path).unwrap_or(false) {
            return Err(yoagent::types::ToolError::Failed(format!(
                "Not a directory: {}",
                abs_path.display()
            )));
        }

        if ctx.cancel.is_cancelled() {
            return Err(yoagent::types::ToolError::Cancelled);
        }

        let entries: Vec<String> = match self.operations.read_dir(&abs_path) {
            Ok(items) => {
                let mut items: Vec<(String, bool)> = items
                    .into_iter()
                    .map(|entry| (entry.name, entry.is_dir))
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
// Shared list renderer (used by grep, find, ls)
// =====================================================================

struct ListRenderer {
    tool_name: &'static str,
    /// Format: "{pattern}" or "/{pattern}/" or empty for no pattern.
    pattern_format: &'static str,
    no_results_text: &'static str,
    collapsed_lines: usize,
    show_glob: bool,
}

impl ListRenderer {
    fn grep() -> Self {
        Self {
            tool_name: "grep",
            pattern_format: "/{}/ ",
            no_results_text: "No matches found",
            collapsed_lines: 15,
            show_glob: true,
        }
    }

    fn find() -> Self {
        Self {
            tool_name: "find",
            pattern_format: "{} in ",
            no_results_text: "No files found matching pattern",
            collapsed_lines: 20,
            show_glob: false,
        }
    }

    fn ls() -> Self {
        Self {
            tool_name: "ls",
            pattern_format: "",
            no_results_text: "(empty directory)",
            collapsed_lines: 20,
            show_glob: false,
        }
    }
}

impl ToolRenderer for ListRenderer {
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
            "{} {}{}",
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold(self.tool_name)),
            if self.pattern_format.is_empty() {
                String::new()
            } else {
                let p = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                theme.fg_key(ThemeKey::Accent, &self.pattern_format.replace("{}", p))
            },
            theme.fg_key(ThemeKey::ToolOutput, &path_display),
        );

        if self.show_glob
            && let Some(g) = args.get("glob").and_then(|v| v.as_str())
        {
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
        if output.is_empty() || output == self.no_results_text {
            return vec![theme.fg_key(ThemeKey::ToolOutput, output)];
        }

        let lines: Vec<&str> = output.lines().collect();
        let max_lines = if ctx.expanded {
            usize::MAX
        } else {
            self.collapsed_lines
        };
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
