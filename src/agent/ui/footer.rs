use crate::agent::types::Usage;
use crate::agent::ui::theme::RabTheme;
use crate::agent::ui::theme::ThemeKey;
use crate::tui::util::{truncate_to_width, visible_width};

// ── Helpers matching pi's footer.ts ──────────────────────────────

/// Sanitize text for display in a single-line status.
/// Removes newlines, tabs, carriage returns, and other control characters.
fn sanitize_status_text(text: &str) -> String {
    text.replace(['\r', '\n', '\t'], " ")
        .split(' ')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Format token count for compact footer display (pi-style).
pub fn format_tokens(count: u64) -> String {
    if count < 1000 {
        return count.to_string();
    }
    if count < 10000 {
        return format!("{:.1}k", count as f64 / 1000.0);
    }
    if count < 1_000_000 {
        return format!("{}k", (count as f64 / 1000.0).round() as u64);
    }
    if count < 10_000_000 {
        return format!("{:.1}M", count as f64 / 1_000_000.0);
    }
    format!("{}M", (count as f64 / 1_000_000.0).round() as u64)
}

/// Format cwd for footer display (pi-style `formatCwdForFooter`).
/// Resolves cwd relative to home directory, using `~` prefix.
///
/// Matches pi which uses `path.resolve()` + `path.relative()` to handle
/// symlinks, `..`, and edge cases correctly.
pub fn format_cwd_for_footer(cwd: &str, home: Option<&str>) -> String {
    let home = match home {
        Some(h) => h,
        None => return cwd.to_string(),
    };

    // Canonicalize both paths to resolve symlinks and `..` (pi uses `resolve`).
    // Fall back to raw paths if canonicalize fails (e.g. non-existent cwd).
    let resolved_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| std::path::PathBuf::from(cwd));
    let resolved_home =
        std::fs::canonicalize(home).unwrap_or_else(|_| std::path::PathBuf::from(home));

    match resolved_cwd.strip_prefix(&resolved_home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => cwd.to_string(),
    }
}

// ── Footer Component ─────────────────────────────────────────────

/// Pi-style footer: 2-3 lines with dim styling.
/// Matches pi's `FooterComponent` in `footer.ts` exactly.
pub struct Footer {
    cwd: String,
    git_branch: Option<String>,
    session_name: Option<String>,

    /// Pre-computed stats (pi computes fresh from session entries each render).
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
    total_cache_write: u64,
    total_cost: f64,
    latest_cache_hit_rate: Option<f64>,

    context_percent: Option<f64>,
    context_window: u64,

    auto_compact: bool,

    model: String,
    /// Whether model supports reasoning (for showing thinking level).
    model_supports_reasoning: bool,
    thinking_level: Option<String>,

    /// Number of unique providers with available models (for footer display).
    available_provider_count: usize,

    /// Whether using OAuth subscription (shows "(sub)" after cost).
    using_subscription: bool,

    /// Experimental features enabled.
    experimental_enabled: bool,

