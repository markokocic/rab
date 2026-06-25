use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::agent::ui::theme::{RabTheme, current_theme};
use crate::tui::Component;
use crate::tui::component::{RenderCache, RenderCacheKey};
use crate::tui::components::Text;
use crate::tui::components::r#box::TuiBox;
use crate::tui::keybindings::{self, ACTION_APP_TOOLS_EXPAND};
use crate::tui::util::truncate_to_width;

/// Maximum preview lines when collapsed (matching pi's collapsible tool result).
const PREVIEW_LINES: usize = 10;

/// Preview line limit for bash tools (matching pi's BASH_PREVIEW_LINES).
const BASH_PREVIEW_LINES: usize = 5;

/// Combined tool execution component - matches pi's `ToolExecutionComponent`.
///
/// Renders tool call + result as ONE component with background transitions:
/// - Pending (call only, no result) → `toolPendingBg`
/// - Success (call + result, !is_error) → `toolSuccessBg`
/// - Error (call + result, is_error) → `toolErrorBg`
///
/// Delegates actual rendering to the tool-specific ToolRenderer when available.
pub struct ToolExecComponent {
    name: String,
    renderer: Option<Box<dyn ToolRenderer>>,
    args: serde_json::Value,
    output: Option<String>,
    is_error: bool,
    is_complete: bool,
    expanded: bool,
    /// When execution started (for live duration display).
    started_at: Option<Instant>,
    /// Final duration in seconds, captured when the tool completes.
    /// While running, duration is computed at render time from `started_at` (pi pattern).
    final_duration: Option<f64>,
    /// Tracks when to next invalidate for re-render (1s tick, matching pi's setInterval).
    last_timer_tick: Option<Instant>,
    // ── Bash-specific fields (used when no renderer) ──
    is_bash: bool,
    was_truncated: bool,
    full_output_path: Option<String>,
    exit_code: Option<i32>,
    cancelled: bool,
    // ── Read-specific (used when no renderer) ──
    file_path: Option<String>,
    // ── Structured details for UI renderer (not sent to LLM) ──
    details: Option<serde_json::Value>,
    // ── Working directory for path resolution in renderers ──
    cwd: String,
    // ── Dirty tracking for efficient re-render ──
    dirty: bool,
    // ── Render cache ──
    cache: Option<RenderCache>,
}

impl ToolExecComponent {
    pub fn new(
        name: impl Into<String>,
        renderer: Option<Box<dyn ToolRenderer>>,
        args: serde_json::Value,
        cwd: String,
    ) -> Self {
        Self {
            name: name.into(),
            renderer,
            args,
            output: None,
            is_error: false,
            is_complete: false,
            expanded: false,
            started_at: None,
            final_duration: None,
            last_timer_tick: None,
            is_bash: false,
            was_truncated: false,
            full_output_path: None,
            exit_code: None,
            cancelled: false,
            file_path: None,
            details: None,
            cwd,
            dirty: true,
            cache: None,
        }
    }

    // ── Legacy setters (used by app.rs for non-renderer paths) ──

    /// Set the execution start time (for live duration display).
    pub fn set_started_at(&mut self, instant: std::time::Instant) {
        self.started_at = Some(instant);
        // Initialize invalidation timer so first tick fires after ~1s
        self.last_timer_tick = Some(instant);
        self.mark_dirty();
    }

    pub fn set_file_path(&mut self, path: impl Into<String>) {
        self.file_path = Some(path.into());
        self.mark_dirty();
    }

    pub fn set_bash(&mut self, is_bash: bool) {
        self.is_bash = is_bash;
        self.mark_dirty();
    }

    /// Set the final duration in seconds (used when the tool completes, e.g. bash).
    /// Freezes the timer at this exact value (no more live computation).
    pub fn set_final_duration(&mut self, secs: f64) {
        self.final_duration = Some(secs);
        self.mark_dirty();
    }

    pub fn set_truncated(&mut self, truncated: bool, full_output_path: Option<String>) {
        self.was_truncated = truncated;
        self.full_output_path = full_output_path;
        self.mark_dirty();
    }

    pub fn set_exit_code(&mut self, code: i32) {
        self.exit_code = Some(code);
        self.mark_dirty();
    }

    pub fn set_cancelled(&mut self, cancelled: bool) {
        self.cancelled = cancelled;
        self.mark_dirty();
    }

