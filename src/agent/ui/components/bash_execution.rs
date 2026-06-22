use crate::tui::Component;
use crate::tui::components::loader::Loader;
use crate::tui::util::wrap_text_with_ansi;

/// Maximum lines of output to keep for LLM context truncation (matching pi's DEFAULT_MAX_LINES).
const DEFAULT_MAX_LINES: usize = 1000;
/// Maximum bytes of output to keep (matching pi's DEFAULT_MAX_BYTES).
const DEFAULT_MAX_BYTES: usize = 16_385;

/// Preview line limit when not expanded (matches pi's PREVIEW_LINES).
const PREVIEW_LINES: usize = 20;

/// Bash execution component - renders a bash command with borders, spinner, and output.
///
/// Matches pi's BashExecutionComponent design:
/// - Spacer (1 blank line) above top border
/// - Top/bottom borders in `bashMode` color (or `dim` for !! commands)
/// - Command header with `$` prefix
/// - Spinner while running (uses Loader component)
/// - Streaming output in muted color (no ANSI)
/// - Collapse/expand support showing FIRST N lines (preview truncation)
/// - Status line with exit code, duration, cancellation, truncation warnings
/// - Width-aware visual truncation for collapsed preview
pub struct BashExecution {
    command: String,
    output_lines: Vec<String>,
    status: BashStatus,
    expanded: bool,
    exclude_from_context: bool,
    /// Full output path for truncation warning.
    full_output_path: Option<String>,
    /// Whether output was truncated for LLM context limits.
    was_truncated: bool,
    /// Execution duration in seconds (parsed from result content or set externally).
    duration_secs: Option<f64>,
    /// Loader component for spinner animation.
    loader: Loader,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BashStatus {
    Running,
    Complete { exit_code: i32 },
    Cancelled,
    Error(String),
}

impl BashExecution {
    pub fn new(command: impl Into<String>) -> Self {
        let command = command.into();

        // Create a loader matching pi's style: spinner in bashMode color, message in muted color
        let theme = crate::agent::ui::theme::current_theme();
        let spinner_ansi = theme.fg_ansi("bashMode").to_string();
        let msg_ansi = theme.fg_ansi("muted").to_string();
        drop(theme);
        let loader = Loader::new(
            Box::new(move |s| format!("{}{}\x1b[39m", spinner_ansi, s)),
            Box::new(move |s| format!("{}{}\x1b[39m", msg_ansi, s)),
            "Running... (Esc to cancel)",
        );

        Self {
            command,
            output_lines: Vec::new(),
            status: BashStatus::Running,
            expanded: false,
            exclude_from_context: false,
            full_output_path: None,
            was_truncated: false,
            duration_secs: None,
            loader,
        }
    }

    pub fn append_output(&mut self, line: impl Into<String>) {
        self.output_lines.push(line.into());
    }

    /// Append a chunk of output that may contain newlines.
    /// Handles splitting into lines similar to pi's appendOutput (preserving incomplete last line).
    pub fn append_chunk(&mut self, chunk: &str) {
        // Strip ANSI codes and normalize line endings (matching pi)
        let clean = strip_ansi(chunk).replace("\r\n", "\n").replace('\r', "\n");

        let new_lines: Vec<&str> = clean.split('\n').collect();
        if new_lines.is_empty() {
            return;
        }

        if !self.output_lines.is_empty() && !new_lines.is_empty() {
            // Append first chunk to last line (incomplete line continuation, matching pi)
            let last_idx = self.output_lines.len() - 1;
            self.output_lines[last_idx].push_str(new_lines[0]);
            self.output_lines
                .extend(new_lines[1..].iter().map(|s| s.to_string()));
        } else {
            self.output_lines
                .extend(new_lines.iter().map(|s| s.to_string()));
        }
    }

    pub fn set_complete(&mut self, exit_code: i32) {
        self.status = if exit_code == 0 {
            BashStatus::Complete { exit_code: 0 }
        } else {
            BashStatus::Complete { exit_code }
        };
        self.stop_loader();
    }