    pub extension_statuses: Vec<(String, String)>, // (key, text) sorted by key

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
            latest_cache_hit_rate: None,
            context_percent: None,
            context_window: 0,
            auto_compact: true,
            model: String::new(),
            model_supports_reasoning: false,
            thinking_level: None,
            available_provider_count: 1,
            using_subscription: false,
            experimental_enabled: false,
            extension_statuses: Vec::new(),
            theme,
        }
    }

    // ── Setters (called from App) ──

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

    /// Set whether the model supports reasoning (for showing thinking level in footer).
    pub fn set_model_supports_reasoning(&mut self, supports: bool) {
        self.model_supports_reasoning = supports;
    }

    pub fn set_thinking_level(&mut self, level: Option<String>) {
        self.thinking_level = level;
    }

    pub fn set_auto_compact(&mut self, enabled: bool) {
        self.auto_compact = enabled;
    }

    pub fn set_available_provider_count(&mut self, count: usize) {
        self.available_provider_count = count;
    }

    pub fn set_using_subscription(&mut self, using: bool) {
        self.using_subscription = using;
    }

    pub fn set_experimental_enabled(&mut self, enabled: bool) {
        self.experimental_enabled = enabled;
    }

    /// Pi-style: accumulate usage from a single response's usage data.
    /// Accumulates input, output, cache_read, cache_write, and cost.
    /// Updates latest_cache_hit_rate from this call's cache ratio.
    pub fn accumulate_usage(&mut self, usage: &Usage) {
        let input = usage.input_tokens.unwrap_or(0) as u64;
        let output = usage.output_tokens.unwrap_or(0) as u64;
        let cache_read = usage.cache_tokens.unwrap_or(0) as u64;
        let cache_write = usage.cache_write_tokens.unwrap_or(0) as u64;

        self.total_input += input;
        self.total_output += output;
        self.total_cache_read += cache_read;
        self.total_cache_write += cache_write;

        if let Some(cost) = usage.cost_total {
            self.total_cost += cost;
        }

        // Compute cache hit rate from latest call
        let total_prompt = input + cache_read;
        if total_prompt > 0 {
            self.latest_cache_hit_rate = Some((cache_read as f64 / total_prompt as f64) * 100.0);
        }
    }

    /// Pi-style: set cumulative usage directly (replaces accumulated values).
    pub fn set_usage(
        &mut self,
        total_input: u64,
        total_output: u64,
        total_cache_read: u64,
        total_cache_write: u64,
        total_cost: f64,
        latest_cache_hit_rate: Option<f64>,
    ) {
        self.total_input = total_input;
        self.total_output = total_output;
        self.total_cache_read = total_cache_read;
        self.total_cache_write = total_cache_write;
        self.total_cost = total_cost;
        self.latest_cache_hit_rate = latest_cache_hit_rate;
    }

    /// Pi-style: cost is set separately (from usage.cost.total).
    pub fn set_cost(&mut self, cost: f64) {
        self.total_cost = cost;
    }

    /// Set cache write tokens separately.
    pub fn set_cache_write(&mut self, cache_write: u64) {
        self.total_cache_write = cache_write;
    }

    /// Pi-style: no streaming dot indicator in footer (handled by working indicator).
    /// Kept for compatibility with existing call sites.
    pub fn set_streaming(&mut self, _streaming: bool) {
        // No-op: pi footer doesn't show streaming dot
    }

    /// Pi-style set context / context window.
    pub fn set_context(&mut self, percent: Option<f64>, window: u64) {
        self.context_percent = percent;
        self.context_window = window;
    }

    /// Set an extension status (pi-style, key-value pair).
    pub fn set_extension_status(&mut self, key: String, text: Option<String>) {
        if let Some(text) = text {
            // Update existing or insert
            if let Some(pos) = self.extension_statuses.iter().position(|(k, _)| k == &key) {
                self.extension_statuses[pos].1 = text;
            } else {
                self.extension_statuses.push((key, text));
            }
        } else {
            // Remove
            self.extension_statuses.retain(|(k, _)| k != &key);
        }
        // Keep sorted by key (pi-style)
        self.extension_statuses.sort_by(|(a, _), (b, _)| a.cmp(b));
    }

    /// Clear all extension statuses (pi-style).
    pub fn clear_extension_statuses(&mut self) {
        self.extension_statuses.clear();
    }
}

