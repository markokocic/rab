use std::cell::RefCell;
use std::rc::Rc;

use crate::agent::footer_data_provider::FooterDataProvider;
use crate::agent::session::Session;
use crate::agent::ui::theme::RabTheme;
use crate::agent::ui::theme::color;
use crate::tui::util::{truncate_to_width, visible_width};
use crate::util::paths;

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

    // Canonicalize both paths to resolve symlinks and `..` (matching pi's `resolve`).
    // Fall back to raw paths if canonicalize fails (e.g. non-existent cwd).
    let resolved_cwd = paths::canonicalize(std::path::Path::new(cwd));
    let resolved_home = paths::canonicalize(std::path::Path::new(home));

    match resolved_cwd.strip_prefix(&resolved_home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => cwd.to_string(),
    }
}

// ── Footer Component ─────────────────────────────────────────────

/// Pi-style footer: 2-3 lines with dim styling.
/// Matches pi's `FooterComponent` in `footer.ts` exactly.
///
/// Architecture (pull-based):
/// - Renders cached usage/context stats refreshed at **turn end** via
///   `refresh_from_session()`, not on every render frame.
/// - Git branch and extension statuses are **pulled** from the
///   `FooterDataProvider` each render, not pushed from the App.
/// - Model/settings state (model name, thinking level, auto-compact)
///   is set directly by the App (these change infrequently mid-session).
pub struct Footer {
    cwd: String,
    session_name: Option<String>,

    // ── Cached usage stats — refreshed at turn end via refresh_from_session() ──
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
    total_cache_write: u64,
    latest_cache_hit_rate: Option<f64>,
    total_cost: f64,

    context_percent: Option<f64>,
    context_window: u64,

    // ── Model / settings state (set directly by App) ──
    model: String,
    model_provider: Option<String>,
    thinking_level: Option<String>,
    auto_compact: bool,
    experimental_enabled: bool,

    // ── Data provider (pull-based: git branch, extension statuses, model/provider) ──
    provider: Rc<RefCell<FooterDataProvider>>,

    theme: RabTheme,
}

impl Footer {
    pub fn new(cwd: impl Into<String>, provider: Rc<RefCell<FooterDataProvider>>) -> Self {
        let theme = crate::agent::ui::theme::current_theme().clone();
        Self {
            cwd: cwd.into(),
            session_name: None,
            total_input: 0,
            total_output: 0,
            total_cache_read: 0,
            total_cache_write: 0,
            latest_cache_hit_rate: None,
            total_cost: 0.0,
            context_percent: None,
            context_window: 0,
            auto_compact: true,
            model: String::new(),
            model_provider: None,
            thinking_level: None,
            experimental_enabled: false,
            provider,
            theme,
        }
    }

    // ── Pull-based refresh (called at turn end) ─────────────────