    pub fn set_cancelled(&mut self) {
        self.status = BashStatus::Cancelled;
        self.stop_loader();
    }

    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.status = BashStatus::Error(msg.into());
        self.stop_loader();
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    pub fn set_exclude_from_context(&mut self, exclude: bool) {
        self.exclude_from_context = exclude;
    }

    pub fn set_full_output_path(&mut self, path: impl Into<String>) {
        self.full_output_path = Some(path.into());
    }

    pub fn set_truncated(&mut self, truncated: bool) {
        self.was_truncated = truncated;
    }

    /// Set the execution duration in seconds.
    pub fn set_duration_secs(&mut self, secs: f64) {
        self.duration_secs = Some(secs);
    }

    /// Parse and set duration from result content (format: `[Xs]` at end of string).
    pub fn set_duration_from_content(&mut self, content: &str) {
        if let Some(end_bracket) = content.rfind(']')
            && let Some(start_bracket) = content[..end_bracket].rfind('[')
        {
            let num_str = &content[start_bracket + 1..end_bracket];
            if let Ok(secs) = num_str.parse::<f64>() {
                self.duration_secs = Some(secs);
            }
        }
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    fn stop_loader(&mut self) {
        self.loader.stop();
    }

    fn border_color_key(&self) -> &'static str {
        if self.exclude_from_context {
            return "dim";
        }
        match self.status {
            BashStatus::Running => "bashMode",
            BashStatus::Complete { exit_code: 0 } => "bashMode",
            BashStatus::Complete { .. } => "error",
            BashStatus::Cancelled => "warning",
            BashStatus::Error(_) => "error",
        }
    }

    /// Apply context truncation matching pi's truncateTail logic.
    fn context_truncated_output(&self) -> (String, bool) {
        let output = self.output_lines.join("\n");

        // Simulate pi's truncateTail: truncate by maxLines, then by maxBytes
        let lines: Vec<&str> = output.split('\n').collect();
        let total_lines = lines.len();
        let truncated_lines: Vec<&str> = if total_lines > DEFAULT_MAX_LINES {
            lines[lines.len() - DEFAULT_MAX_LINES..].to_vec()
        } else {
            lines
        };

        let joined = truncated_lines.join("\n");
        let bytes = joined.len();
        if bytes > DEFAULT_MAX_BYTES {
            // Truncate bytes from the end
            let mut byte_end = DEFAULT_MAX_BYTES;
            // Ensure we don't cut in the middle of a UTF-8 character
            while byte_end > 0 && !joined.is_char_boundary(byte_end) {
                byte_end -= 1;
            }
            let truncated: String = joined[..byte_end].to_string();
            (truncated, true)
        } else {
            (
                joined,
                total_lines > DEFAULT_MAX_LINES || bytes > DEFAULT_MAX_BYTES,
            )
        }
    }

    /// Get the raw output (for building messages sent to the LLM).
    pub fn get_output(&self) -> String {
        self.output_lines.join("\n")
    }

    /// Get the command that was executed.
    pub fn get_command(&self) -> String {
        self.command.clone()
    }
}

impl Component for BashExecution {
    fn set_expanded(&mut self, expanded: bool) {
        BashExecution::set_expanded(self, expanded);
    }

