use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use crate::tui::visual_truncate::truncate_to_visual_lines;
use anyhow::Context;
use async_trait::async_trait;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::{Mutex as TokioMutex, mpsc::UnboundedSender};

// ── BashOperations (pluggable) ────────────────────────────────────

/// Pluggable operations for the bash tool (matching pi's BashOperations).
/// Override these to delegate command execution to remote systems (for example SSH).
#[async_trait]
pub trait BashOperations: Send + Sync {
    /// Execute a command and stream output via the sender.
    /// Returns the exit code (0 = success, non-zero = error, None = killed).
    async fn exec(
        &self,
        command: &str,
        cwd: &Path,
        on_data: UnboundedSender<String>,
        signal: Option<&Cancel>,
        timeout: Option<u64>,
        env: Option<HashMap<String, String>>,
    ) -> Result<Option<i32>, anyhow::Error>;
}

#[derive(Clone, Default)]
pub struct BashToolOptions {
    /// Custom operations for command execution. Default: local shell.
    pub operations: Option<Arc<dyn BashOperations>>,
    /// Command prefix prepended to every command (for example shell setup commands).
    pub command_prefix: Option<String>,
    /// Optional explicit shell path from settings.
    pub shell_path: Option<String>,
}

pub struct BashExtension {
    cwd: PathBuf,
    options: BashToolOptions,
}

impl BashExtension {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            options: BashToolOptions::default(),
        }
    }

    pub fn with_options(cwd: PathBuf, options: BashToolOptions) -> Self {
        Self { cwd, options }
    }

    pub fn with_shell_path(cwd: PathBuf, shell_path: String) -> Self {
        Self {
            cwd,
            options: BashToolOptions {
                shell_path: Some(shell_path),
                ..BashToolOptions::default()
            },
        }
    }
}

impl Extension for BashExtension {
    fn name(&self) -> Cow<'static, str> {
        "bash".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(BashTool {
            cwd: self.cwd.clone(),
            shell_path: self.options.shell_path.clone(),
            command_prefix: self.options.command_prefix.clone(),
            operations: self.options.operations.clone(),
        })]
    }
}

struct BashTool {
    cwd: PathBuf,
    shell_path: Option<String>,
    command_prefix: Option<String>,
    operations: Option<Arc<dyn BashOperations>>,
}

// ── Constants ────────────────────────────────────────────────────

const DEFAULT_MAX_LINES: usize = 2000;
const DEFAULT_MAX_BYTES: usize = 50 * 1024; // 50KB
const BASH_TEMP_FILE_PREFIX: &str = "pi-bash";

/// Grace period after child exit (ms) — matching pi's EXIT_STDIO_GRACE_MS.
/// Detached descendants may keep stdout/stderr pipes open; we poll until idle.
const EXIT_STDIO_GRACE_MS: u64 = 100;

// ── Shell resolution (matching pi's getShellConfig) ──────────────

/// Shell configuration: which shell binary to use and how to pass commands.
struct ShellConfig {
    shell: String,
    args: Vec<String>,
}

/// Resolve the shell to use for command execution.
/// Resolution order (matching pi):
/// 1. User-specified shell_path (from BashTool.shell_path)
/// 2. On Unix: /bin/bash, then bash on PATH, then fallback to sh
/// 3. On Windows: Git Bash, bash on PATH, fallback to sh
fn resolve_shell(shell_path: Option<&str>) -> ShellConfig {
    if let Some(path) = shell_path {
        return ShellConfig {
            shell: path.to_string(),
            args: vec!["-c".to_string()],
        };
    }

    // Try /bin/bash first (most common on Unix)
    if std::path::Path::new("/bin/bash").exists() {
        return ShellConfig {
            shell: "/bin/bash".to_string(),
            args: vec!["-c".to_string()],
        };
    }

    // Try `which bash`
    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("which")
            .arg("bash")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() && std::path::Path::new(&path).exists() {
                return ShellConfig {
                    shell: path,
                    args: vec!["-c".to_string()],
                };
            }
        }
    }

    // Fallback to sh
    ShellConfig {
        shell: "sh".to_string(),
        args: vec!["-c".to_string()],
    }
}

// ── Helpers ──────────────────────────────────────────────────────