    pub fn set_result_with_details(
        &mut self,
        output: impl Into<String>,
        is_error: bool,
        details: Option<serde_json::Value>,
    ) {
        self.output = Some(output.into());
        self.is_error = is_error;
        self.is_complete = true;
        self.details = details;
        // Capture final duration from started_at if not explicitly set via set_final_duration
        // (covers non-bash tools and fast commands that complete before any render).
        if self.final_duration.is_none()
            && let Some(start) = self.started_at
        {
            self.final_duration = Some(start.elapsed().as_secs_f64());
        }
        self.mark_dirty();
    }

    pub fn set_result(&mut self, output: impl Into<String>, is_error: bool) {
        self.set_result_with_details(output, is_error, None);
    }

    pub fn set_args(&mut self, args: serde_json::Value) {
        self.args = args;
        self.mark_dirty();
    }

    /// Mark this component as needing re-render.
    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.cache = None;
    }

    /// Returns the current duration for display.
    /// - If completed: returns `final_duration` (frozen at completion).
    /// - If running: computes live elapsed time from `started_at` (pi pattern).
    fn live_duration(&self) -> Option<f64> {
        if let Some(dur) = self.final_duration {
            return Some(dur);
        }
        self.started_at.map(|t| t.elapsed().as_secs_f64())
    }

    /// Tick the timer: marks dirty every 1s to trigger re-render.
    /// Matches pi's `setInterval(() => context.invalidate(), 1000)` in renderResult.
    /// Duration is computed at render time via `live_duration()`, not stored here.
    /// Returns true if this tick caused a re-render (caller should update dirty).
    pub fn tick_timer(&mut self) -> bool {
        if self.is_complete || self.started_at.is_none() {
            return false;
        }
        let now = Instant::now();
        let should_invalidate = self
            .last_timer_tick
            .is_none_or(|last| now.duration_since(last) >= std::time::Duration::from_secs(1));
        if should_invalidate {
            self.last_timer_tick = Some(now);
            self.mark_dirty();
            return true;
        }
        false
    }

    /// Compute a hash of the current state for cache key.
    fn state_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.name.hash(&mut hasher);
        self.args.to_string().hash(&mut hasher);
        self.is_error.hash(&mut hasher);
        self.is_complete.hash(&mut hasher);
        // Include live duration in hash so cache invalidates when elapsed time changes
        // (component is re-rendered every frame by Container, but cache_check is a no-op).
        self.live_duration().map(|s| s.to_bits()).hash(&mut hasher);
        self.exit_code.hash(&mut hasher);
        self.cancelled.hash(&mut hasher);
        self.was_truncated.hash(&mut hasher);
        self.output.hash(&mut hasher);
        hasher.finish()
    }
}

impl Component for ToolExecComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.mark_dirty();
    }

    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = current_theme();

        // If tool has a renderer, delegate to it
        if let Some(ref renderer) = self.renderer {
            return self.render_with_renderer(renderer.as_ref(), &theme, width);
        }

        // ── Generic fallback rendering (no tool-specific renderer) ──
        self.render_generic(&theme, width)
    }

    fn invalidate(&mut self) {
        self.mark_dirty();
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn cache_key(&self, width: usize) -> Option<RenderCacheKey> {
        // Duration is computed at render time via live_duration(); cache includes the current
        // value so it's invalidated on each render. Container::render() doesn't use caching,
        // so this is effectively a no-op - kept for correctness if caching is added later.
        Some(RenderCacheKey {
            width,
            expanded: self.expanded,
            state_hash: self.state_hash(),
        })
    }

    fn get_cached_render(&self) -> Option<&RenderCache> {
        self.cache.as_ref()
    }

    fn set_cached_render(&mut self, cache: RenderCache) {
        self.cache = Some(cache);
        self.dirty = false;
    }
}