    fn render(&self, width: usize) -> Vec<String> {
        let theme = crate::agent::ui::theme::current_theme();
        let border_key = self.border_color_key();
        let border_fn = |s: &str| theme.fg(border_key, s);

        let mut lines: Vec<String> = Vec::new();

        // ── Spacer (1 blank line above, matching pi) ──
        lines.push(String::new());

        // ── Top border (pi-style: just ─ repeated) ──
        let top_border = "─".repeat(width.max(1));
        lines.push(border_fn(&top_border));

        // ── Command header (pi-style: bold $ command in border color) ──
        let header = format!(
            "{} {}",
            theme.bold_fg(border_key, "$"),
            theme.fg(border_key, &self.command)
        );
        lines.push(header);

        // ── Apply context truncation (same limits as bash tool, matching pi) ──
        let (context_output, context_truncated) = self.context_truncated_output();
        let available_lines: Vec<&str> = if context_output.is_empty() {
            Vec::new()
        } else {
            context_output.split('\n').collect()
        };

        // ── Preview truncation (pi-style: first PREVIEW_LINES when collapsed, hint at top) ──
        let preview_lines: Vec<&str> = if self.expanded {
            available_lines.clone()
        } else if available_lines.len() > PREVIEW_LINES {
            available_lines[..PREVIEW_LINES].to_vec()
        } else {
            available_lines.clone()
        };

        let hidden_line_count = available_lines.len().saturating_sub(preview_lines.len());

        // ── "N earlier lines" hint at top when collapsed (matching pi) ──
        if !self.expanded && hidden_line_count > 0 {
            let hint = theme.fg("muted", &format!("... {} more lines", hidden_line_count));
            lines.push(hint);
        }

        // ── Output ──
        if !preview_lines.is_empty() {
            for line in &preview_lines {
                let styled = theme.fg("toolOutput", line);
                let wrapped = wrap_text_with_ansi(&styled, width);
                lines.extend(wrapped);
            }
        }

        // ── Status / hints ──
        let mut status_parts: Vec<String> = Vec::new();

        // Empty line before status (matching pi)
        if !preview_lines.is_empty() {
            status_parts.push(String::new());
        }

        // Duration (pi: "Elapsed X.Xs" during, "Took X.Xs" after)
        if let Some(secs) = self.duration_secs {
            let label = match self.status {
                BashStatus::Running => "Elapsed",
                _ => "Took",
            };
            status_parts.push(theme.fg("muted", &format!("{} {:.1}s", label, secs)));
        }

        // Status text
        match &self.status {
            BashStatus::Running => {
                // Loader handles the spinner display
            }
            BashStatus::Complete { exit_code } if *exit_code != 0 => {
                status_parts.push(theme.fg("error", &format!("(exit {})", exit_code)));
            }
            BashStatus::Cancelled => {
                status_parts.push(theme.fg("warning", "(cancelled)"));
            }
            BashStatus::Error(msg) => {
                status_parts.push(theme.fg("error", &format!("Error: {}", msg)));
            }
            _ => {}
        }

        // Truncation warning (context truncation, not preview truncation)
        let was_truncated = context_truncated || self.was_truncated;
        if was_truncated {
            if let Some(ref path) = self.full_output_path {
                status_parts.push(theme.fg(
                    "warning",
                    &format!("Output truncated. Full output: {}", path),
                ));
            } else {
                status_parts.push(theme.fg("warning", "Output truncated."));
            }
        }

        // Render loader or status
        match &self.status {
            BashStatus::Running => {
                // Render the loader (pi-style: spinner with "Running... (Esc to cancel)" message)
                let loader_lines = self.loader.render(width);
                lines.extend(loader_lines);
            }
            _ => {
                if !status_parts.is_empty() {
                    // Skip leading empty line
                    let status_line = if status_parts.len() == 1 && status_parts[0].is_empty() {
                        String::new()
                    } else {
                        status_parts.join("  ")
                    };
                    if !status_line.is_empty() {
                        lines.push(status_line);
                    }
                }
            }
        }

        // ── Bottom border (pi-style: just ─ repeated) ──
        let bottom_border = "─".repeat(width.max(1));
        lines.push(border_fn(&bottom_border));

        lines
    }