    /// Refresh cached usage/context stats from session entries.
    /// Called at turn end (AgentEnd) — NOT on every render frame.
    ///
    /// Matches pi's `render()` scanning `sessionManager.getEntries()`,
    /// but the scan happens once per turn instead of once per frame.
    pub fn refresh_from_session(&mut self, session: &Session) {
        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut total_cache_read = 0u64;
        let mut total_cache_write = 0u64;
        let mut total_cost: f64 = 0.0;
        let mut latest_cache_hit_rate: Option<f64> = None;
        // Track the last assistant message's total tokens for context %.
        // usage.input represents the FULL context sent in that request
        // (system + accumulated history). Using the last message avoids
        // summing all historical usage values (which would overcount).
        let mut last_context_tokens: Option<u64> = None;

        // Walk session entries summing usage and cost from all assistant messages.
        // Cost is pre-computed per message and stored in the session (pi-style),
        // so we just sum the stored values — no need to re-resolve models.
        for entry in session.get_entries() {
            if let Some(yoagent::types::Message::Assistant { usage, .. }) = entry.message.as_llm() {
                total_input += usage.input;
                total_output += usage.output;
                total_cache_read += usage.cache_read;
                total_cache_write += usage.cache_write;
                // Keep updating — after the loop this holds the LAST assistant's usage
                last_context_tokens = Some(usage.input + usage.output + usage.cache_read);

                let total_prompt = usage.input + usage.cache_read + usage.cache_write;
                if total_prompt > 0 {
                    latest_cache_hit_rate =
                        Some((usage.cache_read as f64 / total_prompt as f64) * 100.0);
                }

                // Sum pre-computed cost (pi-style: stored per message at creation time)
                total_cost += session.entry_cost(&entry.id).map_or(0.0, |c| c.total);
            }
        }

        self.total_input = total_input;
        self.total_output = total_output;
        self.total_cache_read = total_cache_read;
        self.total_cache_write = total_cache_write;
        self.total_cost = total_cost;
        self.latest_cache_hit_rate = latest_cache_hit_rate;

        // Compute context percentage from the LAST assistant message's
        // total tokens (not the sum of all usage), matching
        // compaction::estimate_context_tokens approach.
        // This avoids massive overcounting from summing all usage.input
        // values (each represents the full context for that request).
        if let Some(ctx_tokens) = last_context_tokens {
            if self.context_window > 0 {
                self.context_percent =
                    Some((ctx_tokens as f64 / self.context_window as f64) * 100.0);
            } else {
                self.context_percent = None;
            }
        } else if self.context_window > 0 {
            // No assistant messages yet — show unknown
            self.context_percent = None;
        } else {
            self.context_percent = None;
        }

        // Update session name from session
        self.session_name = session.session_name().map(|s| s.to_string());

        // Pull model/provider/thinking from the latest session changes
        self.provider.borrow_mut().refresh_from_session(session);

        // Update footer fields from provider (round-trip through session)
        {
            let prov = self.provider.borrow();
            if let Some(mid) = prov.get_model_id() {
                self.model = mid.to_string();
            }
            if let Some(mp) = prov.get_model_provider()
                && !mp.is_empty()
            {
                self.model_provider = Some(mp.to_string());
            }
        }

        // Extract latest thinking level from session
        for entry in session.get_entries() {
            if let yoagent::types::AgentMessage::Extension(ext) = &entry.message
                && ext.kind == "session/thinking_level_change"
                && let Some(level) = ext.data.get("level").and_then(|v| v.as_str())
            {
                self.thinking_level = Some(level.to_string());
            }
        }
    }

    // ── Direct setters (model / settings state) ────────────────

    pub fn set_cwd(&mut self, cwd: impl Into<String>) {
        self.cwd = cwd.into();
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }

    pub fn set_thinking_level(&mut self, level: Option<String>) {
        self.thinking_level = level;
    }

    pub fn set_auto_compact(&mut self, enabled: bool) {
        self.auto_compact = enabled;
    }

    pub fn set_context_window(&mut self, window: u64) {
        self.context_window = window;
        // Don't recompute context_percent here — it's set correctly by
        // refresh_from_session which uses the last assistant's usage.
        // If set_context_window is called before refresh_from_session,
        // context_percent stays None (shown as "?/window").
    }

    pub fn set_experimental_enabled(&mut self, enabled: bool) {
        self.experimental_enabled = enabled;
    }

    /// Pi-style: no streaming dot indicator in footer (handled by working indicator).
    /// Kept for compatibility with existing call sites.
    pub fn set_streaming(&mut self, _streaming: bool) {
        // No-op: pi footer doesn't show streaming dot
    }
}

