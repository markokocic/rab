use crate::tui::Component;

/// Maximum visible lines per line of output before truncation.
const MAX_LINE_LEN: usize = 200;

/// Number of preview lines to show when collapsed.
const PREVIEW_LINES: usize = 20;

/// Bash execution component - renders a bash command with borders, spinner, and output.
///
/// Matches pi's BashExecutionComponent design:
/// - Top/bottom borders in `bashMode` color
/// - Command header with `$` prefix
/// - Spinner while running
/// - Streaming output in muted color
/// - Collapse/expand support with preview truncation
pub struct BashExecution {
    command: String,
    output: Vec<String>,
    status: BashStatus,
    expanded: bool,
    /// Optional compact label shown when collapsed and no output to preview.
    compact_label: Option<String>,
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
        Self {
            command: command.into(),
            output: Vec::new(),
            status: BashStatus::Running,
            expanded: false,
            compact_label: None,
        }
    }

    pub fn append_output(&mut self, line: impl Into<String>) {
        self.output.push(line.into());
    }

    pub fn set_complete(&mut self, exit_code: i32) {
        self.status = if exit_code == 0 {
            BashStatus::Complete { exit_code: 0 }
        } else {
            BashStatus::Complete { exit_code }
        };
    }

    pub fn set_cancelled(&mut self) {
        self.status = BashStatus::Cancelled;
    }

    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.status = BashStatus::Error(msg.into());
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    pub fn set_compact_label(&mut self, label: impl Into<String>) {
        self.compact_label = Some(label.into());
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    fn border_color(&self) -> &'static str {
        match self.status {
            BashStatus::Running => "toolPendingBg",
            BashStatus::Complete { exit_code: 0 } => "bashMode",
            BashStatus::Complete { .. } => "error",
            BashStatus::Cancelled => "warning",
            BashStatus::Error(_) => "error",
        }
    }
}

impl Component for BashExecution {
    fn render(&self, width: usize) -> Vec<String> {
        use crate::agent::ui::theme::current_theme;
        let theme = current_theme();

        let mut lines: Vec<String> = Vec::new();

        // ── Top border ──
        let border_fn = |s: &str| theme.fg(self.border_color(), s);
        let border = format!("┌{}┐", "─".repeat(width.saturating_sub(2)));
        lines.push(border_fn(&border));

        // ── Command header ──
        let header = format!(
            " {} {}",
            theme.bold_fg(self.border_color(), "$"),
            theme.fg(self.border_color(), &self.command)
        );
        lines.push(border_fn(&crate::tui::util::truncate_to_width(
            &header, width, "", false,
        )));

        // ── Output ──
        if !self.expanded && self.output.len() > PREVIEW_LINES {
            // Collapsed: show first PREVIEW_LINES, then "... N more lines"
            for line in &self.output[..PREVIEW_LINES] {
                let styled = theme.fg("toolOutput", line);
                let truncated = truncate_output_line(&styled, width);
                lines.push(truncated);
            }
            let hidden = self.output.len() - PREVIEW_LINES;
            lines.push(theme.fg("muted", &format!("... {} more lines", hidden)));
        } else if self.expanded || !self.output.is_empty() {
            // Expanded or small output: show all with visual truncation
            for line in &self.output {
                let styled = theme.fg("toolOutput", line);
                let truncated = truncate_output_line(&styled, width);
                lines.push(truncated);
            }
        } else if let Some(ref label) = self.compact_label {
            // No output but compact label: show just the label
            lines.push(theme.fg("toolTitle", label));
        }

        // ── Status line ──
        match &self.status {
            BashStatus::Running => {
                let spinner = "⠋";
                let msg = format!(" {} {}", spinner, theme.fg("muted", "Running..."));
                lines.push(msg);
            }
            BashStatus::Complete { exit_code } => {
                if *exit_code != 0 {
                    lines.push(theme.fg("error", &format!("(exit {})", exit_code)));
                }
            }
            BashStatus::Cancelled => {
                lines.push(theme.fg("warning", "(cancelled)"));
            }
            BashStatus::Error(msg) => {
                lines.push(theme.fg("error", &format!("Error: {}", msg)));
            }
        }

        // ── Bottom border ──
        let border = format!("└{}┘", "─".repeat(width.saturating_sub(2)));
        lines.push(border_fn(&border));

        lines
    }

    fn invalidate(&mut self) {}
}

/// Truncate an output line to avoid excessively long lines in the terminal.
fn truncate_output_line(text: &str, _width: usize) -> String {
    if text.len() > MAX_LINE_LEN {
        let truncated: String = text.chars().take(MAX_LINE_LEN).collect();
        format!("{}…", truncated)
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bash_execution_new() {
        let bash = BashExecution::new("echo hello");
        assert_eq!(bash.command, "echo hello");
        assert!(bash.output.is_empty());
        assert_eq!(bash.status, BashStatus::Running);
        assert!(!bash.expanded);
    }

    #[test]
    fn test_bash_execution_append_output() {
        let mut bash = BashExecution::new("echo hello");
        bash.append_output("hello");
        bash.append_output("world");
        assert_eq!(bash.output.len(), 2);
        assert_eq!(bash.output[0], "hello");
        assert_eq!(bash.output[1], "world");
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
    fn test_bash_execution_compact_label() {
        let mut bash = BashExecution::new("echo hello");
        bash.set_compact_label("✓ Done");
        assert_eq!(bash.compact_label, Some("✓ Done".into()));
    }

    #[test]
    fn test_bash_execution_render_has_borders() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let bash = BashExecution::new("echo hello");
        let lines = bash.render(80);
        let all = lines.join("\n");
        assert!(all.contains('┌'), "Should have top border");
        assert!(all.contains('└'), "Should have bottom border");
        assert!(all.contains("echo hello"), "Should show command");
    }

    #[test]
    fn test_bash_execution_render_status() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let mut bash = BashExecution::new("echo hello");
        bash.append_output("hello world");

        // Complete with exit 0
        bash.set_complete(0);
        let lines = bash.render(80);
        let all = lines.join("\n");
        assert!(all.contains("hello world"), "Should show output");
        assert!(!all.contains("exit"), "No exit code for success");

        // Complete with exit 1
        bash.set_complete(1);
        let lines = bash.render(80);
        let all = lines.join("\n");
        assert!(all.contains("exit 1"), "Should show exit code");
    }

    #[test]
    fn test_truncate_long_output_line() {
        let long = "a".repeat(300);
        let truncated = truncate_output_line(&long, 80);
        // MAX_LINE_LEN=200 chars + 1 ellipsis char (3 bytes in UTF-8)
        let chars: Vec<char> = truncated.chars().collect();
        assert!(
            chars.len() <= 201,
            "Should truncate to MAX_LINE_LEN+1 chars"
        );
        assert!(truncated.ends_with('…'), "Should end with ellipsis");
    }

    #[test]
    fn test_collapsed_preview_shows_first_lines() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let mut bash = BashExecution::new("test");
        for i in 0..50 {
            bash.append_output(format!("line {}", i));
        }
        bash.set_complete(0);

        let lines = bash.render(80);
        let all = lines.join("\n");
        assert!(all.contains("line 0"), "Collapsed: show first line");
        assert!(all.contains("line 19"), "Collapsed: show 20th line");
        assert!(!all.contains("line 20"), "Collapsed: hide 21st line");
        assert!(all.contains("30 more lines"), "Should show remaining count");
    }

    #[test]
    fn test_expanded_shows_all_lines() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
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
}