/// Kill a process group by its leader PID.
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    if pid > 0 {
        let _ = std::process::Command::new("kill")
            .arg("--")
            .arg(format!("-{}", pid))
            .status();
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
    shell_path: Option<&str>,
) -> std::io::Result<tokio::process::Child> {
    let shell_cfg = resolve_shell(shell_path);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut std_cmd = std::process::Command::new(&shell_cfg.shell);
        std_cmd.args(&shell_cfg.args).arg(command).current_dir(cwd);
        unsafe {
            std_cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        let mut tokio_cmd = tokio::process::Command::from(std_cmd);
        tokio_cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    }
    #[cfg(not(unix))]
    {
        tokio::process::Command::new(&shell_cfg.shell)
            .args(&shell_cfg.args)
            .arg(command)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    }
}

/// Sanitize binary output for display/storage (matching pi's sanitizeBinaryOutput + stripAnsi).
fn sanitize_output(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_escape = false;
    for c in text.chars() {
        if in_escape {
            if c == '\x1b' || c == '\u{9b}' {
                continue;
            }
            if c.is_ascii_alphabetic() || c == '~' {
                in_escape = false;
            }
            continue;
        }
        if c == '\x1b' || c == '\u{9b}' {
            in_escape = true;
            continue;
        }
        let code = c as u32;
        if code <= 0x1f && code != 0x09 && code != 0x0a && code != 0x0d {
            continue;
        }
        if (0xfff9..=0xfffb).contains(&code) {
            continue;
        }
        result.push(c);
    }
    result
}

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
    truncated: bool,
    total_lines: usize,
    output_lines: usize,
    output_bytes: usize,
    truncated_by: &'static str,
    last_line_partial: bool,
}

