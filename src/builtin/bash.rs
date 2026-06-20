use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use anyhow::Context;
use async_trait::async_trait;
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

// ── Helpers ──────────────────────────────────────────────────────

/// Spawn a bash command with process group setup for clean cancellation.
fn spawn_bash_command(command: &str, cwd: &std::path::Path) -> std::io::Result<tokio::process::Child> {
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

/// Format bytes as a human-readable size string.
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
    content: String,
    total_lines: usize,
    output_lines: usize,
    output_bytes: usize,
    truncated: bool,
    truncated_by: &'static str, // "lines" | "bytes"
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
            total_lines,
            output_lines: total_lines,
            output_bytes: total_bytes,
            truncated: false,
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
        total_lines,
        output_lines: output.len(),
        output_bytes: byte_count,
        truncated: true,
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

    fn label(&self) -> &str {
        "Execute bash commands"
    }

    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
        cancel: Cancel,
    ) -> anyhow::Result<ToolOutput> {
        let _ = tool_call_id;
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;
        let timeout = args["timeout"].as_u64();

        cancel.check()?;

        // Build the command with process group setup for process-tree killing
        let child = spawn_bash_command(command, &self.cwd)
            .with_context(|| format!("Failed to spawn command: {}", command))?;

        let pid = child.id().unwrap_or(0);

        // Set up cancellation monitor: kill the process group if cancelled
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_clone = cancelled.clone();
        let cancel_monitor: tokio::task::JoinHandle<()> = tokio::spawn(async move {
            while !cancel.is_cancelled() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            cancel_clone.store(true, Ordering::SeqCst);
            // Kill the process group
            #[cfg(unix)]
            {
                if pid > 0 {
                    let _ = std::process::Command::new("kill")
                        .arg("--")
                        .arg(format!("-{}", pid))
                        .spawn();
                }
            }
            #[cfg(not(unix))]
            {
                let _ = pid;
                let _ = child.start_kill();
            }
        });

        // Read output with optional timeout
        let output_result = if let Some(secs) = timeout {
            tokio::time::timeout(
                std::time::Duration::from_secs(secs),
                child.wait_with_output(),
            )
            .await
        } else {
            // No timeout, but still need to race against cancellation
            tokio::time::timeout(
                std::time::Duration::from_secs(86400), // 24h effective no-timeout
                child.wait_with_output(),
            )
            .await
        };

        cancel_monitor.abort();

        // If cancelled, also try to kill the process
        if cancelled.load(Ordering::SeqCst) {
            #[cfg(unix)]
            {
                if pid > 0 {
                    let _ = std::process::Command::new("kill")
                        .arg("--")
                        .arg(format!("-{}", pid))
                        .spawn();
                }
            }
            return Err(anyhow::anyhow!("Command aborted"));
        }

        let output = match output_result {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => {
                // Timeout — kill the process group
                #[cfg(unix)]
                {
                    if pid > 0 {
                        let _ = std::process::Command::new("kill")
                            .arg("--")
                            .arg(format!("-{}", pid))
                            .spawn();
                    }
                }
                return Err(anyhow::anyhow!(
                    "Command timed out after {} seconds",
                    timeout.unwrap_or(0)
                ));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        // Apply tail truncation (pi-style: keep last N lines/bytes)
        let trunc = truncate_tail(&combined, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);

        let mut result_text = trunc.content;

        // Save full output to temp file if truncated (as advertised in the description)
        let full_output_path = if trunc.truncated {
            let tmp_dir = std::env::temp_dir().join("rab-bash");
            std::fs::create_dir_all(&tmp_dir).ok();
            let tmp_path = tmp_dir.join(format!("{}.txt", uuid::Uuid::new_v4()));
            if std::fs::write(&tmp_path, &combined).is_ok() {
                Some(tmp_path)
            } else {
                None
            }
        } else {
            None
        };

        // Build continuation/footer messages
        if trunc.truncated {
            let start_line = trunc.total_lines - trunc.output_lines + 1;
            let end_line = trunc.total_lines;

            let footer = if let Some(ref path) = full_output_path {
                if trunc.last_line_partial {
                    let last_line_size = format_size(
                        combined.lines().last().map(|l| l.len()).unwrap_or(0)
                    );
                    format!(
                        "\n\n[Showing last {} of line {} (line is {}). Full output: {}]",
                        format_size(trunc.output_bytes),
                        end_line,
                        last_line_size,
                        path.display()
                    )
                } else if trunc.truncated_by == "lines" {
                    format!(
                        "\n\n[Showing lines {}-{} of {}. Full output: {}]",
                        start_line,
                        end_line,
                        trunc.total_lines,
                        path.display()
                    )
                } else {
                    format!(
                        "\n\n[Showing lines {}-{} of {} ({} limit). Full output: {}]",
                        start_line,
                        end_line,
                        trunc.total_lines,
                        format_size(DEFAULT_MAX_BYTES),
                        path.display()
                    )
                }
            } else {
                if trunc.last_line_partial {
                    format!(
                        "\n\n[Showing last {} of line {}.]",
                        format_size(trunc.output_bytes),
                        end_line,
                    )
                } else if trunc.truncated_by == "lines" {
                    format!(
                        "\n\n[Showing lines {}-{} of {}.]",
                        start_line, end_line, trunc.total_lines,
                    )
                } else {
                    format!(
                        "\n\n[Showing lines {}-{} of {} ({} limit).]",
                        start_line,
                        end_line,
                        trunc.total_lines,
                        format_size(DEFAULT_MAX_BYTES),
                    )
                }
            };
            result_text.push_str(&footer);
        }

        // Add exit code info
        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            if result_text.is_empty() {
                return Ok(ToolOutput::ok(format!("Command exited with code {}", code)));
            }
            result_text.push_str(&format!("\n\n[Command exited with code {}]", code));
        } else if result_text.is_empty() {
            result_text = "(no output)".to_string();
        }

        Ok(ToolOutput::ok(result_text))
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
        assert_eq!(result.truncated_by, "lines");
        assert_eq!(result.output_lines, 2000);
        assert!(result.content.starts_with("line 3001"));
    }

    #[test]
    fn test_truncate_tail_by_bytes() {
        let content: String = (1..=100)
            .map(|i| format!("line {} {}\n", i, "x".repeat(1000)))
            .collect();
        let result = truncate_tail(&content, 2000, 50000);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, "bytes");
        assert!(result.output_lines < 100);
    }

    #[test]
    fn test_truncate_tail_partial_last_line() {
        // A single line that exceeds the byte limit
        let content = format!("short\n{}\n", "x".repeat(60000));
        let result = truncate_tail(&content, 2000, 50000);
        assert!(result.truncated);
        assert!(result.last_line_partial);
        // Should contain the end of the long line
        assert!(result.content.len() <= 50000);
    }

    #[test]
    fn test_truncate_tail_empty() {
        let result = truncate_tail("", 2000, 50000);
        assert!(!result.truncated);
        assert_eq!(result.content, "");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500B");
        assert_eq!(format_size(50 * 1024), "50.0KB");
    }
}
