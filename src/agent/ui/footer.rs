use crate::agent::types::Usage;
use crate::agent::ui::theme::RabTheme;
use crate::tui::Theme;
use crate::tui::util::{truncate_to_width, visible_width};

/// Pi-style footer: 2–3 lines with dim styling.
pub struct Footer {
    cwd: String,
    git_branch: Option<String>,
    session_name: Option<String>,
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
    total_cache_write: u64,
    total_cost: f64,
    context_percent: Option<f64>,
    context_window: u64,
    auto_compact: bool,
    model: String,
    thinking_level: Option<String>,
    is_streaming: bool,
    pub extension_statuses: Vec<String>,
    theme: RabTheme,
}

impl Footer {
    pub fn new(cwd: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            git_branch: None,
            session_name: None,
            total_input: 0,
            total_output: 0,
            total_cache_read: 0,
            total_cache_write: 0,
            total_cost: 0.0,
            context_percent: None,
            context_window: 0,
            auto_compact: true,
            model: String::new(),
            thinking_level: None,
            is_streaming: false,
            extension_statuses: Vec::new(),
            theme: RabTheme,
        }
    }

    pub fn set_cwd(&mut self, cwd: impl Into<String>) {
        self.cwd = cwd.into();
    }
    pub fn set_git_branch(&mut self, branch: Option<String>) {
        self.git_branch = branch;
    }
    pub fn set_session_name(&mut self, name: Option<String>) {
        self.session_name = name;
    }
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }
    pub fn set_thinking_level(&mut self, level: Option<String>) {
        self.thinking_level = level;
    }
    pub fn set_streaming(&mut self, streaming: bool) {
        self.is_streaming = streaming;
    }
    pub fn set_auto_compact(&mut self, enabled: bool) {
        self.auto_compact = enabled;
    }

    pub fn accumulate_usage(&mut self, usage: &Usage) {
        self.total_input += usage.input_tokens.unwrap_or(0) as u64;
        self.total_output += usage.output_tokens.unwrap_or(0) as u64;
        self.total_cache_read += usage.cache_tokens.unwrap_or(0) as u64;
    }

    pub fn set_context(&mut self, percent: Option<f64>, window: u64) {
        self.context_percent = percent;
        self.context_window = window;
    }
}

impl crate::tui::Component for Footer {
    fn render(&self, width: usize) -> Vec<String> {
        let w = width;
        let dim = |s: &str| self.theme.fg("dim", s);
        let error = |s: &str| self.theme.fg("error", s);
        let warning = |s: &str| self.theme.fg("warning", s);
        let accent = |s: &str| self.theme.fg("accent", s);
        let mut lines = Vec::new();

        // ── Line 1: cwd (branch) • session-name ──
        let mut pwd = self.cwd.clone();
        if let Ok(home) = std::env::var("HOME")
            && pwd.starts_with(&home)
        {
            pwd = pwd.replacen(&home, "~", 1);
        }
        if let Some(ref branch) = self.git_branch {
            pwd = format!("{} ({})", pwd, branch);
        }
        if let Some(ref name) = self.session_name {
            pwd = format!("{} • {}", pwd, name);
        }
        lines.push(dim(&pwd));

        // ── Line 2: stats left, model right ──
        let mut stats_parts: Vec<String> = Vec::new();

        if self.total_input > 0 {
            stats_parts.push(format!(
                "↑{}",
                super::messages::fmt_tokens(self.total_input as f64)
            ));
        }
        if self.total_output > 0 {
            stats_parts.push(format!(
                "↓{}",
                super::messages::fmt_tokens(self.total_output as f64)
            ));
        }
        if self.total_cache_read > 0 {
            stats_parts.push(format!(
                "R{}",
                super::messages::fmt_tokens(self.total_cache_read as f64)
            ));
        }
        if self.total_cache_write > 0 {
            stats_parts.push(format!(
                "W{}",
                super::messages::fmt_tokens(self.total_cache_write as f64)
            ));
        }

        // Context window %
        let auto_indicator = if self.auto_compact { " (auto)" } else { "" };
        let context_str = match self.context_percent {
            Some(p) => {
                let window_str = super::messages::fmt_tokens(self.context_window as f64);
                let s = format!("{:.1}%/{}{}", p, window_str, auto_indicator);
                if p > 90.0 {
                    error(&s)
                } else if p > 70.0 {
                    warning(&s)
                } else {
                    s
                }
            }
            None => {
                let window_str = super::messages::fmt_tokens(self.context_window as f64);
                format!("?/{}{}", window_str, auto_indicator)
            }
        };
        stats_parts.push(context_str);

        if self.total_cost > 0.0 {
            stats_parts.push(format!("${:.3}", self.total_cost));
        }

        let stats_left = stats_parts.join(" ");

        // Model + thinking on the right
        let model_display = self
            .model
            .strip_prefix("opencode_go::")
            .unwrap_or(&self.model);
        let right_side = match &self.thinking_level {
            Some(level) if level != "off" => format!("{} • {}", model_display, level),
            _ => model_display.to_string(),
        };

        let stats_w = visible_width(&stats_left);
        let right_w = visible_width(&right_side);
        let total_needed = stats_w + 2 + right_w;

        let line2 = if total_needed <= w {
            let padding = " ".repeat(w - stats_w - right_w);
            format!("{}{}{}", stats_left, padding, right_side)
        } else {
            let available = w.saturating_sub(stats_w + 2);
            if available > 0 {
                let truncated = truncate_to_width(&right_side, available, "", false);
                let truncated_w = visible_width(&truncated);
                let padding = " ".repeat(w.saturating_sub(stats_w + truncated_w));
                format!("{}{}{}", stats_left, padding, truncated)
            } else {
                stats_left
            }
        };

        let status_dot = if self.is_streaming {
            accent("●")
        } else {
            dim("○")
        };
        lines.push(dim(&format!("{} {}", status_dot, line2)));

        // ── Line 3: extension statuses ──
        if !self.extension_statuses.is_empty() {
            let status_line = self.extension_statuses.join(" ");
            lines.push(dim(&status_line));
        }

        lines
    }
}
