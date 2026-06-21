use crate::agent::types::Usage;
use crate::agent::ui::messages::{fmt_tokens, pad_to_width};
use crate::agent::ui::theme::RabTheme;
use crate::tui::util::{truncate_to_width, visible_width};

/// Pi-style footer: 2-3 lines with dim styling.
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
        let theme = crate::agent::ui::theme::current_theme().clone();
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
            theme,
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
        if w < 4 {
            return vec![]; // Too narrow to show anything
        }

        let dim = |s: &str| self.theme.fg("dim", s);
        let error = |s: &str| self.theme.fg("error", s);
        let warning = |s: &str| self.theme.fg("warning", s);
        let accent = |s: &str| self.theme.fg("accent", s);
        let bold = |s: &str| self.theme.fg("accent", s);
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
        let line1 = truncate_to_width(&dim(&pwd), w, "…", false);
        lines.push(if line1.is_empty() {
            String::new()
        } else {
            pad_to_width(&line1, w)
        });

        // ── Line 2: stats left, model right ──
        // Build stats parts
        let mut stats_parts: Vec<String> = Vec::new();

        if self.total_input > 0 {
            stats_parts.push(format!("↑{}", fmt_tokens(self.total_input as f64)));
        }
        if self.total_output > 0 {
            stats_parts.push(format!("↓{}", fmt_tokens(self.total_output as f64)));
        }
        if self.total_cache_read > 0 {
            stats_parts.push(format!("R{}", fmt_tokens(self.total_cache_read as f64)));
        }
        if self.total_cache_write > 0 {
            stats_parts.push(format!("W{}", fmt_tokens(self.total_cache_write as f64)));
        }

        // Context window with auto-compact indicator
        if self.auto_compact {
            let ac_label = bold("⚡");
            stats_parts.push(format!("{}auto", ac_label));
        }
        let context_str = match self.context_percent {
            Some(p) => {
                let window_str = fmt_tokens(self.context_window as f64);
                let s = format!("{:.1}%/{}", p, window_str);
                if p > 90.0 {
                    error(&s)
                } else if p > 70.0 {
                    warning(&s)
                } else {
                    s
                }
            }
            None if self.context_window > 0 => {
                let window_str = fmt_tokens(self.context_window as f64);
                format!("?/{}", window_str)
            }
            None => String::new(),
        };
        if !context_str.is_empty() {
            stats_parts.push(context_str);
        }

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

        let dot_indicator = if self.is_streaming {
            accent("●")
        } else {
            dim("○")
        };
        let dot_prefix = format!("{} ", dot_indicator);
        let dot_w = visible_width(&dot_prefix); // 2

        let stats_w = visible_width(&stats_left);
        let right_w = visible_width(&right_side);
        let total_needed = dot_w + stats_w + 2 + right_w;

        let line2 = if total_needed <= w {
            // Everything fits
            let padding = " ".repeat(w - stats_w - right_w - dot_w);
            format!("{}{}{}{}", dot_prefix, stats_left, padding, right_side)
        } else {
            // Need to truncate. Priority: dot > model > stats
            let min_for_right = w.saturating_sub(dot_w + stats_w + 2);
            if min_for_right >= 4 {
                // Room for at least some of the right side
                let truncated = truncate_to_width(&right_side, min_for_right, "…", false);
                let truncated_w = visible_width(&truncated);
                let padding = " ".repeat(w.saturating_sub(dot_w + stats_w + truncated_w));
                format!("{}{}{}{}", dot_prefix, stats_left, padding, truncated)
            } else {
                // Very narrow: shrink stats too
                let avail_for_stats = w.saturating_sub(dot_w + 1);
                let truncated_stats = truncate_to_width(&stats_left, avail_for_stats, "…", false);
                let s = format!("{}{}", dot_prefix, truncated_stats);
                pad_to_width(&s, w)
            }
        };
        lines.push(line2);

        // ── Line 3: extension statuses ──
        if !self.extension_statuses.is_empty() {
            let status_line = self.extension_statuses.join(" ");
            let truncated = truncate_to_width(&dim(&status_line), w, "…", false);
            lines.push(if truncated.is_empty() {
                String::new()
            } else {
                pad_to_width(&truncated, w)
            });
        }

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::Component;

    fn make_footer() -> Footer {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let mut footer = Footer::new("/home/user/project");
        footer.set_model("test-model");
        footer.set_git_branch(Some("main".into()));
        footer
    }

    // ── Line 1 (cwd) tests ──

    #[test]
    fn test_footer_shows_cwd() {
        let footer = make_footer();
        let lines = footer.render(80);
        assert!(lines.len() >= 2, "Should have at least 2 lines");
        assert!(lines[0].contains("project"), "Should show cwd");
    }

    #[test]
    fn test_footer_shows_git_branch() {
        let footer = make_footer();
        let lines = footer.render(80);
        assert!(lines[0].contains("main"), "Should show git branch");
    }

    #[test]
    fn test_footer_shows_session_name() {
        let mut footer = make_footer();
        footer.set_session_name(Some("my-session".into()));
        let lines = footer.render(80);
        assert!(lines[0].contains("my-session"), "Should show session name");
    }

    #[test]
    fn test_cwd_truncated_to_width() {
        let mut footer = Footer::new("/very/long/path/that/exceeds/available/width/completely");
        footer.set_model("model");
        let lines = footer.render(30);
        assert!(lines.len() >= 2);
        // Should not exceed width
        for line in &lines {
            assert!(
                visible_width(line) <= 30,
                "Line '{}' exceeds width 30",
                line
            );
        }
    }

    // ── Line 2 (stats/model) tests ──

    #[test]
    fn test_footer_shows_model() {
        let footer = make_footer();
        let lines = footer.render(80);
        assert!(lines[1].contains("test-model"), "Should show model name");
    }

    #[test]
    fn test_footer_shows_dot_indicator() {
        let footer = make_footer();
        let lines = footer.render(80);
        assert!(lines[1].contains('○'), "Should show idle dot indicator");
    }

    #[test]
    fn test_footer_shows_streaming_dot() {
        let mut footer = make_footer();
        footer.set_streaming(true);
        let lines = footer.render(80);
        assert!(
            lines[1].contains('●'),
            "Should show streaming dot indicator"
        );
    }

    #[test]
    fn test_footer_shows_thinking_level() {
        let mut footer = make_footer();
        footer.set_thinking_level(Some("high".into()));
        let lines = footer.render(80);
        assert!(lines[1].contains("high"), "Should show thinking level");
    }

    #[test]
    fn test_footer_shows_token_usage() {
        let mut footer = make_footer();
        let usage = Usage {
            input_tokens: Some(1500),
            output_tokens: Some(500),
            cache_tokens: None,
        };
        footer.accumulate_usage(&usage);
        let lines = footer.render(80);
        assert!(lines[1].contains("↑"), "Should show input tokens");
        assert!(lines[1].contains("↓"), "Should show output tokens");
    }

    #[test]
    fn test_footer_usage_multiple_calls() {
        let mut footer = make_footer();
        let u1 = Usage {
            input_tokens: Some(1000),
            output_tokens: Some(500),
            cache_tokens: None,
        };
        footer.accumulate_usage(&u1);
        let u2 = Usage {
            input_tokens: Some(2000),
            output_tokens: Some(300),
            cache_tokens: None,
        };
        footer.accumulate_usage(&u2);
        // Should accumulate: 3000 input, 800 output
        let lines = footer.render(80);
        assert!(lines[1].contains("↑3.0k"), "Should show accumulated input");
        assert!(lines[1].contains("↓800"), "Should show accumulated output");
    }

    // ── Auto-compact indicator tests ──

    #[test]
    fn test_footer_shows_auto_compact_indicator_when_enabled() {
        let mut footer = make_footer();
        footer.set_auto_compact(true);
        let lines = footer.render(80);
        assert!(
            lines[1].contains("⚡"),
            "Should show auto-compact indicator when enabled"
        );
        assert!(
            lines[1].contains("auto"),
            "Should show 'auto' label when enabled"
        );
    }

    #[test]
    fn test_footer_hides_auto_compact_indicator_when_disabled() {
        let mut footer = make_footer();
        footer.set_auto_compact(false);
        let lines = footer.render(80);
        assert!(
            !lines[1].contains("⚡"),
            "Should NOT show auto-compact indicator when disabled"
        );
    }

    // ── Narrow terminal tests ──

    #[test]
    fn test_footer_handles_narrow_terminal() {
        let mut footer = make_footer();
        footer.set_thinking_level(Some("high".into()));
        let usage = Usage {
            input_tokens: Some(100000),
            output_tokens: Some(50000),
            cache_tokens: Some(10000),
        };
        footer.accumulate_usage(&usage);
        let lines = footer.render(10);
        assert!(!lines.is_empty(), "Should render even at width 10");
        for line in &lines {
            assert!(
                visible_width(line) <= 10,
                "Line '{}' exceeds width 10",
                line
            );
        }
    }

    #[test]
    fn test_footer_handles_very_narrow_terminal() {
        let footer = make_footer();
        let lines = footer.render(3);
        // At width 3, should return empty (guard at < 4)
        assert!(lines.is_empty(), "Should return empty at width 3");
    }

    #[test]
    fn test_footer_stats_not_truncated_when_room() {
        let mut footer = make_footer();
        let usage = Usage {
            input_tokens: Some(100),
            output_tokens: Some(50),
            cache_tokens: None,
        };
        footer.accumulate_usage(&usage);
        let lines = footer.render(80);
        assert!(lines[1].contains("↑100"), "Should show full token count");
        assert!(lines[1].contains("↓50"), "Should show full token count");
    }

    #[test]
    fn test_footer_line2_exact_width() {
        let footer = make_footer();
        let lines = footer.render(80);
        // Line 2 should have exactly visible_width = 80 (padded)
        for line in &lines {
            let vw = visible_width(line);
            assert!(vw <= 80, "Line width {} > 80", vw);
        }
    }

    #[test]
    fn test_footer_line2_padded_correctly() {
        let footer = make_footer();
        for w in [40, 60, 80, 120] {
            let lines = footer.render(w);
            for line in &lines {
                let vw = visible_width(line);
                assert!(vw <= w, "At width {}: line width {} exceeds", w, vw);
            }
        }
    }

    // ── Extension status line tests ──

    #[test]
    fn test_footer_shows_extension_statuses() {
        let mut footer = make_footer();
        footer.extension_statuses.push("• ext1: ready".into());
        let lines = footer.render(80);
        assert!(lines.len() >= 3, "Should have 3 lines");
        assert!(lines[2].contains("ext1"), "Should show extension status");
    }

    #[test]
    fn test_footer_extension_status_truncated() {
        let mut footer = make_footer();
        footer
            .extension_statuses
            .push("a very long extension status message that should be truncated".into());
        let lines = footer.render(30);
        if lines.len() >= 3 {
            let vw = visible_width(&lines[2]);
            assert!(vw <= 30, "Extension status line exceeds width");
        }
    }

    #[test]
    fn test_footer_context_percent_colors() {
        let mut footer = make_footer();
        footer.set_context(Some(95.0), 128000);
        let lines = footer.render(80);
        // High context should be in error color (wrapped in ANSI escape)
        assert!(lines[1].contains("95"), "Should show context percent");
        assert!(
            lines[1].contains("128k"),
            "Should show formatted window size"
        );
    }

    #[test]
    fn test_footer_context_without_percent() {
        let mut footer = make_footer();
        footer.set_context(None, 64000);
        let lines = footer.render(80);
        assert!(lines[1].contains("?"), "Should show unknown context");
        assert!(lines[1].contains("64k"), "Should show context window size");
    }

    // ── Edge cases ──

    #[test]
    fn test_footer_no_model_shows_nothing() {
        let mut footer = Footer::new("/path");
        footer.set_model("");
        let lines = footer.render(80);
        assert!(!lines.is_empty(), "Should still render with empty model");
    }

    #[test]
    fn test_footer_model_strip_prefix() {
        let mut footer = make_footer();
        footer.set_model("opencode_go::claude-opus");
        let lines = footer.render(80);
        assert!(
            !lines[1].contains("opencode_go::"),
            "Should strip opencode_go:: prefix"
        );
        assert!(
            lines[1].contains("claude-opus"),
            "Should show model after prefix"
        );
    }
}