impl crate::tui::Component for Footer {
    fn render(&mut self, width: usize) -> Vec<String> {
        let w = width;
        if w < 4 {
            return vec![]; // Too narrow to show anything
        }

        let theme = &self.theme;

        // ── Line 1: pwd (git branch) • session-name ──
        // pi: truncateToWidth(theme.fg("dim", pwd), width, theme.fg("dim", "..."));
        // No padding — the TUI's differential render handles trailing space.
        let home = std::env::var("HOME").ok();
        let mut pwd = format_cwd_for_footer(&self.cwd, home.as_deref());

        if let Some(ref branch) = self.git_branch {
            pwd = format!("{} ({})", pwd, branch);
        }
        if let Some(ref name) = self.session_name {
            pwd = format!("{} • {}", pwd, name);
        }
        let pwd_line = truncate_to_width(
            &theme.fg_key(ThemeKey::Dim, &pwd),
            w,
            &theme.fg_key(ThemeKey::Dim, "..."),
            false, // pi: no padding
        );

        // ── Line 2: stats left, model right (both dimmed separately) ──
        // Build stats parts (pi-style order and content)
        let mut stats_parts: Vec<String> = Vec::new();

        if self.total_input > 0 {
            stats_parts.push(format!("↑{}", format_tokens(self.total_input)));
        }
        if self.total_output > 0 {
            stats_parts.push(format!("↓{}", format_tokens(self.total_output)));
        }
        if self.total_cache_read > 0 {
            stats_parts.push(format!("R{}", format_tokens(self.total_cache_read)));
        }
        if self.total_cache_write > 0 {
            stats_parts.push(format!("W{}", format_tokens(self.total_cache_write)));
        }
        if (self.total_cache_read > 0 || self.total_cache_write > 0)
            && let Some(hit_rate) = self.latest_cache_hit_rate
        {
            stats_parts.push(format!("CH{:.1}%", hit_rate));
        }

        // Cost with optional "(sub)" indicator (pi-style)
        if self.total_cost > 0.0 || self.using_subscription {
            let cost_str = if self.using_subscription {
                format!("${:.3} (sub)", self.total_cost)
            } else {
                format!("${:.3}", self.total_cost)
            };
            stats_parts.push(cost_str);
        }

        // Context percentage with color (pi-style: red > 90, yellow > 70)
        let context_percent_str = match self.context_percent {
            Some(p) => {
                let window_str = format_tokens(self.context_window);
                let display = if self.auto_compact {
                    format!("{:.1}%/{} (auto)", p, window_str)
                } else {
                    format!("{:.1}%/{}", p, window_str)
                };
                if p > 90.0 {
                    theme.fg_key(ThemeKey::Error, &display)
                } else if p > 70.0 {
                    theme.fg_key(ThemeKey::Warning, &display)
                } else {
                    display
                }
            }
            None => {
                let window_str = format_tokens(self.context_window);
                if self.auto_compact {
                    format!("?/{} (auto)", window_str)
                } else {
                    format!("?/{}", window_str)
                }
            }
        };
        if !context_percent_str.is_empty() {
            stats_parts.push(context_percent_str);
        }

        // Experimental features indicator (pi-style)
        if self.experimental_enabled {
            stats_parts.push(format!(
                "{} {}",
                theme.fg_key(ThemeKey::Dim, "•"),
                theme.bold(&theme.fg_key(ThemeKey::Warning, "xp"))
            ));
        }

        let mut stats_left = stats_parts.join(" ");

        // Build right side: model name + thinking level (pi-style)
        let model_name = if self.model.is_empty() {
            "no-model".to_string()
        } else {
            self.model
                .strip_prefix("opencode_go::")
                .unwrap_or(&self.model)
                .to_string()
        };

        // Pi-style right side with thinking level indicator
        let right_side_without_provider = if self.model_supports_reasoning {
            match &self.thinking_level {
                Some(level) if level != "off" => format!("{} • {}", model_name, level),
                _ => format!("{} • thinking off", model_name),
            }
        } else {
            model_name.clone()
        };

        // Prepend provider in parentheses if multiple providers (pi-style)
        let right_side = if self.available_provider_count > 1 && !self.model.is_empty() {
            let model_with_provider = format!("(?) {}", right_side_without_provider);
            // Only use provider prefix if it fits (checked below)
            model_with_provider
        } else {
            right_side_without_provider.clone()
        };

        // Compute widths and layout (pi-style)
        let mut stats_left_width = visible_width(&stats_left);

        // Pi-style: if statsLeft is too wide, truncate it (no padding).
        if stats_left_width > w {
            stats_left = truncate_to_width(&stats_left, w, "...", false);
            stats_left_width = visible_width(&stats_left);
        }

        let right_side_width = visible_width(&right_side);
        let min_padding: usize = 2;

        let stats_line = if stats_left_width + min_padding + right_side_width <= w {
            // Both fit
            let padding = " ".repeat(w - stats_left_width - right_side_width);
            format!("{}{}{}", stats_left, padding, right_side)
        } else if !self.model.is_empty()
            && self.available_provider_count > 1
            && stats_left_width + min_padding + visible_width(&right_side_without_provider) <= w
        {
            // Try without provider prefix
            let padding =
                " ".repeat(w - stats_left_width - visible_width(&right_side_without_provider));
            format!("{}{}{}", stats_left, padding, right_side_without_provider)
        } else {
            // Need to truncate right side
            let available_for_right = w.saturating_sub(stats_left_width + min_padding);
            if available_for_right > 0 {
                let truncated_right =
                    truncate_to_width(&right_side, available_for_right, "", false);
                let truncated_right_width = visible_width(&truncated_right);
                let padding = " ".repeat(w - stats_left_width - truncated_right_width);
                format!("{}{}{}", stats_left, padding, truncated_right)
            } else {
                // Not enough space for right side at all
                stats_left.clone()
            }
        };

        // Pi-style: dim statsLeft and remainder separately (statsLeft may contain colored context %)
        let dim_stats_left = theme.fg_key(ThemeKey::Dim, &stats_left);
        let remainder = &stats_line[stats_left.len()..]; // padding + rightSide
        let dim_remainder = theme.fg_key(ThemeKey::Dim, remainder);

        let stats_line_formatted = format!("{}{}", dim_stats_left, dim_remainder);

        let mut lines = vec![pwd_line, stats_line_formatted];

        // ── Line 3: extension statuses (sorted by key, sanitized) ──
        // pi: truncateToWidth(statusLine, width, theme.fg("dim", "...")) — no padding.
        if !self.extension_statuses.is_empty() {
            let status_text: Vec<String> = self
                .extension_statuses
                .iter()
                .map(|(_, text)| sanitize_status_text(text))
                .collect();
            let status_line = status_text.join(" ");
            let truncated = truncate_to_width(
                &status_line,
                w,
                &theme.fg_key(ThemeKey::Dim, "..."),
                false, // pi: no padding
            );
            if !truncated.trim().is_empty() {
                lines.push(truncated);
            }
        }

        lines
    }

