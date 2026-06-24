use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use crate::tui::visual_truncate::truncate_to_visual_lines;
use anyhow::Context;
use async_trait::async_trait;
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::{Mutex as TokioMutex, mpsc::UnboundedSender};

pub struct BashExtension {
    cwd: std::path::PathBuf,
}

impl BashExtension {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for BashExtension {
    fn name(&self) -> Cow<'static, str> {
        "bash".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(BashTool {
            cwd: self.cwd.clone(),
        })]
    }
}

struct BashTool {
    cwd: std::path::PathBuf,
}

// ── Constants ────────────────────────────────────────────────────

const DEFAULT_MAX_LINES: usize = 2000;
const DEFAULT_MAX_BYTES: usize = 50 * 1024; // 50KB
const DEFAULT_TIMEOUT_SECS: u64 = 300; // 5 minutes default timeout for all commands

// ── Helpers ──────────────────────────────────────────────────────

/// Kill a process group by its leader PID.
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    if pid > 0 {
        let _ = std::process::Command::new("kill")
            .arg("--")
            .arg(format!("-{}", pid))
            .spawn();
    }
}

#[cfg(not(unix))]
fn kill_process_group(pid: u32) {
    let _ = pid;
}

/// Spawn a bash command with process group setup for clean cancellation.
fn spawn_bash_command(
    command: &str,
    cwd: &std::path::Path,
) -> std::io::Result<tokio::process::Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut std_cmd = std::process::Command::new("sh");
        std_cmd.arg("-c").arg(command).current_dir(cwd);
        unsafe {
            std_cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        let mut tokio_cmd = tokio::process::Command::from(std_cmd);
        tokio_cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    }
    #[cfg(not(unix))]
    {
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    }
}

/// Format the final bash execution result, matching pi's bash tool output format.
///
/// Pi's bash tool (LLM-called) returns raw output, not the `bashExecutionToText` format.
/// - Non-empty output → raw output (no Ran prefix, no backtick fences)
/// - Empty output → "(no output)"
/// - Truncated → raw output + `\n\n[Showing lines X-Y of Z... Full output: path]`
/// - Non-zero exit → returned as Err with output + `\n\nCommand exited with code N`
/// - Cancelled → returned as Err with output + `\n\nCommand aborted`
fn finish_bash_execution(
    _command: &str,
    combined: &str,
    exit_code: i32,
    cancelled: bool,
    _started_at: Instant,
    on_update: Option<UnboundedSender<ToolOutput>>,
) -> Result<ToolOutput, anyhow::Error> {
    // Apply tail truncation (pi-style: keep last N lines/bytes)
    let trunc = truncate_tail(combined, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);

    // Build output text: raw output or (no output)
    let mut result_text = if trunc.content.is_empty() {
        "(no output)".to_string()
    } else {
        trunc.content.clone()
    };

    // Truncation notice (matching pi: appended to text, not in details)
    if trunc.truncated {
        let tmp_dir = std::env::temp_dir().join("rab-bash");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let tmp_path = tmp_dir.join(format!("{}.txt", uuid::Uuid::new_v4()));
        let saved = if std::fs::write(&tmp_path, combined).is_ok() {
            Some(tmp_path)
        } else {
            None
        };

        let start_line = trunc.total_lines - trunc.output_lines + 1;
        let end_line = trunc.total_lines;

        let notice = if trunc.truncated_by == "lines" {
            format!(
                "\n\n[Showing lines {}-{} of {}. Full output: {}]",
                start_line,
                end_line,
                trunc.total_lines,
                saved
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            )
        } else {
            format!(
                "\n\n[Showing lines {}-{} of {} ({} limit). Full output: {}]",
                start_line,
                end_line,
                trunc.total_lines,
                format_size(DEFAULT_MAX_BYTES),
                saved
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            )
        };
        result_text.push_str(&notice);
    }

    // Send final update (before error conversion, so UI shows the output)
    if let Some(ref tx) = on_update {
        let _ = tx.send(ToolOutput::ok(result_text.clone()));
    }

    // Error cases: return as Err with output + status (matching pi)
    if cancelled {
        let err_msg = if result_text.is_empty() || result_text == "(no output)" {
            "Command aborted".to_string()
        } else {
            format!("{}\n\nCommand aborted", result_text)
        };
        return Err(anyhow::anyhow!("{}", err_msg));
    }

    if exit_code != 0 {
        let err_msg = if result_text.is_empty() || result_text == "(no output)" {
            format!("Command exited with code {}", exit_code)
        } else {
            format!("{}\n\nCommand exited with code {}", result_text, exit_code)
        };
        return Err(anyhow::anyhow!("{}", err_msg));
    }

    Ok(ToolOutput::ok(result_text))
}

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