/// Truncate content from the tail, keeping complete lines that fit within limits.
fn truncate_tail(content: &str, max_lines: usize, max_bytes: usize) -> TailTruncation {
    let total_bytes = content.len();
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

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

    let mut output: Vec<&str> = Vec::new();
    let mut byte_count: usize = 0;
    let mut truncated_by = "lines";
    let mut last_line_partial = false;

    for line in lines.iter().rev().take(max_lines) {
        let line_bytes = line.len();
        let with_newline = if output.is_empty() {
            line_bytes
        } else {
            line_bytes + 1
        };

        if byte_count + with_newline > max_bytes {
            truncated_by = "bytes";
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

// ── Result formatting ────────────────────────────────────────────

fn finish_bash_execution(
    combined: &str,
    exit_code: i32,
    cancelled: bool,
    timed_out: Option<u64>,
    on_update: Option<UnboundedSender<ToolOutput>>,
) -> Result<ToolOutput, anyhow::Error> {
    let trunc = truncate_tail(combined, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);

    let mut result_text = if trunc.content.is_empty() {
        "(no output)".to_string()
    } else {
        trunc.content.clone()
    };

    // Save full output to temp file if truncated
    let full_output_path = if trunc.truncated {
        let tmp_dir = std::env::temp_dir().join(BASH_TEMP_FILE_PREFIX);
        let _ = std::fs::create_dir_all(&tmp_dir);
        let tmp_path = tmp_dir.join(format!("{}.log", uuid::Uuid::new_v4()));
        let saved = std::fs::write(&tmp_path, combined).ok().map(|_| tmp_path);

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
        saved
    } else {
        None
    };

    // Build structured details
    let details = if trunc.truncated || full_output_path.is_some() {
        Some(serde_json::json!({
            "truncation": {
                "truncated": trunc.truncated,
                "truncatedBy": trunc.truncated_by,
                "totalLines": trunc.total_lines,
                "outputLines": trunc.output_lines,
                "outputBytes": trunc.output_bytes,
                "lastLinePartial": trunc.last_line_partial,
                "maxLines": DEFAULT_MAX_LINES,
                "maxBytes": DEFAULT_MAX_BYTES,
            },
            "fullOutputPath": full_output_path.as_ref().map(|p| p.display().to_string()),
        }))
    } else {
        None
    };

    let final_output = if cancelled {
        if result_text.is_empty() || result_text == "(no output)" {
            "Command aborted".to_string()
        } else {
            format!("{}\n\nCommand aborted", result_text)
        }
    } else if let Some(secs) = timed_out {
        if result_text.is_empty() || result_text == "(no output)" {
            format!("Command timed out after {} seconds", secs)
        } else {
            format!(
                "{}\n\nCommand timed out after {} seconds",
                result_text, secs
            )
        }
    } else if exit_code != 0 {
        if result_text.is_empty() || result_text == "(no output)" {
            format!("Command exited with code {}", exit_code)
        } else {
            format!("{}\n\nCommand exited with code {}", result_text, exit_code)
        }
    } else {
        if let Some(ref tx) = on_update {
            let _ = tx.send(ToolOutput::ok_with_details(
                result_text.clone(),
                details.clone().unwrap_or_default(),
            ));
        }
        return Ok(ToolOutput::ok_with_details(
            result_text,
            details.unwrap_or_default(),
        ));
    };

    if let Some(ref tx) = on_update {
        let _ = tx.send(ToolOutput::ok_with_details(
            final_output.clone(),
            details.clone().unwrap_or_default(),
        ));
    }

    Err(anyhow::anyhow!("{}", final_output))
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

    fn prompt_snippet(&self) -> Option<Cow<'static, str>> {
        Some("Execute bash commands (ls, grep, find, etc.)".into())
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
        let timeout = args["timeout"].as_u64();
        let started_at = Instant::now();

        cancel.check()?;

        // Apply command prefix if set
        let effective_command = if let Some(ref prefix) = self.command_prefix {
            format!("{}\n{}", prefix, command)
        } else {
            command.to_string()
        };

        // Check that the working directory exists (matching pi's fsAccess check)
        if !self.cwd.exists() {
            anyhow::bail!(
                "Working directory does not exist: {}\nCannot execute bash commands.",
                self.cwd.display()
            );
        }

        // If custom operations are provided, delegate entirely
        if let Some(ref ops) = self.operations {
            let (output_tx, mut output_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let ops_cancel = cancel.clone();

            // Spawn the operations exec in background
            let ops_command = effective_command.clone();
            let ops_cwd = self.cwd.clone();
            let ops = ops.clone();
            let ops_handle = tokio::spawn(async move {
                ops.exec(
                    &ops_command,
                    &ops_cwd,
                    output_tx,
                    Some(&ops_cancel),
                    timeout,
                    None,
                )
                .await
            });

            // Collect output from the channel
            let mut combined = String::new();
            while let Some(chunk) = output_rx.recv().await {
                combined.push_str(&chunk);
                // Stream partial output to on_update
                if let Some(ref tx) = on_update {
                    let _ = tx.send(ToolOutput::ok(combined.clone()));
                }
            }

            let exit_code = ops_handle.await.unwrap_or(Ok(None)).unwrap_or(None);
            let code = exit_code.unwrap_or(-1);

            return finish_bash_execution(&combined, code, cancel.is_cancelled(), None, on_update);
        }

        let mut child =
            spawn_bash_command(&effective_command, &self.cwd, self.shell_path.as_deref())
                .with_context(|| format!("Failed to spawn command: {}", effective_command))?;

        let pid = child.id().unwrap_or(0);

        // Shared output buffer for streaming reads
        let combined = Arc::new(TokioMutex::new(String::new()));
        let combined_clone = combined.clone();

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
            let mut stdout_buf = vec![0u8; 65536];
            let mut stderr_buf = vec![0u8; 65536];
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
                                let text = String::from_utf8_lossy(&stdout_buf[..n]);
                                let sanitized = sanitize_output(&text);
                                let mut out = combined_clone.lock().await;
                                out.push_str(&sanitized);
                            }
                            Err(_) => stdout_done = true,
                        }
                    }
                    result = stderr_reader.read(&mut stderr_buf), if !stderr_done => {
                        match result {
                            Ok(0) => stderr_done = true,
                            Ok(n) => {
                                let text = String::from_utf8_lossy(&stderr_buf[..n]);
                                let sanitized = sanitize_output(&text);
                                let mut out = combined_clone.lock().await;
                                out.push_str(&sanitized);
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

        // ── PID tracking for cleanup on shutdown signals ──
        // Register this PID so it gets killed if the parent exits unexpectedly
        let _pid_guard = ProcessGuard::new(pid);

        // Set up cancellation monitor: kill the process group if cancelled
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_flag = cancelled.clone();
        let cancel_inner = cancel.clone();
        let _cancel_monitor: tokio::task::JoinHandle<()> = tokio::spawn(async move {
            while !cancel_inner.is_cancelled() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            cancel_flag.store(true, Ordering::SeqCst);
            kill_process_group(pid);
        });

        // Send initial empty update (matching pi's onUpdate({ content: [], details: undefined }))
        if let Some(ref tx) = on_update {
            let _ = tx.send(ToolOutput::ok(String::new()));
        }

        // Wait for the process to exit, with optional timeout and streaming updates
        let timeout_dur = timeout.map(std::time::Duration::from_secs);
        let throttle_ms = 100u64;
        let mut last_update_at = Instant::now();

        let exit_code: i32;

        loop {
            if cancelled.load(Ordering::SeqCst) {
                kill_process_group(pid);
                read_task.abort();
                let combined_str = combined.lock().await.clone();
                return finish_bash_execution(&combined_str, -1, true, None, on_update);
            }

            if let Some(dur) = timeout_dur
                && started_at.elapsed() > dur
            {
                kill_process_group(pid);
                read_task.abort();
                let combined_str = combined.lock().await.clone();
                return finish_bash_execution(&combined_str, -1, false, timeout, on_update);
            }

            if let Some(ref tx) = on_update
                && last_update_at.elapsed().as_millis() as u64 >= throttle_ms
            {
                let out = combined.lock().await.clone();
                if !out.is_empty() {
                    last_update_at = Instant::now();
                    let _ = tx.send(ToolOutput::ok(out));
                }
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    exit_code = status.code().unwrap_or(-1);
                    // ── Idle grace period (matching pi's waitForChildProcess) ──
                    // After child exit, wait for pipes to go idle (late data from
                    // detached descendants). Poll every 100ms; if no new data
                    // within the grace window, the output is stable.
                    let mut last_len = combined.lock().await.len();
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(EXIT_STDIO_GRACE_MS))
                            .await;
                        let new_len = combined.lock().await.len();
                        if new_len == last_len {
                            break;
                        }
                        last_len = new_len;
                    }
                    read_task.abort();
                    break;
                }
                Ok(None) => {
                    tokio::time::sleep(std::time::Duration::from_millis(throttle_ms)).await;
                }
                Err(_) => {
                    read_task.await.ok();
                    exit_code = -1;
                    break;
                }
            }
        }

        let combined_str = combined.lock().await.clone();
        if let Some(ref tx) = on_update
            && !combined_str.is_empty()
        {
            let _ = tx.send(ToolOutput::ok(combined_str.clone()));
        }

        finish_bash_execution(&combined_str, exit_code, false, None, on_update)
    }
}