impl ToolExecComponent {
    /// Render using the tool-specific renderer (pi pattern).
    fn render_with_renderer(
        &self,
        renderer: &dyn ToolRenderer,
        theme: &RabTheme,
        width: usize,
    ) -> Vec<String> {
        let is_partial = !self.is_complete;

        // Build render context (matching pi's ToolRenderContext)
        let expand_key = crate::agent::ui::components::tool_messages::format_key_hint(
            crate::tui::keybindings::ACTION_APP_TOOLS_EXPAND,
        );
        let ctx = ToolRenderContext {
            expanded: self.expanded,
            args_complete: self.is_complete,
            is_partial,
            is_error: self.is_error,
            cwd: self.cwd.clone(),
            duration_secs: self.live_duration(),
            exit_code: self.exit_code,
            cancelled: self.cancelled,
            was_truncated: self.was_truncated,
            full_output_path: self.full_output_path.clone(),
            file_path: self.file_path.clone(),
            expand_key,
            details: self.details.clone(),
        };

        // For `renderShell: "self"` tools (like edit), wrap call + result in a
        // single TuiBox with the appropriate background (padding 1,1 matching
        // pi's Box(1,1)). Pi's pattern: both renderCall's call component (Box
        // with bg) and renderResult's additional text go inside selfRenderContainer
        // which is rendered as a single block. We combine them in one TuiBox so
        // the diff preview / result shares the same background as the header.
        if renderer.render_self() {
            let mut lines: Vec<String> = Vec::new();
            // Spacer above (matches pi's Spacer(1) in ToolExecutionComponent constructor)
            lines.push(String::new());

            // Wrap call + result in a colored box with padding 1,1 (matching pi's Box(1,1))
            let bg_key = if !self.is_complete {
                "toolPendingBg"
            } else if self.is_error {
                "toolErrorBg"
            } else {
                "toolSuccessBg"
            };
            let bg_ansi = theme.bg_ansi(bg_key).to_string();
            let mut call_box = TuiBox::new(1, 1, Some(crate::tui::Style::new().bg(bg_ansi)));

            let mut all_content = String::new();

            // Call header
            let call_lines = renderer.render_call(&self.args, width, theme, &ctx);
            if !call_lines.is_empty() {
                all_content.push_str(&call_lines.join("\n"));
            }

            // Result body (in Pi, renderResult updates the call component's preview
            // with the actual diff, so the diff stays inside the colored box)
            if let Some(ref output) = self.output {
                let result_lines = renderer.render_result(output, width, theme, &ctx);
                if !result_lines.is_empty() {
                    if !all_content.is_empty() {
                        all_content.push('\n');
                        // Add a spacer blank line between header and result (matching Pi's
                        // buildEditCallComponent which adds Spacer(1) before the diff body)
                        all_content.push('\n');
                    }
                    all_content.push_str(&result_lines.join("\n"));
                }
            }

            if !all_content.is_empty() {
                let call_text = Text::new(all_content, 0, 0, None);
                call_box.add_child(std::boxed::Box::new(call_text));
                lines.extend(call_box.render(width));
            }
            return lines;
        }

        // ── Default shell: colored box wrapping ──
        let bg_key = if !self.is_complete {
            "toolPendingBg"
        } else if self.is_error {
            "toolErrorBg"
        } else {
            "toolSuccessBg"
        };
        let bg_ansi = theme.bg_ansi(bg_key).to_string();
        let theme_clone = theme.clone();

        let padding_x = 1;
        let content_width = width.saturating_sub(2 * padding_x).max(1);
        let mut msg_box = TuiBox::new(1, 1, Some(crate::tui::Style::new().bg(bg_ansi)));

        // Call header (pass content_width so renderers can do width-dependent
        // formatting like truncate_to_visual_lines)
        let call_lines = renderer.render_call(&self.args, content_width, &theme_clone, &ctx);
        let header_text = Text::new(call_lines.join("\n"), 0, 0, None);
        msg_box.add_child(std::boxed::Box::new(header_text));

        // Result body
        if let Some(ref output) = self.output {
            let result_lines = renderer.render_result(output, content_width, &theme_clone, &ctx);
            if !result_lines.is_empty() {
                let result_text = Text::new(result_lines.join("\n"), 0, 0, None);
                msg_box.add_child(std::boxed::Box::new(result_text));
            }
        }

        msg_box.render(width)
    }