/// Truncation result for tail-based truncation (keep last N lines/bytes).
struct TailTruncation {
    /// Truncated output content.
    content: String,
    /// Whether truncation occurred.
    truncated: bool,
    // Fields below are only used in tests; kept for test assertions.
    #[allow(dead_code)]
    total_lines: usize,
    #[allow(dead_code)]
    output_lines: usize,
    #[allow(dead_code)]
    output_bytes: usize,
    #[allow(dead_code)]
    truncated_by: &'static str, // "lines" | "bytes"
    #[allow(dead_code)]
    last_line_partial: bool,
}

/// Truncate content from the tail, keeping complete lines that fit within limits.
/// Keeps the LAST N lines/bytes. Never returns partial lines unless the last line
/// of the original content exceeds the byte limit.
fn truncate_tail(content: &str, max_lines: usize, max_bytes: usize) -> TailTruncation {
    let total_bytes = content.len();
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Check if no truncation needed
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TailTruncation {
            content: content.to_string(),
            truncated: false,
            total_lines,
            output_lines: total_lines,
            output_bytes: total_bytes,
            truncated_by: "",
            last_line_partial: false,
        };
    }

    // Work backwards from the end
    let mut output: Vec<&str> = Vec::new();
    let mut byte_count: usize = 0;
    let mut truncated_by = "lines";
    let mut last_line_partial = false;

    for line in lines.iter().rev().take(max_lines) {
        let line_bytes = line.len();
        let with_newline = if output.is_empty() {
            line_bytes
        } else {
            line_bytes + 1 // +1 for preceding newline
        };

        if byte_count + with_newline > max_bytes {
            truncated_by = "bytes";
            // If we haven't added ANY lines yet and this line exceeds maxBytes,
            // take the end of the line (partial)
            if output.is_empty() {
                let end_start = line.len().saturating_sub(max_bytes);
                let truncated_line = &line[end_start..];
                output.push(truncated_line);
                byte_count = truncated_line.len();
                last_line_partial = true;
            }
            break;
        }

        output.push(line);
        byte_count += with_newline;
    }

    if output.len() >= max_lines && byte_count <= max_bytes {
        truncated_by = "lines";
    }

    output.reverse();
    TailTruncation {
        content: output.join("\n"),
        truncated: true,
        total_lines,
        output_lines: output.len(),
        output_bytes: byte_count,
        truncated_by,
        last_line_partial,
    }
}

// ── AgentTool implementation ─────────────────────────────────────