// ── Bash tool renderer ───────────────────────────────────────────

struct BashRenderer;

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

        let clean = strip_context_truncation_footer(content)
            .trim_end()
            .to_string();
        let all_lines: Vec<&str> = clean.lines().collect();

        if all_lines.is_empty() || (all_lines.len() == 1 && all_lines[0].is_empty()) {
            return lines;
        }

        let preview_count = 5;
        let (preview_lines, hidden_line_count) = if ctx.expanded {
            (all_lines.clone(), 0)
        } else {
            truncate_to_visual_lines(&all_lines, width, preview_count)
        };

        // ── Preview hint with dim/muted styling (matching pi's keyHint) ──
        if !ctx.expanded && hidden_line_count > 0 {
            if ctx.expand_key.is_empty() {
                lines.push(theme.fg_key(
                    ThemeKey::Muted,
                    &format!("... {} earlier lines", hidden_line_count),
                ));
            } else {
                // Pi pattern: muted prefix + dim key + muted suffix
                // e.g. "... (12 earlier lines, \x1b[2mctrl+o\x1b[22m to expand)"
                let prefix = theme.fg_key(
                    ThemeKey::Muted,
                    &format!("... ({} earlier lines, ", hidden_line_count),
                );
                let key_styled = theme.fg("dim", &ctx.expand_key);
                let suffix = theme.fg_key(ThemeKey::Muted, " to expand)");
                lines.push(format!("{}{}{}", prefix, key_styled, suffix));
            }
        }

        let fg_key = if ctx.is_error { "error" } else { "toolOutput" };
        for line in &preview_lines {
            if line.is_empty() {
                lines.push(String::new());
            } else {
                lines.push(theme.fg(fg_key, line));
            }
        }

        if let Some(secs) = ctx.duration_secs {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            let is_complete = ctx.exit_code.is_some() || ctx.cancelled;
            let label = if is_complete { "Took" } else { "Elapsed" };
            lines.push(theme.fg_key(ThemeKey::Muted, &format!("{} {:.1}s", label, secs)));
        }

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

// ── PID tracking for cleanup on shutdown signals ────────────────
// Matching pi's trackDetachedChildPid / untrackDetachedChildPid.
// On SIGTERM/SIGHUP, all tracked PIDs are killed before exit.

use std::sync::Mutex;

static TRACKED_PIDS: Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

fn track_pid(pid: u32) {
    if let Ok(mut pids) = TRACKED_PIDS.lock() {
        pids.push(pid);
    }
}