    /// Generic fallback rendering (no tool-specific renderer).
    fn render_generic(&self, theme: &RabTheme, width: usize) -> Vec<String> {
        let bg_key = if !self.is_complete {
            "toolPendingBg"
        } else if self.is_error {
            "toolErrorBg"
        } else {
            "toolSuccessBg"
        };
        let bg_ansi = theme.bg_ansi(bg_key).to_string();

        let mut msg_box = TuiBox::new(1, 1, Some(crate::tui::Style::new().bg(bg_ansi)));

        // ── Header ──
        let header_styled = format_generic_call_header(&self.name, &self.args, theme);
        let header_text = Text::new(header_styled, 0, 0, None);
        msg_box.add_child(std::boxed::Box::new(header_text));

        // ── Result output ──
        let skip_output = self.name == "write" && self.is_complete && !self.is_error;
        if let Some(ref output) = self.output
            && !skip_output
        {
            if self.is_bash {
                msg_box.add_child(std::boxed::Box::new(BashResult::new(
                    output,
                    self.is_error,
                    self.expanded,
                    self.live_duration(),
                    self.was_truncated,
                    self.full_output_path.as_deref(),
                    self.exit_code,
                    self.cancelled,
                    theme,
                )));
            } else {
                let fg_key = if self.is_error { "error" } else { "toolOutput" };
                let fg_ansi = theme.fg_ansi(fg_key).to_string();

                let display_text = if self.expanded {
                    output.clone()
                } else {
                    let lines: Vec<&str> = output.lines().collect();
                    if lines.len() > PREVIEW_LINES {
                        let preview = lines[..PREVIEW_LINES].join("\n");
                        format!(
                            "{}\n... ({} more lines)",
                            preview,
                            lines.len() - PREVIEW_LINES
                        )
                    } else {
                        output.clone()
                    }
                };

                // Apply syntax highlighting for read results
                let styled_lines: Vec<String> = if self.name == "read" && !self.is_error {
                    if let Some(ref path) = self.file_path {
                        let lang = crate::tui::components::path_to_language(path);
                        #[cfg(feature = "syntect")]
                        if lang.is_some() {
                            let hl = crate::tui::components::highlight_code(&display_text, lang);
                            if !hl.is_empty() {
                                hl
                            } else {
                                display_text
                                    .lines()
                                    .map(|line| format!("{}{}\x1b[39m", fg_ansi, line))
                                    .collect()
                            }
                        } else {
                            display_text
                                .lines()
                                .map(|line| format!("{}{}\x1b[39m", fg_ansi, line))
                                .collect()
                        }
                    } else {
                        display_text
                            .lines()
                            .map(|line| format!("{}{}\x1b[39m", fg_ansi, line))
                            .collect()
                    }
                } else {
                    display_text
                        .lines()
                        .map(|line| format!("{}{}\x1b[39m", fg_ansi, line))
                        .collect()
                };

                let result_text = Text::new(styled_lines.join("\n"), 0, 0, None);
                msg_box.add_child(std::boxed::Box::new(result_text));
            }
        }

        msg_box.render(width)
    }
}

/// Format a generic tool call header (fallback when no tool-specific renderer).
fn format_generic_call_header(name: &str, args: &serde_json::Value, theme: &RabTheme) -> String {
    match name {
        "bash" => {
            let cmd = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("...");
            let timeout = args.get("timeout").and_then(|v| v.as_i64());
            let timeout_suffix = timeout
                .map(|t| theme.fg_key(ThemeKey::Muted, &format!(" (timeout {}s)", t)))
                .unwrap_or_default();
            format!(
                "{}{}",
                theme.fg("toolTitle", &theme.bold(&format!("$ {}", cmd))),
                timeout_suffix
            )
        }
        "read" => {
            let path = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let short = shorten_path(path);
            let path_disp = if short.is_empty() {
                String::new()
            } else {
                theme.fg_key(ThemeKey::Accent, &short)
            };
            let range = format_line_range(args, theme);
            format!(
                "{} {} {}",
                theme.fg("toolTitle", &theme.bold("read")),
                path_disp,
                range
            )
        }
        "write" => {
            let path = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let short = shorten_path(path);
            let path_disp = if short.is_empty() {
                String::new()
            } else {
                theme.fg_key(ThemeKey::Accent, &short)
            };
            format!(
                "{} {}",
                theme.fg("toolTitle", &theme.bold("write")),
                path_disp
            )
        }
        "edit" => {
            let path = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let short = shorten_path(path);
            let path_disp = if short.is_empty() {
                String::new()
            } else {
                theme.fg_key(ThemeKey::Accent, &short)
            };
            format!(
                "{} {}",
                theme.fg("toolTitle", &theme.bold("edit")),
                path_disp
            )
        }
        "ls" => {
            let path = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let limit = args.get("limit").and_then(|v| v.as_u64());
            let short = shorten_path(path);
            let limit_str = limit.map(|l| format!(" (limit {})", l)).unwrap_or_default();
            format!(
                "{} {}{}",
                theme.fg("toolTitle", &theme.bold("ls")),
                theme.fg_key(ThemeKey::Accent, &short),
                limit_str
            )
        }
        _ => {
            let args_str = serde_json::to_string(args).unwrap_or_default();
            let suffix = if args_str.is_empty() || args_str == "{}" {
                String::new()
            } else {
                format!("  {}", theme.fg_key(ThemeKey::Muted, &args_str))
            };
            format!("{}{}", theme.fg("toolTitle", &theme.bold(name)), suffix)
        }
    }
}