#[async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command in the current working directory. Returns stdout and stderr. \
         Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, \
         full output is saved to a temp file. Optionally provide a timeout in seconds."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash command to execute"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (optional, no default timeout)"
                }
            }
        })
    }

    fn renderer(&self) -> Option<Box<dyn ToolRenderer>> {
        Some(Box::new(BashRenderer))
    }

    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
        cancel: Cancel,
        on_update: Option<UnboundedSender<ToolOutput>>,
    ) -> anyhow::Result<ToolOutput> {
        let _ = tool_call_id;
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;
        let timeout = args["timeout"].as_u64().or(Some(DEFAULT_TIMEOUT_SECS));
        let started_at = Instant::now();

        cancel.check()?;

        // Build the command with process group setup for process-tree killing
        let mut child = spawn_bash_command(command, &self.cwd)
            .with_context(|| format!("Failed to spawn command: {}", command))?;

        let pid = child.id().unwrap_or(0);

        // Shared output buffer for streaming reads
        let combined = Arc::new(TokioMutex::new(String::new()));
        let combined_clone = combined.clone();

        // Read stdout in a background task
        let stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;
        let stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr"))?;

        use tokio::io::AsyncReadExt;
        let read_task = tokio::spawn(async move {
            let mut stdout_buf = vec![0u8; 4096];
            let mut stderr_buf = vec![0u8; 4096];
            let mut stdout_reader = stdout_pipe;
            let mut stderr_reader = stderr_pipe;
            let mut stdout_done = false;
            let mut stderr_done = false;
            loop {
                tokio::select! {
                    result = stdout_reader.read(&mut stdout_buf), if !stdout_done => {
                        match result {
                            Ok(0) => stdout_done = true,
                            Ok(n) => {
                                let mut out = combined_clone.lock().await;
                                out.push_str(&String::from_utf8_lossy(&stdout_buf[..n]));
                            }
                            Err(_) => stdout_done = true,
                        }
                    }
                    result = stderr_reader.read(&mut stderr_buf), if !stderr_done => {
                        match result {
                            Ok(0) => stderr_done = true,
                            Ok(n) => {
                                let mut out = combined_clone.lock().await;
                                out.push_str(&String::from_utf8_lossy(&stderr_buf[..n]));
                            }
                            Err(_) => stderr_done = true,
                        }
                    }
                }
                if stdout_done && stderr_done {
                    break;
                }
            }
        });

        // Set up cancellation monitor: kill the process group if cancelled
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_clone = cancelled.clone();
        let _cancel_monitor: tokio::task::JoinHandle<()> = tokio::spawn(async move {
            while !cancel.is_cancelled() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            cancel_clone.store(true, Ordering::SeqCst);
            kill_process_group(pid);
        });

        // Wait for the process to exit, with optional timeout and streaming updates
        let timeout_dur = timeout.map(std::time::Duration::from_secs);
        loop {
            // Check cancellation
            if cancelled.load(Ordering::SeqCst) {
                kill_process_group(pid);
                read_task.abort();
                return Err(anyhow::anyhow!("Command aborted"));
            }

            // Check timeout
            if let Some(dur) = timeout_dur
                && started_at.elapsed() > dur
            {
                kill_process_group(pid);
                read_task.abort();
                return Err(anyhow::anyhow!(
                    "Command timed out after {} seconds",
                    timeout.unwrap_or(0)
                ));
            }

            // Send streaming update (1s tick interval, matching pi)
            if let Some(ref tx) = on_update {
                let out = combined.lock().await;
                if !out.is_empty() {
                    let elapsed = started_at.elapsed();
                    let display = format!(
                        "{}\n\n[Elapsed {:.1}s]",
                        out.trim_end(),
                        elapsed.as_secs_f64()
                    );
                    let _ = tx.send(ToolOutput::ok(display));
                }
            }

            // Check if process has exited
            match child.try_wait() {
                Ok(Some(status)) => {
                    read_task.await.ok();
                    let combined_str = combined.lock().await.clone();
                    let exit_code = status.code().unwrap_or(-1);

                    return finish_bash_execution(
                        command,
                        &combined_str,
                        exit_code,
                        false,
                        started_at,
                        on_update,
                    );
                }
                Ok(None) => {
                    // Still running, poll again soon (1s tick, matching pi)
                    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                }
                Err(_) => {
                    read_task.await.ok();
                    let combined_str = combined.lock().await.clone();
                    let exit_code = -1;
                    return finish_bash_execution(
                        command,
                        &combined_str,
                        exit_code,
                        false,
                        started_at,
                        on_update,
                    );
                }
            }
        }
    }
}