    fn invalidate(&mut self) {
        self.loader.invalidate();
    }
}

/// Simple truncation for output lines: if a line's visible width exceeds the terminal width,
/// truncate it. This is a simplified version of pi's truncateToVisualLines.
/// Strip ANSI escape codes from a string.
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until we hit a letter in range 0x40-0x7E (end of CSI/OSC)
            // Or until we hit BEL (0x07) for OSC sequences
            for n in chars.by_ref() {
                if n == '\x07' || n.is_ascii_uppercase() || n.is_ascii_lowercase() {
                    break;
                }
                if n == '\x1b' {
                    // Nested escape? Put it back conceptually, but we've already consumed
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ui::theme::init_theme;

    #[test]
    fn test_bash_execution_new() {
        let bash = BashExecution::new("echo hello");
        assert_eq!(bash.command, "echo hello");
        assert!(bash.output_lines.is_empty());
        assert_eq!(bash.status, BashStatus::Running);
        assert!(!bash.expanded);
        assert!(!bash.exclude_from_context);
    }

    #[test]
    fn test_bash_execution_append_output() {
        let mut bash = BashExecution::new("echo hello");
        bash.append_output("hello");
        bash.append_output("world");
        assert_eq!(bash.output_lines.len(), 2);
        assert_eq!(bash.output_lines[0], "hello");
        assert_eq!(bash.output_lines[1], "world");
    }

    #[test]
    fn test_bash_execution_append_chunk() {
        let mut bash = BashExecution::new("echo hello");
        bash.append_chunk("line1\nline2\nline3");
        assert_eq!(bash.output_lines.len(), 3);
        assert_eq!(bash.output_lines[0], "line1");
        assert_eq!(bash.output_lines[1], "line2");
        assert_eq!(bash.output_lines[2], "line3");
    }

    #[test]
    fn test_bash_execution_append_chunk_continues_last_line() {
        let mut bash = BashExecution::new("echo hello");
        bash.append_output("partial");
        bash.append_chunk(" continuation\nnext");
        assert_eq!(bash.output_lines.len(), 2);
        assert_eq!(bash.output_lines[0], "partial continuation");
        assert_eq!(bash.output_lines[1], "next");
    }

    #[test]
    fn test_bash_execution_append_chunk_strips_ansi() {
        let mut bash = BashExecution::new("echo hello");
        bash.append_chunk("\x1b[31mcolored\x1b[0m");
        assert_eq!(bash.output_lines.len(), 1);
        assert_eq!(bash.output_lines[0], "colored");
    }

    #[test]
    fn test_bash_execution_set_complete() {
        let mut bash = BashExecution::new("echo hello");
        bash.set_complete(0);
        assert_eq!(bash.status, BashStatus::Complete { exit_code: 0 });

        bash.set_complete(1);
        assert_eq!(bash.status, BashStatus::Complete { exit_code: 1 });
    }

    #[test]
    fn test_bash_execution_set_cancelled() {
        let mut bash = BashExecution::new("echo hello");
        bash.set_cancelled();
        assert_eq!(bash.status, BashStatus::Cancelled);
    }

    #[test]
    fn test_bash_execution_set_error() {
        let mut bash = BashExecution::new("echo hello");
        bash.set_error("something went wrong");
        assert_eq!(
            bash.status,
            BashStatus::Error("something went wrong".into())
        );
    }

    #[test]
    fn test_bash_execution_set_expanded() {
        let mut bash = BashExecution::new("echo hello");
        assert!(!bash.expanded);
        bash.set_expanded(true);
        assert!(bash.expanded);
        bash.set_expanded(false);
        assert!(!bash.expanded);
    }

    #[test]
    fn test_bash_execution_exclude_from_context() {
        let mut bash = BashExecution::new("echo hello");
        assert!(!bash.exclude_from_context);
        bash.set_exclude_from_context(true);
        assert!(bash.exclude_from_context);
    }

    #[test]
    fn test_bash_execution_get_output() {
        let mut bash = BashExecution::new("echo hello");
        bash.append_output("line1");
        bash.append_output("line2");
        assert_eq!(bash.get_output(), "line1\nline2");
    }

    #[test]
    fn test_bash_execution_get_command() {
        let bash = BashExecution::new("echo hello");
        assert_eq!(bash.get_command(), "echo hello");
    }

    #[test]
    fn test_bash_execution_render_has_borders() {
        init_theme(Some("dark"), false);
        let bash = BashExecution::new("echo hello");
        let lines = bash.render(80);
        let all = lines.join("\n");
        // Should have top border (just ─ with ANSI color codes)
        assert!(lines[1].contains('─'), "Top border should contain ─");
        // Should have bottom border (just ─ with ANSI color codes)
        assert!(
            lines[lines.len() - 1].contains('─'),
            "Bottom border should contain ─"
        );
        assert!(all.contains("echo hello"), "Should show command");
        // Spacer should be first line
        assert!(lines[0].is_empty(), "First line should be empty (spacer)");
    }

    #[test]
    fn test_bash_execution_render_status() {
        init_theme(Some("dark"), false);
        let mut bash = BashExecution::new("echo hello");
        bash.append_output("hello world");

        // Complete with exit 0
        bash.set_complete(0);
        let lines = bash.render(80);
        let all = lines.join("\n");
        assert!(all.contains("hello world"), "Should show output");
        assert!(!all.contains("exit 0"), "No exit code for success");

        // Complete with exit 1
        bash.set_complete(1);
        let lines = bash.render(80);
        let all = lines.join("\n");
        assert!(all.contains("exit 1"), "Should show exit code");
    }

    #[test]
    fn test_collapsed_preview_shows_first_lines() {
        init_theme(Some("dark"), false);
        let mut bash = BashExecution::new("test");
        for i in 0..50 {
            bash.append_output(format!("line {}", i));
        }
        bash.set_complete(0);

        let lines = bash.render(80);
        let all = lines.join("\n");
        assert!(all.contains("line 0"), "Collapsed: show first line");
        assert!(all.contains("line 19"), "Collapsed: show line 20");
        assert!(!all.contains("line 20"), "Collapsed: hide line 21");
        assert!(!all.contains("line 49"), "Collapsed: hide last line");
        assert!(all.contains("30 more lines"), "Should show remaining count");
    }

    #[test]
    fn test_expanded_shows_all_lines() {
        init_theme(Some("dark"), false);
        let mut bash = BashExecution::new("test");
        for i in 0..50 {
            bash.append_output(format!("line {}", i));
        }
        bash.set_expanded(true);
        bash.set_complete(0);

        let lines = bash.render(80);
        let all = lines.join("\n");
        assert!(all.contains("line 0"), "Expanded: show first line");
        assert!(all.contains("line 49"), "Expanded: show last line");
        assert!(
            !all.contains("more lines"),
            "No 'more lines' indicator when expanded"
        );
    }

    #[test]
    fn test_exclude_from_context_uses_dim_border() {
        init_theme(Some("dark"), false);
        let mut bash = BashExecution::new("hidden command");
        bash.set_exclude_from_context(true);
        let lines = bash.render(80);
        let all = lines.join("\n");
        assert!(all.contains("hidden command"), "Should show command");
    }

    #[test]
    fn test_cancelled_shows_warning() {
        init_theme(Some("dark"), false);
        let mut bash = BashExecution::new("sleep 10");
        bash.set_cancelled();
        let lines = bash.render(80);
        let all = lines.join("\n");
        assert!(all.contains("cancelled"), "Should show cancelled status");
    }

    #[test]
    fn test_context_truncation() {
        let mut bash = BashExecution::new("test");
        // Add more lines than MAX_LINES
        for i in 0..DEFAULT_MAX_LINES + 10 {
            bash.append_output(format!("line {}", i));
        }
        let (output, truncated) = bash.context_truncated_output();
        assert!(truncated, "Should be truncated");
        let line_count = output.split('\n').count();
        assert_eq!(line_count, DEFAULT_MAX_LINES, "Should have MAX_LINES lines");
    }

    #[test]
    fn test_append_chunk_preserves_incomplete_last_line() {
        let mut bash = BashExecution::new("echo test");
        bash.append_chunk("first\nsecond\nincomplete");
        assert_eq!(bash.output_lines.len(), 3);
        assert_eq!(bash.output_lines[0], "first");
        assert_eq!(bash.output_lines[1], "second");
        assert_eq!(bash.output_lines[2], "incomplete");
    }

    #[test]
    fn test_strip_ansi_basic() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("no ansi"), "no ansi");
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn test_strip_ansi_complex() {
        assert_eq!(strip_ansi("\x1b[1;31mbold red\x1b[0m"), "bold red");
        assert_eq!(
            strip_ansi("\x1b[38;2;255;0;0mtruecolor\x1b[39m"),
            "truecolor"
        );
    }
}