/// Format line range for read tool (e.g. ":1-10" in warning color).
fn format_line_range(args: &serde_json::Value, theme: &RabTheme) -> String {
    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
    let limit = args.get("limit").and_then(|v| v.as_u64());
    if offset == 0 && limit.is_none() {
        return String::new();
    }
    let start = if offset > 0 { offset } else { 1 };
    let range_str = match limit {
        Some(l) => format!(":{}-{}", start, start + l - 1),
        None => format!(":{}", start),
    };
    theme.fg_key(ThemeKey::Warning, &range_str)
}

/// Shorten a path (replace home with ~).
fn shorten_path(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        path.replacen(&home, "~", 1)
    } else {
        path.to_string()
    }
}

/// Format a keybinding action as a concise key hint string.
fn format_key_hint(action_id: &str) -> String {
    // Pi-style key formatting: returns the raw key ID string (e.g. "ctrl+o")
    // rather than Emacs-style notation ("C-o"). Matches pi's keyText().
    let keys = keybindings::get_keybindings().get_keys(action_id);
    if keys.is_empty() {
        return String::new();
    }
    keys[0].clone()
}

// ═══════════════════════════════════════════════════════════════════
// Bash-specific result rendering (legacy fallback when no renderer)
// ═══════════════════════════════════════════════════════════════════

struct BashResult {
    output: String,
    is_error: bool,
    expanded: bool,
    duration_secs: Option<f64>,
    was_truncated: bool,
    full_output_path: Option<String>,
    exit_code: Option<i32>,
    cancelled: bool,
    theme: RabTheme,
}

impl BashResult {
    #[allow(clippy::too_many_arguments)]
    fn new(
        output: &str,
        is_error: bool,
        expanded: bool,
        duration_secs: Option<f64>,
        was_truncated: bool,
        full_output_path: Option<&str>,
        exit_code: Option<i32>,
        cancelled: bool,
        theme: &RabTheme,
    ) -> Self {
        let clean_output = strip_context_truncation_footer(output);
        Self {
            output: clean_output,
            is_error,
            expanded,
            duration_secs,
            was_truncated,
            full_output_path: full_output_path.map(|s| s.to_string()),
            exit_code,
            cancelled,
            theme: theme.clone(),
        }
    }
}

impl Component for BashResult {
    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = &self.theme;
        let fg_ansi = if self.is_error {
            theme.fg_ansi_key(ThemeKey::Error)
        } else {
            theme.fg_ansi("toolOutput")
        }
        .to_string();
        let dim_ansi = theme.fg_ansi_key(ThemeKey::Muted).to_string();
        let warning_ansi = theme.fg_ansi_key(ThemeKey::Warning).to_string();
        let expand_key = format_key_hint(ACTION_APP_TOOLS_EXPAND);

        let mut lines: Vec<String> = Vec::new();

        let all_lines: Vec<&str> = self.output.split('\n').collect();

        if all_lines.is_empty() || (all_lines.len() == 1 && all_lines[0].is_empty()) {
            return lines;
        }

        // Use visual-line-aware truncation for preview
        let (preview_lines, hidden_line_count) = if self.expanded {
            (all_lines.clone(), 0)
        } else {
            truncate_to_visual_lines(&all_lines, width, BASH_PREVIEW_LINES)
        };