/// Tool renderer for the `bash` tool.
/// Formats call headers with `$ command` and result with tail-based preview.
/// Tool renderer for the `bash` tool.
/// Formats call headers with `$ command` and result with tail-based preview.
struct BashRenderer;

// ── Visual-line-aware truncation (delegated to shared module) ────

impl ToolRenderer for BashRenderer {
    fn render_call(
        &self,
        args: &serde_json::Value,
        _width: usize,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Vec<String> {
        let cmd = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("...");
        let timeout = args.get("timeout").and_then(|v| v.as_i64());
        let timeout_suffix = timeout
            .map(|t| theme.fg_key(ThemeKey::Muted, &format!(" (timeout {}s)", t)))
            .unwrap_or_default();

        vec![format!(
            "{}{}",
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold(&format!("$ {}", cmd))),
            timeout_suffix
        )]
    }

    fn render_result(
        &self,
        content: &str,
        width: usize,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();

        // Strip truncation footer and trim trailing whitespace/newlines
        // (matching pi's output.trim() in rebuildBashResultRenderComponent)
        let clean = strip_context_truncation_footer(content)
            .trim_end()
            .to_string();
        // Use lines() instead of split('\n') to avoid trailing empty string
        // from final newline (split would produce ["a", "b", ""] vs lines() ["a", "b"])
        let all_lines: Vec<&str> = clean.lines().collect();

        if all_lines.is_empty() || (all_lines.len() == 1 && all_lines[0].is_empty()) {
            return lines;
        }

        // Visual-line-aware truncation (matching pi's truncateToVisualLines)
        let preview_count = 5;
        let (preview_lines, hidden_line_count) = if ctx.expanded {
            (all_lines.clone(), 0)
        } else {
            truncate_to_visual_lines(&all_lines, width, preview_count)
        };

        if !ctx.expanded && hidden_line_count > 0 {
            let hint = if ctx.expand_key.is_empty() {
                theme.fg_key(
                    ThemeKey::Muted,
                    &format!("... {} earlier lines", hidden_line_count),
                )
            } else {
                theme.fg(
                    "muted",
                    &format!(
                        "... ({} earlier lines, {} to expand)",
                        hidden_line_count, ctx.expand_key
                    ),
                )
            };
            lines.push(hint);
        }

        let fg_key = if ctx.is_error { "error" } else { "toolOutput" };
        for line in &preview_lines {
            if line.is_empty() {
                lines.push(String::new());
            } else {
                lines.push(theme.fg(fg_key, line));
            }
        }

        // Duration (with blank line separator before it, matching pi's `\n` prefix)
        if let Some(secs) = ctx.duration_secs {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            let is_complete = ctx.exit_code.is_some() || ctx.cancelled;
            let label = if is_complete { "Took" } else { "Elapsed" };
            lines.push(theme.fg_key(ThemeKey::Muted, &format!("{} {:.1}s", label, secs)));
        }

        // Pi does not add separate exit code or cancelled status lines because
        // the tool result content already includes "Command exited with code N" or
        // "Command aborted" from the tool error response. The content is rendered
        // as-is above, preserving the status at the end where truncation keeps it.

        // Truncation warnings (with blank line separator, matching pi)
        if ctx.was_truncated {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            if let Some(ref path) = ctx.full_output_path {
                lines.push(theme.fg(
                    "warning",
                    &format!("Output truncated. Full output: {}", path),
                ));
            } else {
                lines.push(theme.fg_key(ThemeKey::Warning, "Output truncated."));
            }
        }

        lines
    }
}