impl crate::tui::Component for Footer {
    fn render(&mut self, width: usize) -> Vec<String> {
        let w = width;
        if w < 4 {
            return vec![]; // Too narrow to show anything
        }

        let theme = &self.theme;

        // ── Pull git branch and extension statuses from provider ──
        let git_branch = self
            .provider
            .borrow()
            .get_git_branch()
            .map(|s| s.to_string());

        let extension_statuses: Vec<(String, String)> = self
            .provider
            .borrow()
            .get_extension_statuses()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // ── Line 1: pwd (git branch) • session-name ──
        let home = std::env::var("HOME").ok();
        let icon = theme.bold(&theme.fg(color::Accent, "Ⱀ"));
        let dir = format_cwd_for_footer(&self.cwd, home.as_deref());
        let mut pwd = format!("{} {}", icon, theme.fg(color::Dim, &dir));

        if let Some(ref branch) = git_branch {
            pwd = format!("{} {}", pwd, theme.fg(color::Dim, &format!("({})", branch)));
        }
        if let Some(ref name) = self.session_name {
            pwd = format!("{} {}", pwd, theme.fg(color::Dim, &format!("• {}", name)));
        }
        let pwd_line = truncate_to_width(
            &pwd,
            w,
            &theme.fg(color::Dim, "..."),
            false, // pi: no padding
        );

        // ── Line 2: stats left, model right (both dimmed separately) ──
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

        // Cost display (pi-style: $X.XXX or $X.XXX (sub))
        if self.total_cost > 0.0 {
            stats_parts.push(format!("${:.3}", self.total_cost));
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
                    theme.fg(color::Error, &display)
                } else if p > 70.0 {
                    theme.fg(color::Warning, &display)
                } else {
                    display
                }
            }
            None => {
                let window_str = format_tokens(self.context_window);
                if self.context_window > 0 {
                    if self.auto_compact {
                        format!("?/{} (auto)", window_str)
                    } else {
                        format!("?/{}", window_str)
                    }
                } else {
                    // No context window configured — don't show context at all
                    String::new()
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
                theme.fg(color::Dim, "•"),
                theme.bold(&theme.fg(color::Warning, "xp"))
            ));
        }

        let mut stats_left = stats_parts.join(" ");

        // Build right side: model name + thinking level (pi-style)
        let model_name = if self.model.is_empty() {
            "no-model".to_string()
        } else {
            self.model.clone()
        };

        // Always show thinking level if available
        let right_side_without_provider = match &self.thinking_level {
            Some(level) if level != "off" => format!("{} • {}", model_name, level),
            Some(_) => format!("{} • thinking off", model_name),
            None => model_name.clone(),
        };

        // Always prepend provider in parentheses if available (non-empty provider name).
        // Fall back to the cached provider from FooterDataProvider if we don't have
        // a local copy (shouldn't happen — refresh_from_session always sets it).
        let pname = self.model_provider.clone().or_else(|| {
            self.provider.borrow().get_model_provider().and_then(|s| {
                if s.is_empty() {
                    None
                } else {
                    Some(s.to_string())
                }
            })
        });
        let right_side = if let Some(ref pname) = pname {
            format!("({}) {}", pname, right_side_without_provider)
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

        let (stats_line, extra_model_line) =
            if stats_left_width + min_padding + right_side_width <= w {
                // Both fit on one line
                let padding = " ".repeat(w - stats_left_width - right_side_width);
                (format!("{}{}{}", stats_left, padding, right_side), None)
            } else if pname.is_some() {
                // Try without provider prefix
                let without_provider_width = visible_width(&right_side_without_provider);
                if stats_left_width + min_padding + without_provider_width <= w {
                    let padding = " ".repeat(w - stats_left_width - without_provider_width);
                    (
                        format!("{}{}{}", stats_left, padding, right_side_without_provider),
                        None,
                    )
                } else {
                    // Don't fit on one line — put on separate lines
                    let model_for_line = if right_side_width > w {
                        truncate_to_width(&right_side, w, &theme.fg(color::Dim, "..."), false)
                    } else {
                        right_side.clone()
                    };
                    (stats_left.clone(), Some(model_for_line))
                }
            } else {
                // Don't fit on one line — put on separate lines
                let model_for_line = if right_side_width > w {
                    truncate_to_width(&right_side, w, &theme.fg(color::Dim, "..."), false)
                } else {
                    right_side.clone()
                };
                (stats_left.clone(), Some(model_for_line))
            };

        // Pi-style: dim statsLeft and remainder separately
        let dim_stats_left = theme.fg(color::Dim, &stats_left);
        let remainder = &stats_line[stats_left.len()..]; // padding + rightSide (if combined)
        let dim_remainder = theme.fg(color::Dim, remainder);

        let stats_line_formatted = format!("{}{}", dim_stats_left, dim_remainder);

        let mut lines = vec![pwd_line, stats_line_formatted];

        // ── Extra line: model info on its own line (when stats+model don't fit together) ──
        if let Some(model_line) = extra_model_line {
            lines.push(theme.fg(color::Dim, &model_line));
        }

        // ── Last line(s): extension statuses (sorted by key, sanitized) ──
        if !extension_statuses.is_empty() {
            let status_text: Vec<String> = extension_statuses
                .iter()
                .map(|(_, text)| sanitize_status_text(text))
                .collect();
            let status_line = status_text.join(" ");
            let truncated = truncate_to_width(
                &status_line,
                w,
                &theme.fg(color::Dim, "..."),
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

    // ── format_cwd_for_footer tests ──

    #[test]
    fn test_format_cwd_home() {
        assert_eq!(
            format_cwd_for_footer("/home/user/project", Some("/home/user")),
            "~/project"
        );
    }

    #[test]
    fn test_format_cwd_home_exact() {
        assert_eq!(format_cwd_for_footer("/home/user", Some("/home/user")), "~");
    }

    #[test]
    fn test_format_cwd_outside_home() {
        assert_eq!(
            format_cwd_for_footer("/opt/app", Some("/home/user")),
            "/opt/app"
        );
    }

    #[test]
    fn test_format_cwd_no_home() {
        assert_eq!(
            format_cwd_for_footer("/some/path", None::<&str>),
            "/some/path"
        );
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

    // ── Render tests ──

    #[test]
    fn test_footer_render_model_info() {
        let provider = Rc::new(RefCell::new(FooterDataProvider::new("/path".into())));
        provider
            .borrow_mut()
            .set_test_model_provider(Some("test-provider"));

        let mut footer = Footer::new("/path", provider);
        footer.set_model("my-model");
        footer.set_thinking_level(Some("high".into()));
        footer.set_experimental_enabled(true);

        let lines = footer.render(80);
        assert!(lines[1].contains("my-model"), "should show model name");
        assert!(lines[1].contains("high"), "should show thinking level");
        assert!(lines[1].contains("(test-provider)"), "should show provider");
        assert!(
            lines[1].contains("xp"),
            "should show experimental indicator"
        );

        // no-model fallback
        footer.set_model("");
        let lines = footer.render(80);
        assert!(
            lines[1].contains("no-model"),
            "should show 'no-model' fallback"
        );

        // thinking off
        footer.set_model("my-model");
        footer.set_thinking_level(Some("off".into()));
        let lines = footer.render(80);
        assert!(
            lines[1].contains("thinking off"),
            "should show 'thinking off'"
        );
    }

    #[test]
    fn test_footer_render_usage_stats() {
        let provider = Rc::new(RefCell::new(FooterDataProvider::new("/path".into())));
        let mut footer = Footer::new("/path", provider);
        footer.set_model("test-model");

        footer.total_input = 1500;
        footer.total_output = 500;
        footer.total_cache_read = 200;
        footer.latest_cache_hit_rate = Some(16.7);

        let lines = footer.render(80);
        assert!(lines[1].contains("↑1.5k"), "should show input tokens");
        assert!(lines[1].contains("↓500"), "should show output tokens");
        assert!(lines[1].contains("CH16.7%"), "should show cache hit rate");
    }

    #[test]
    fn test_footer_render_context() {
        let provider = Rc::new(RefCell::new(FooterDataProvider::new("/path".into())));
        let mut footer = Footer::new("/path", provider);
        footer.set_model("test-model");

        // With auto-compact enabled and known percent
        footer.set_auto_compact(true);
        footer.context_window = 64000;
        footer.context_percent = Some(50.0);
        let lines = footer.render(80);
        assert!(
            lines[1].contains("50.0%/64k (auto)"),
            "context with auto-compact"
        );

        // Auto-compact disabled
        footer.set_auto_compact(false);
        footer.context_window = 128000;
        footer.context_percent = Some(50.0);
        let lines = footer.render(80);
        assert!(!lines[1].contains("(auto)"), "hide (auto) when disabled");

        // High context (danger color)
        footer.context_percent = Some(95.0);
        footer.context_window = 64000;
        let lines = footer.render(80);
        assert!(
            lines[1].contains("\x1b[38;2;"),
            "high context should have ANSI color"
        );

        // Unknown context
        footer.context_percent = None;
        let lines = footer.render(80);
        assert!(lines[1].contains("?"), "unknown context should show '?'");
        assert!(lines[1].contains("64k"), "should show window size");
    }

    #[test]
    fn test_footer_render_extension_statuses() {
        let provider = Rc::new(RefCell::new(FooterDataProvider::new("/path".into())));
        provider
            .borrow_mut()
            .set_extension_status("z_last", Some("last"));
        provider
            .borrow_mut()
            .set_extension_status("a_first", Some("first"));

        let provider_render = Rc::clone(&provider);
        let mut footer = Footer::new("/path", provider);
        footer.set_model("test-model");
        let lines = footer.render(80);

        assert!(lines.len() >= 3, "should have 3+ lines");
        let ext_line = &lines[2];
        assert!(ext_line.contains("first"), "should show first extension");
        assert!(ext_line.contains("last"), "should show last extension");
        assert!(
            ext_line.find("first") < ext_line.find("last"),
            "extension statuses should be sorted"
        );
        assert!(
            visible_width(ext_line) < 80,
            "extension line should not be padded"
        );

        // Sanitization
        provider_render
            .borrow_mut()
            .set_extension_status("ext1", Some("hello\nworld\ttab"));
        let lines = footer.render(80);
        if lines.len() >= 3 {
            assert!(!lines[2].contains('\n'), "should sanitize newlines");
            assert!(!lines[2].contains('\t'), "should sanitize tabs");
        }

        // Removal
        provider_render
            .borrow_mut()
            .set_extension_status("ext1", None);
        let lines = footer.render(80);
        assert!(
            lines.len() < 3 || !lines[2].contains("ready"),
            "removed status should not appear"
        );
    }

    #[test]
    fn test_footer_render_widths() {
        let provider = Rc::new(RefCell::new(FooterDataProvider::new("/path".into())));
        let mut footer = Footer::new("/path", provider);
        footer.set_model("test-model");
        footer.set_thinking_level(Some("high".into()));

        // Very narrow: width < 4
        assert!(footer.render(3).is_empty(), "width 3 should return empty");

        // Narrow: width 10 — must render within bounds
        let lines = footer.render(10);
        assert!(!lines.is_empty(), "width 10 should not be empty");
        for line in &lines {
            assert!(visible_width(line) <= 10, "line exceeds width 10");
        }

        // Width bounds across several sizes
        for w in [40, 60, 80, 120] {
            let lines = footer.render(w);
            for line in &lines {
                assert!(visible_width(line) <= w, "at width {}: line exceeds", w);
            }
        }

        // Pwd line not padded
        let provider2 = Rc::new(RefCell::new(FooterDataProvider::new("/home/user".into())));
        let mut footer2 = Footer::new("/home/user", provider2);
        footer2.set_model("test-model");
        let lines = footer2.render(80);
        assert!(
            visible_width(&lines[0]) < 80,
            "pwd line should not be padded"
        );
    }
}