        if !self.expanded && hidden_line_count > 0 {
            let hint = if expand_key.is_empty() {
                format!(
                    "\x1b[0m{}... {} earlier lines\x1b[39m",
                    dim_ansi, hidden_line_count
                )
            } else {
                format!(
                    "\x1b[0m{}... ({} earlier lines, {} to expand)\x1b[39m",
                    dim_ansi, hidden_line_count, expand_key,
                )
            };
            let truncated = truncate_to_width(&hint, width, "...", false);
            lines.push(truncated);
        }

        for line in &preview_lines {
            let styled = if line.is_empty() {
                String::new()
            } else {
                format!("{}{}\x1b[39m", fg_ansi, line)
            };
            let truncated = truncate_to_width(&styled, width, "...", false);
            lines.push(truncated);
        }

        let is_complete = self.exit_code.is_some() || self.cancelled;
        if let Some(secs) = self.duration_secs {
            let label = if is_complete { "Took" } else { "Elapsed" };
            let duration_text = format!("{}{} {:.1}s\x1b[39m", dim_ansi, label, secs);
            lines.push(duration_text);
        }

        // Pi does not add separate exit code or cancelled status lines because
        // the tool result content already includes "Command exited with code N" or
        // "Command aborted" from the tool error response.

        if self.was_truncated {
            if let Some(ref path) = self.full_output_path {
                lines.push(format!(
                    "{}Output truncated. Full output: {}\x1b[39m",
                    warning_ansi, path
                ));
            } else {
                lines.push(format!("{}Output truncated.\x1b[39m", warning_ansi));
            }
        }

        lines
    }

    fn invalidate(&mut self) {}
}

// ═══════════════════════════════════════════════════════════════════
// Visual-line-aware truncation (delegated to shared tui::visual_truncate)
// ═══════════════════════════════════════════════════════════════════

use crate::agent::ui::theme::ThemeKey;
use crate::tui::visual_truncate::truncate_to_visual_lines;

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

// ═══════════════════════════════════════════════════════════════════
// Rc wrapper for shared ownership
// ═══════════════════════════════════════════════════════════════════

pub struct RcToolExec(pub Rc<RefCell<ToolExecComponent>>);

impl Clone for RcToolExec {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Component for RcToolExec {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.0.borrow_mut().render(width)
    }

    fn set_expanded(&mut self, expanded: bool) {
        self.0.borrow_mut().set_expanded(expanded);
    }

    fn invalidate(&mut self) {
        self.0.borrow_mut().invalidate();
    }

    fn is_dirty(&self) -> bool {
        self.0.borrow().is_dirty()
    }

    fn clear_dirty(&mut self) {
        self.0.borrow_mut().clear_dirty();
    }

    fn cache_key(&self, width: usize) -> Option<RenderCacheKey> {
        self.0.borrow().cache_key(width)
    }

    fn get_cached_render(&self) -> Option<&RenderCache> {
        // Can't return reference into RefCell - cache is managed by inner component
        None
    }

    fn set_cached_render(&mut self, cache: RenderCache) {
        self.0.borrow_mut().set_cached_render(cache);
    }
}

// ═══════════════════════════════════════════════════════════════════
// Backward-compatible old types
// ═══════════════════════════════════════════════════════════════════

pub struct ToolCallComponent {
    name: String,
    args: String,
    expanded: bool,
}

impl ToolCallComponent {
    pub fn new(name: impl Into<String>, args: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: args.into(),
            expanded: false,
        }
    }
}

impl Component for ToolCallComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let bg_ansi = theme.bg_ansi_key(ThemeKey::ToolPendingBg).to_string();

        let mut styled = String::new();
        styled.push_str("\x1b[1m");
        styled.push_str(theme.fg_ansi("toolTitle"));
        styled.push_str(&self.name);
        styled.push_str("\x1b[22m");

        if !self.args.is_empty() && self.args != "{}" {
            styled.push_str("  ");
            styled.push_str(theme.fg_ansi_key(ThemeKey::Muted));
            styled.push_str(&self.args);
        }
        styled.push_str("\x1b[39m");

        let mut msg_box = TuiBox::new(1, 1, Some(crate::tui::Style::new().bg(bg_ansi)));
        msg_box.add_child(std::boxed::Box::new(Text::new(styled, 0, 0, None)));
        msg_box.render(width)
    }
    fn invalidate(&mut self) {}
}

pub struct ToolResultComponent {
    content: String,
    is_error: bool,
    expanded: bool,
}