fn untrack_pid(pid: u32) {
    if let Ok(mut pids) = TRACKED_PIDS.lock() {
        pids.retain(|&p| p != pid);
    }
}

/// Kill all tracked child process groups. Called on SIGTERM/SIGHUP.
pub fn kill_tracked_children() {
    let pids: Vec<u32> = TRACKED_PIDS.lock().map(|p| p.clone()).unwrap_or_default();
    for pid in pids {
        kill_process_group(pid);
    }
}

struct ProcessGuard {
    pid: u32,
}

impl ProcessGuard {
    fn new(pid: u32) -> Self {
        if pid > 0 {
            track_pid(pid);
        }
        Self { pid }
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if self.pid > 0 {
            untrack_pid(self.pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool() -> BashTool {
        BashTool {
            cwd: std::env::temp_dir(),
            shell_path: None,
            command_prefix: None,
            operations: None,
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

        let tool = BashTool {
            cwd: tmp.clone(),
            shell_path: None,
            command_prefix: None,
            operations: None,
        };
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
        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"command": "echo start && sleep 10 && echo end", "timeout": 1}),
                Cancel::new(),
                None,
            )
            .await;
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

    #[test]
    fn test_truncate_tail_exact_line_fit() {
        let lines: String = (1..=2000).map(|i| format!("line {}\n", i)).collect();
        let result = truncate_tail(&lines, 2000, 50000);
        assert!(!result.truncated);
        assert!(result.content.lines().count() == 2000);
    }

    #[test]
    fn test_truncate_tail_one_over_line_limit() {
        let lines: String = (1..=2001).map(|i| format!("line {}\n", i)).collect();
        let result = truncate_tail(&lines, 2000, 50000);
        assert!(result.truncated);
        assert_eq!(result.content.lines().count(), 2000);
        assert!(result.content.starts_with("line 2"));
    }

    #[test]
    fn test_truncate_tail_exact_byte_fit() {
        let line = "a".repeat(50000);
        let result = truncate_tail(&line, 2000, 50000);
        assert!(!result.truncated);
    }

    #[test]
    fn test_truncate_tail_one_byte_over() {
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
        assert_eq!(result.content.len(), 50000);
        assert!(result.content.ends_with("x".repeat(50000).as_str()));
    }

    #[test]
    fn test_truncate_tail_byte_count_respects_newlines() {
        let content: String = (1..=100)
            .map(|i| format!("line {} {}\n", i, "x".repeat(1000)))
            .collect();
        let result = truncate_tail(&content, 2000, 50000);
        assert!(result.truncated);
        assert!(result.output_bytes <= 50000);
    }

    #[tokio::test]
    async fn truncated_by_lines_shows_footer() {
        let tool = make_tool();
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
        assert!(!output.content.contains("Output truncated"));
        assert!(!output.content.contains("Full output:"));
    }

    #[tokio::test]
    async fn truncated_saves_temp_file() {
        let tool = make_tool();
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
            output.content.contains("/pi-bash/"),
            "expected temp file path with /pi-bash/, got: {}",
            output.content
        );
    }

    #[test]
    fn test_truncate_tail_many_short_lines() {
        let content: String = (1..=10000).map(|i| format!("{}\n", i)).collect();
        let result = truncate_tail(&content, 2000, 50000);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, "lines");
        assert_eq!(result.output_lines, 2000);
        assert!(
            result.content.starts_with("8001"),
            "starts with: {:?}",
            &result.content[..10]
        );
    }

    #[test]
    fn test_truncate_tail_lines_and_bytes_both_exceeded() {
        let content: String = (1..=5000)
            .map(|i| format!("line {} {}\n", i, "x".repeat(100)))
            .collect();
        let result = truncate_tail(&content, 2000, 30000);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, "bytes");
        assert!(result.output_lines < 2000);
    }

    // ── ProcessGuard tests ──────────────────────────────────────

    #[test]
    fn test_process_guard_tracks_pid() {
        let pid = 12345u32;
        {
            let _guard = ProcessGuard::new(pid);
            let pids = TRACKED_PIDS.lock().unwrap();
            assert!(pids.contains(&pid));
        }
        let pids = TRACKED_PIDS.lock().unwrap();
        assert!(!pids.contains(&pid));
    }

    #[test]
    fn test_process_guard_zero_pid() {
        {
            let _guard = ProcessGuard::new(0);
            let pids = TRACKED_PIDS.lock().unwrap();
            assert!(!pids.contains(&0));
        }
    }
}