    fn invalidate(&mut self) {
        // No cached state to invalidate
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

    // ── format_cwd_for_footer tests ──

    #[test]
    fn test_format_cwd_home() {
        let result = format_cwd_for_footer("/home/user/project", Some("/home/user"));
        assert_eq!(result, "~/project");
    }

    #[test]
    fn test_format_cwd_home_exact() {
        let result = format_cwd_for_footer("/home/user", Some("/home/user"));
        assert_eq!(result, "~");
    }

    #[test]
    fn test_format_cwd_outside_home() {
        let result = format_cwd_for_footer("/opt/app", Some("/home/user"));
        assert_eq!(result, "/opt/app");
    }

    #[test]
    fn test_format_cwd_no_home() {
        let result = format_cwd_for_footer("/some/path", None::<&str>);
        assert_eq!(result, "/some/path");
    }

    // ── format_tokens tests ──

    #[test]
    fn test_format_tokens_under_1k() {
        assert_eq!(format_tokens(500), "500");
    }

    #[test]
    fn test_format_tokens_1k_to_10k() {
        assert_eq!(format_tokens(5500), "5.5k");
    }

    #[test]
    fn test_format_tokens_10k_to_1m() {
        assert_eq!(format_tokens(55500), "56k");
    }

    #[test]
    fn test_format_tokens_1m_to_10m() {
        assert_eq!(format_tokens(5_500_000), "5.5M");
    }

    #[test]
    fn test_format_tokens_over_10m() {
        assert_eq!(format_tokens(55_000_000), "55M");
    }

    // ── sanitize_status_text tests ──

    #[test]
    fn test_sanitize_status() {
        assert_eq!(sanitize_status_text("hello\nworld"), "hello world");
        assert_eq!(sanitize_status_text("hello\tworld"), "hello world");
        assert_eq!(sanitize_status_text("hello\r\nworld"), "hello world");
        assert_eq!(sanitize_status_text("  spaced  "), "spaced");
    }

    // ── Line 1 (cwd) tests ──

    #[test]
    fn test_footer_shows_cwd() {
        let mut footer = make_footer();
        let lines = footer.render(80);
        assert!(lines.len() >= 2, "Should have at least 2 lines");
        assert!(lines[0].contains("project"), "Should show cwd");
    }

    #[test]
    fn test_footer_shows_git_branch() {
        let mut footer = make_footer();
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
        let mut footer = make_footer();
        let lines = footer.render(80);
        assert!(lines[1].contains("test-model"), "Should show model name");
    }

    #[test]
    fn test_footer_shows_no_model() {
        let mut footer = Footer::new("/path");
        footer.set_model("");
        let lines = footer.render(80);
        assert!(
            lines[1].contains("no-model"),
            "Should show 'no-model' when model not set"
        );
    }

    #[test]
    fn test_footer_shows_thinking_level() {
        let mut footer = make_footer();
        footer.set_model_supports_reasoning(true);
        footer.set_thinking_level(Some("high".into()));
        let lines = footer.render(80);
        assert!(lines[1].contains("high"), "Should show thinking level");
    }

    #[test]
    fn test_footer_thinking_off_with_reasoning() {
        let mut footer = make_footer();
        footer.set_model_supports_reasoning(true);
        footer.set_thinking_level(Some("off".into()));
        let lines = footer.render(80);
        assert!(
            lines[1].contains("thinking off"),
            "Should show 'thinking off' when reasoning model has level off"
        );
    }

    #[test]
    fn test_footer_shows_token_usage() {
        let mut footer = make_footer();
        let usage = Usage {
            input_tokens: Some(1500),
            output_tokens: Some(500),
            cache_tokens: None,
            cache_write_tokens: None,
            cost_total: None,
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
            cache_write_tokens: None,
            cost_total: None,
        };
        footer.accumulate_usage(&u1);
        let u2 = Usage {
            input_tokens: Some(2000),
            output_tokens: Some(300),
            cache_tokens: None,
            cache_write_tokens: None,
            cost_total: None,
        };
        footer.accumulate_usage(&u2);
        let lines = footer.render(80);
        assert!(lines[1].contains("↑3.0k"), "Should show accumulated input");
        assert!(lines[1].contains("↓800"), "Should show accumulated output");
    }

    #[test]
    fn test_footer_shows_cache_hit_rate() {
        let mut footer = make_footer();
        let usage = Usage {
            input_tokens: Some(1000),
            output_tokens: Some(500),
            cache_tokens: Some(200),
            cache_write_tokens: None,
            cost_total: None,
        };
        footer.accumulate_usage(&usage);
        let lines = footer.render(80);
        assert!(
            lines[1].contains("CH"),
            "Should show cache hit rate when cache tokens present"
        );
        // 200 / (1000 + 200) = 16.7%
        assert!(
            lines[1].contains("CH16.7%"),
            "Should show correct cache hit rate"
        );
    }

    #[test]
    fn test_footer_shows_cost() {
        let mut footer = make_footer();
        footer.set_cost(0.0123);
        let lines = footer.render(80);
        assert!(lines[1].contains("$0.012"), "Should show cost");
    }

    #[test]
    fn test_footer_shows_subscription_indicator() {
        let mut footer = make_footer();
        footer.set_cost(0.0);
        footer.set_using_subscription(true);
        let lines = footer.render(80);
        assert!(
            lines[1].contains("(sub)"),
            "Should show subscription indicator"
        );
    }

    // ── Auto-compact indicator tests ──

    #[test]
    fn test_footer_shows_auto_compact_next_to_context() {
        let mut footer = make_footer();
        footer.set_auto_compact(true);
        footer.set_context(Some(50.0), 128000);
        let lines = footer.render(80);
        assert!(
            lines[1].contains("(auto)"),
            "Should show (auto) next to context percentage"
        );
        assert!(
            lines[1].contains("50.0%/128k (auto)"),
            "Should show context percent with auto compact"
        );
    }

    #[test]
    fn test_footer_hides_auto_compact_when_disabled() {
        let mut footer = make_footer();
        footer.set_auto_compact(false);
        footer.set_context(Some(50.0), 128000);
        let lines = footer.render(80);
        assert!(
            !lines[1].contains("(auto)"),
            "Should NOT show (auto) when disabled"
        );
    }

    // ── Context percent colors ──

    #[test]
    fn test_footer_context_percent_high() {
        let mut footer = make_footer();
        footer.set_context(Some(95.0), 128000);
        let lines = footer.render(80);
        assert!(lines[1].contains("95"), "Should show context percent");
        assert!(
            lines[1].contains("128k"),
            "Should show formatted window size"
        );
        // High context should be in error color (wrapped in ANSI escape)
        assert!(
            lines[1].contains("\x1b[38;2;"),
            "Should have ANSI color for high context"
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

    // ── Extension status line tests ──

    #[test]
    fn test_footer_shows_extension_statuses() {
        let mut footer = make_footer();
        footer.set_extension_status("ext1".into(), Some("ready".into()));
        let lines = footer.render(80);
        assert!(lines.len() >= 3, "Should have 3 lines");
        assert!(lines[2].contains("ready"), "Should show extension status");
    }

    #[test]
    fn test_footer_extension_status_sorted() {
        let mut footer = make_footer();
        footer.set_extension_status("z_last".into(), Some("last".into()));
        footer.set_extension_status("a_first".into(), Some("first".into()));
        let lines = footer.render(80);
        if lines.len() >= 3 {
            let first_idx = lines[2].find("first");
            let last_idx = lines[2].find("last");
            assert!(
                first_idx < last_idx,
                "Extension statuses should be sorted by key"
            );
        }
    }

    #[test]
    fn test_footer_extension_status_sanitized() {
        let mut footer = make_footer();
        footer.set_extension_status("ext1".into(), Some("hello\nworld\ttab".into()));
        let lines = footer.render(80);
        if lines.len() >= 3 {
            assert!(
                !lines[2].contains('\n'),
                "Extension status should not contain newlines"
            );
            assert!(
                !lines[2].contains('\t'),
                "Extension status should not contain tabs"
            );
        }
    }

    #[test]
    fn test_footer_extension_status_removed() {
        let mut footer = make_footer();
        footer.set_extension_status("ext1".into(), Some("ready".into()));
        footer.set_extension_status("ext1".into(), None);
        let lines = footer.render(80);
        assert!(
            lines.len() < 3 || !lines[2].contains("ready"),
            "Extension status should be removed"
        );
    }

    // ── Narrow terminal tests ──

    #[test]
    fn test_footer_handles_narrow_terminal() {
        let mut footer = make_footer();
        footer.set_model_supports_reasoning(true);
        footer.set_thinking_level(Some("high".into()));
        let usage = Usage {
            input_tokens: Some(100000),
            output_tokens: Some(50000),
            cache_tokens: Some(10000),
            cache_write_tokens: None,
            cost_total: None,
        };
        footer.accumulate_usage(&usage);
        footer.set_context(Some(45.0), 128000);
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
        let mut footer = make_footer();
        let lines = footer.render(3);
        assert!(lines.is_empty(), "Should return empty at width 3");
    }

    #[test]
    fn test_footer_stats_not_truncated_when_room() {
        let mut footer = make_footer();
        let usage = Usage {
            input_tokens: Some(100),
            output_tokens: Some(50),
            cache_tokens: None,
            cache_write_tokens: None,
            cost_total: None,
        };
        footer.accumulate_usage(&usage);
        let lines = footer.render(80);
        assert!(lines[1].contains("↑100"), "Should show full token count");
        assert!(lines[1].contains("↓50"), "Should show full token count");
    }

    #[test]
    fn test_footer_line2_exact_width() {
        let mut footer = make_footer();
        let lines = footer.render(80);
        for line in &lines {
            let vw = visible_width(line);
            assert!(vw <= 80, "Line width {} > 80", vw);
        }
    }

    #[test]
    fn test_footer_line2_padded_correctly() {
        let mut footer = make_footer();
        for w in [40, 60, 80, 120] {
            let lines = footer.render(w);
            for line in &lines {
                let vw = visible_width(line);
                assert!(vw <= w, "At width {}: line width {} exceeds", w, vw);
            }
        }
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

    #[test]
    fn test_footer_provider_prefix_when_multiple_providers() {
        let mut footer = make_footer();
        footer.set_model("test-model");
        footer.set_available_provider_count(2);
        let lines = footer.render(80);
        // Provider prefix has "(?)" placeholder since we don't know provider name
        assert!(
            lines[1].contains("(?)"),
            "Should show provider count-based prefix"
        );
    }

    #[test]
    fn test_footer_experimental_indicator() {
        let mut footer = make_footer();
        footer.set_experimental_enabled(true);
        let lines = footer.render(80);
        assert!(
            lines[1].contains("xp"),
            "Should show experimental indicator"
        );
    }

    // ── verify unpadded lines don't exceed width (pi-compatible) ──

    #[test]
    fn test_pwd_line_not_padded() {
        // Pi doesn't pad pwd line to full width — just truncates with ellipsis.
        // Verify the visible width of line 0 doesn't exceed width but also isn't
        // necessarily padded to match width exactly.
        let mut footer = make_footer();
        // Use a short cwd that fits easily
        footer.set_cwd("/home/user");
        let lines = footer.render(80);
        assert!(visible_width(&lines[0]) <= 80, "Pwd line exceeds width");
        // A short path should be less than 80 (not padded) — just the dim text.
        assert!(
            visible_width(&lines[0]) < 80,
            "Pwd line should not be padded to full width (pi behavior)"
        );
    }

    #[test]
    fn test_extension_line_not_padded() {
        let mut footer = make_footer();
        footer.set_extension_status("ext1".into(), Some("short".into()));
        let lines = footer.render(80);
        if lines.len() >= 3 {
            assert!(
                visible_width(&lines[2]) <= 80,
                "Extension line exceeds width"
            );
            assert!(
                visible_width(&lines[2]) < 80,
                "Extension line should not be padded to full width (pi behavior)"
            );
        }
    }
}