impl ToolResultComponent {
    pub fn new(content: impl Into<String>, is_error: bool) -> Self {
        Self {
            content: content.into(),
            is_error,
            expanded: false,
        }
    }
}

impl Component for ToolResultComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let bg_key = if self.is_error {
            "toolErrorBg"
        } else {
            "toolSuccessBg"
        };
        let fg_key = if self.is_error { "error" } else { "toolOutput" };
        let bg_ansi = theme.bg_ansi(bg_key).to_string();
        let styled = theme.fg(fg_key, &self.content);

        let mut msg_box = TuiBox::new(1, 0, Some(crate::tui::Style::new().bg(bg_ansi)));
        msg_box.add_child(std::boxed::Box::new(Text::new(styled, 0, 0, None)));
        msg_box.render(width)
    }
    fn invalidate(&mut self) {}
}

#[cfg(test)]
mod tests {
    use crate::tui::visual_truncate::{truncate_to_visual_lines, visual_line_count};
    #[test]
    fn test_visual_line_count_ascii() {
        assert_eq!(visual_line_count("hello", 80), 1);
        assert_eq!(visual_line_count("", 80), 1);
    }

    #[test]
    fn test_visual_line_count_wrapping() {
        // 100 chars at width 80 = 2 visual lines
        let line = "a".repeat(100);
        assert_eq!(visual_line_count(&line, 80), 2);

        // 160 chars at width 80 = 2 visual lines
        let line = "a".repeat(160);
        assert_eq!(visual_line_count(&line, 80), 2);

        // 161 chars at width 80 = 3 visual lines
        let line = "a".repeat(161);
        assert_eq!(visual_line_count(&line, 80), 3);
    }

    #[test]
    fn test_visual_line_count_zero_width() {
        assert_eq!(visual_line_count("hello", 0), 1);
    }

    #[test]
    fn test_truncate_to_visual_lines_no_truncation() {
        let lines = vec!["short", "also short"];
        let (selected, hidden) = truncate_to_visual_lines(&lines, 80, 10);
        assert_eq!(selected.len(), 2);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn test_truncate_to_visual_lines_with_wrapping() {
        // Create lines that wrap: each is 100 chars at width 80 = 2 visual lines each
        let line1 = "a".repeat(100);
        let line2 = "b".repeat(100);
        let line3 = "c".repeat(100);
        let lines = vec![line1.as_str(), line2.as_str(), line3.as_str()];

        // 3 lines × 2 visual lines each = 6 visual lines total
        // Request only 4 visual lines -> should show last 2 logical lines (4 visual)
        let (selected, hidden) = truncate_to_visual_lines(&lines, 80, 4);
        assert_eq!(selected.len(), 2);
        assert_eq!(hidden, 1);
        assert_eq!(selected[0], line2.as_str());
        assert_eq!(selected[1], line3.as_str());
    }

    #[test]
    fn test_truncate_to_visual_lines_exact_fit() {
        // 2 lines × 2 visual lines = 4 visual lines, request 4
        let line1 = "a".repeat(100);
        let line2 = "b".repeat(100);
        let lines = vec![line1.as_str(), line2.as_str()];

        let (selected, hidden) = truncate_to_visual_lines(&lines, 80, 4);
        assert_eq!(selected.len(), 2);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn test_truncate_to_visual_lines_empty() {
        let lines: Vec<&str> = vec![];
        let (selected, hidden) = truncate_to_visual_lines(&lines, 80, 5);
        assert!(selected.is_empty());
        assert_eq!(hidden, 0);
    }

    #[test]
    fn test_truncate_to_visual_lines_mixed_widths() {
        // Mix of short (1 visual) and long (2 visual) lines
        let short1 = "short";
        let long = "x".repeat(100); // 2 visual lines
        let short2 = "also short";
        let lines = vec![short1, long.as_str(), short2];

        // Total: 1 + 2 + 1 = 4 visual lines
        // Request 3 visual lines -> should skip short1 (1 visual) and show long + short2 (3 visual)
        let (selected, hidden) = truncate_to_visual_lines(&lines, 80, 3);
        assert_eq!(selected.len(), 2);
        assert_eq!(hidden, 1);
        assert_eq!(selected[0], long.as_str());
        assert_eq!(selected[1], short2);
    }
}
