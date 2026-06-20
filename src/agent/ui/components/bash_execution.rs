use crate::tui::Component;

/// Bash execution component - renders a bash command with borders, spinner, and output.
///
/// Matches pi's BashExecutionComponent design:
/// - Top/bottom borders in `bashMode` color
/// - Command header with `$` prefix
/// - Spinner while running
/// - Streaming output in muted color
/// - Collapse/expand support
pub struct BashExecution {
    command: String,
    output: Vec<String>,
    status: BashStatus,
    expanded: bool,
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

        // ── Output (when expanded or if there's little output) ──
        let max_preview = 20;
        let show_all = self.expanded || self.output.len() <= max_preview;
        let output_to_show: &[String] = if show_all {
            &self.output
        } else {
            &self.output[self.output.len().saturating_sub(max_preview)..]
        };

        for line in output_to_show {
            let styled = theme.fg("toolOutput", line);
            let truncated = crate::tui::util::truncate_to_width(&styled, width, "", false);
            lines.push(truncated);
        }

        // ── Status line ──
        match &self.status {
            BashStatus::Running => {
                let spinner = "⠋"; // Will be animated by caller
                let msg = format!(" {} {}", spinner, theme.fg("muted", "Running..."));
                lines.push(msg);
            }
            BashStatus::Complete { exit_code } => {
                if self.output.len() > max_preview && !self.expanded {
                    let hidden = self.output.len() - max_preview;
                    lines.push(theme.fg("muted", &format!("... {} more lines", hidden)));
                }
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