/// Strip the context-truncation footer from bash output.
fn strip_context_truncation_footer(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() < 3 {
        return output.to_string();
    }
    let last = lines.last().map_or("", |v| v).trim();
    if last.starts_with('[')
        && (last.contains("Showing lines") || last.contains("Showing last"))
        && last.contains("Full output:")
    {
        let before: Vec<&str> = lines[..lines.len() - 1].to_vec();
        if !before.is_empty() && before[before.len() - 1].is_empty() {
            before[..before.len() - 1].join("\n")
        } else {
            before.join("\n")
        }
    } else {
        output.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool() -> BashTool {
        BashTool {
            cwd: std::env::temp_dir(),
        }
    }

    #[tokio::test]
    async fn runs_simple_command() {
        let tool = make_tool();
        let output = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "echo hello"}),
                Cancel::new(),
                None,
            )
            .await
            .unwrap();
        assert!(output.content.contains("hello"));
    }

    #[tokio::test]
    async fn captures_stderr() {
        let tool = make_tool();
        let output = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "echo err >&2"}),
                Cancel::new(),
                None,
            )
            .await
            .unwrap();
        assert!(output.content.contains("err"));
    }

    #[tokio::test]
    async fn cancel_aborts() {
        let tool = make_tool();
        let cancel = Cancel::new();
        cancel.cancel();
        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "sleep 10"}),
                cancel,
                None,
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cancelled") || err.contains("aborted"),
            "expected cancellation error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn timeout_works() {
        let tool = make_tool();
        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "sleep 10", "timeout": 1}),
                Cancel::new(),
                None,
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out"));
    }

    #[test]
    fn test_truncate_tail_no_truncation() {
        let result = truncate_tail("hello\nworld\n", 2000, 50000);
        assert!(!result.truncated);
        assert_eq!(result.content, "hello\nworld\n");
    }

    #[test]
    fn test_truncate_tail_by_lines() {
        let content: String = (1..=5000).map(|i| format!("line {}\n", i)).collect();
        let result = truncate_tail(&content, 2000, 50000);
        assert!(result.truncated);
        assert!(result.content.starts_with("line 3001"));
        assert_eq!(result.content.lines().count(), 2000);
    }

    #[test]
    fn test_truncate_tail_by_bytes() {
        let content: String = (1..=100)
            .map(|i| format!("line {} {}\n", i, "x".repeat(1000)))
            .collect();
        let result = truncate_tail(&content, 2000, 50000);
        assert!(result.truncated);
        assert!(result.content.len() <= 50000);
        assert!(result.content.lines().count() < 100);
    }

    #[test]
    fn test_truncate_tail_partial_last_line() {
        // A single line that exceeds the byte limit
        let content = format!("short\n{}\n", "x".repeat(60000));
        let result = truncate_tail(&content, 2000, 50000);
        assert!(result.truncated);
        assert!(!result.content.starts_with("short"));
        assert!(result.content.len() <= 50000);
    }

    #[test]
    fn test_truncate_tail_empty() {
        let result = truncate_tail("", 2000, 50000);
        assert!(!result.truncated);
        assert_eq!(result.content, "");
    }

    // ── Exit code integration tests ──────────────────────────────

    #[tokio::test]
    async fn exit_code_nonzero() {
        let tool = make_tool();
        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "exit 42"}),
                Cancel::new(),
                None,
            )
            .await;
        assert!(result.is_err(), "non-zero exit should return error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exited with code 42"), "got: {}", err);
    }

    #[tokio::test]
    async fn exit_code_with_output() {
        let tool = make_tool();
        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "echo before && exit 1"}),
                Cancel::new(),
                None,
            )
            .await;
        assert!(result.is_err(), "non-zero exit should return error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("before"), "got: {}", err);
        assert!(err.contains("exited with code 1"), "got: {}", err);
    }

    #[tokio::test]
    async fn no_output() {
        let tool = make_tool();
        let output = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "true"}),
                Cancel::new(),
                None,
            )
            .await
            .unwrap();
        assert!(
            output.content.contains("(no output)"),
            "got: {}",
            output.content
        );
    }

    #[tokio::test]
    async fn combined_stdout_stderr() {
        let tool = make_tool();
        let output = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "echo out; echo err >&2"}),
                Cancel::new(),
                None,
            )
            .await
            .unwrap();
        assert!(output.content.contains("out"), "got: {}", output.content);
        assert!(output.content.contains("err"), "got: {}", output.content);
    }

    #[tokio::test]
    async fn runs_in_cwd() {
        let tmp = std::env::temp_dir().join(format!("rab-bash-cwd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("marker.txt"), "hello").unwrap();

        let tool = BashTool { cwd: tmp.clone() };
        let output = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "cat marker.txt"}),
                Cancel::new(),
                None,
            )
            .await
            .unwrap();
        assert!(output.content.contains("hello"), "got: {}", output.content);
    }

    #[tokio::test]
    async fn missing_command_errors() {
        let tool = make_tool();
        let result = tool
            .execute("id".into(), serde_json::json!({}), Cancel::new(), None)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("command"), "got: {}", err);
    }

    #[tokio::test]
    async fn timeout_with_partial_output() {
        let tool = make_tool();
        // Command that produces some output then hangs
        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "echo start && sleep 10 && echo end", "timeout": 1}),
                Cancel::new(),
                None,
            )
            .await;
        // May timeout before process is killed, which is fine
        // The key is it doesn't hang forever
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out"), "got: {}", err);
    }

    #[tokio::test]
    async fn cancel_during_long_command() {
        let tool = make_tool();
        let cancel = Cancel::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            tool.execute(
                "id".into(),
                serde_json::json!({"command": "sleep 30"}),
                cancel_clone,
                None,
            )
            .await
        });

        // Give it a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        cancel.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("aborted") || err.contains("cancelled"),
            "expected cancellation error, got: {}",
            err
        );
    }

    // ── Truncation boundary tests ────────────────────────────────

    #[test]
    fn test_truncate_tail_exact_line_fit() {
        // Content exactly at the line limit - no truncation
        let lines: String = (1..=2000).map(|i| format!("line {}\n", i)).collect();
        let result = truncate_tail(&lines, 2000, 50000);
        assert!(
            !result.truncated,
            "should not truncate when exactly at line limit"
        );
        assert!(result.content.lines().count() == 2000);
    }

    #[test]
    fn test_truncate_tail_one_over_line_limit() {
        let lines: String = (1..=2001).map(|i| format!("line {}\n", i)).collect();
        let result = truncate_tail(&lines, 2000, 50000);
        assert!(result.truncated);
        assert_eq!(result.content.lines().count(), 2000);
        // Should keep last 2000 lines
        assert!(result.content.starts_with("line 2"));
    }

    #[test]
    fn test_truncate_tail_exact_byte_fit() {
        // Content exactly at byte limit - no truncation
        let line = "a".repeat(50000);
        let result = truncate_tail(&line, 2000, 50000);
        assert!(!result.truncated);
    }

    #[test]
    fn test_truncate_tail_one_byte_over() {
        // Content one byte over the limit
        let line = "a".repeat(50001);
        let result = truncate_tail(&line, 2000, 50000);
        assert!(result.truncated);
        assert!(result.content.len() <= 50000);
    }

    #[test]
    fn test_truncate_tail_single_line_under_limit() {
        let result = truncate_tail("hello world", 2000, 50000);
        assert!(!result.truncated);
        assert_eq!(result.content, "hello world");
    }

    #[test]
    fn test_truncate_tail_trailing_newline() {
        let result = truncate_tail("a\nb\n", 2000, 50000);
        assert!(!result.truncated);
        assert_eq!(result.content, "a\nb\n");
    }

    #[test]
    fn test_truncate_tail_no_trailing_newline() {
        let result = truncate_tail("a\nb", 2000, 50000);
        assert!(!result.truncated);
        assert_eq!(result.content, "a\nb");
    }

    #[test]
    fn test_truncate_tail_single_line_exceeds_limit() {
        let content = "x".repeat(60000);
        let result = truncate_tail(&content, 2000, 50000);
        assert!(result.truncated);
        assert!(result.last_line_partial);
        // Should keep the last 50000 bytes of the line
        assert_eq!(result.content.len(), 50000);
        assert!(result.content.ends_with("x".repeat(50000).as_str()));
    }

    #[test]
    fn test_truncate_tail_byte_count_respects_newlines() {
        // Each line is 1000 bytes, 50 lines = 50KB, plus 49 newlines = ~49 bytes extra
        // At 2000 line limit, byte limit should be hit first
        let content: String = (1..=100)
            .map(|i| format!("line {} {}\n", i, "x".repeat(1000)))
            .collect();
        let result = truncate_tail(&content, 2000, 50000);
        assert!(result.truncated);
        // Output bytes should be at most 50000 (byte limit)
        assert!(
            result.output_bytes <= 50000,
            "output_bytes {} exceeds limit 50000",
            result.output_bytes
        );
    }

    // ── Truncation footer tests ─────────────────────────────────

    #[tokio::test]
    async fn truncated_by_lines_shows_footer() {
        let tool = make_tool();
        // Generate 3000 lines of output (exceeds 2000 line limit)
        let cmd = "for i in $(seq 1 3000); do echo \"line $i\"; done";
        let output = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": cmd}),
                Cancel::new(),
                None,
            )
            .await
            .unwrap();
        assert!(
            output.content.contains("Showing lines"),
            "got: {}",
            output.content
        );
        assert!(
            output.content.contains("Full output:"),
            "got: {}",
            output.content
        );
    }

    #[tokio::test]
    async fn small_output_no_footer() {
        let tool = make_tool();
        let output = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "echo hello"}),
                Cancel::new(),
                None,
            )
            .await
            .unwrap();
        // Small output should not have footer markers
        assert!(
            !output.content.contains("Output truncated"),
            "got: {}",
            output.content
        );
        assert!(
            !output.content.contains("Full output:"),
            "got: {}",
            output.content
        );
    }

    #[tokio::test]
    async fn truncated_saves_temp_file() {
        let tool = make_tool();
        // Generate enough output to exceed line limit
        let cmd = "for i in $(seq 1 3000); do echo \"line $i\"; done";
        let output = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": cmd}),
                Cancel::new(),
                None,
            )
            .await
            .unwrap();
        // Should mention a temp file path
        assert!(
            output.content.contains("/rab-bash/"),
            "expected temp file path, got: {}",
            output.content
        );
    }

    // ── Truncate tail: many short lines ──────────────────────────

    #[test]
    fn test_truncate_tail_many_short_lines() {
        // 10000 very short lines, well under byte limit
        let content: String = (1..=10000).map(|i| format!("{}\n", i)).collect();
        let result = truncate_tail(&content, 2000, 50000);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, "lines");
        assert_eq!(result.output_lines, 2000);
        // Should keep the last 2000 lines
        assert!(
            result.content.starts_with("8001"),
            "starts with: {:?}",
            &result.content[..10]
        );
    }

    #[test]
    fn test_truncate_tail_lines_and_bytes_both_exceeded() {
        // Both limits exceeded - byte limit should win (more restrictive)
        let content: String = (1..=5000)
            .map(|i| format!("line {} {}\n", i, "x".repeat(100)))
            .collect();
        let result = truncate_tail(&content, 2000, 30000);
        assert!(result.truncated);
        // With 100-byte lines, 300 lines would be ~30KB + newlines
        // So byte limit should be hit before line limit
        assert_eq!(result.truncated_by, "bytes");
        assert!(result.output_lines < 2000);
    }
}
