use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::agent::ui::theme::{RabTheme, current_theme};
use crate::tui::Component;
use crate::tui::Style;
use crate::tui::component::{RenderCache, RenderCacheKey};
use crate::tui::components::Text;
use crate::tui::components::r#box::TuiBox;
use crate::tui::components::spacer::Spacer;
use crate::tui::container::Container;

/// Maximum preview lines when collapsed.
const PREVIEW_LINES: usize = 10;

/// Combined tool execution component — delegates rendering to tool-specific
/// ToolRenderer when available, falls back to a simple name+args+output display.
///
/// Background transitions:
/// - Pending (call only, no result) → `toolPendingBg`
/// - Success (call + result, !is_error) → `toolSuccessBg`
/// - Error (call + result, is_error) → `toolErrorBg`
pub struct ToolExecComponent {
    name: String,
    renderer: Option<Arc<dyn ToolRenderer>>,
    args: serde_json::Value,
    output: Option<String>,
    is_error: bool,
    is_complete: bool,
    expanded: bool,
    /// When execution started (for live duration display).
    started_at: Option<Instant>,
    /// Final duration in seconds, captured when the tool completes.
    /// While running, duration is computed at render time from `started_at`.
    final_duration: Option<f64>,
    /// Tracks when to next invalidate for re-render (1s tick).
    last_timer_tick: Option<Instant>,
    /// Tool call ID for this execution (pi's toolCallId).
    tool_call_id: String,
    /// Structured details for UI renderer (not sent to LLM).
    details: Option<serde_json::Value>,
    /// Shared mutable state per tool execution (pi's context.state).
    state: Rc<RefCell<serde_json::Value>>,
    /// Working directory for path resolution in renderers.
    cwd: String,
    /// Invalidation sender (for async preview computation).
    invalidate_tx: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    /// Dirty tracking for efficient re-render.
    dirty: bool,
    /// Render cache.
    cache: Option<RenderCache>,
}

impl ToolExecComponent {
    pub fn new(
        name: impl Into<String>,
        renderer: Option<Arc<dyn ToolRenderer>>,
        args: serde_json::Value,
        cwd: String,
        tool_call_id: String,
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
            tool_call_id,
            details: None,
            state: Rc::new(RefCell::new(serde_json::Value::Object(Default::default()))),
            cwd,
            invalidate_tx: None,
            dirty: true,
            cache: None,
        }
    }

    /// Set the execution start time (for live duration display).
    pub fn set_started_at(&mut self, instant: Instant) {
        self.started_at = Some(instant);
        self.last_timer_tick = Some(instant);
        self.mark_dirty();
    }

    /// Set the invalidation sender for async preview computation.
    pub fn set_invalidate_tx(&mut self, tx: tokio::sync::mpsc::UnboundedSender<()>) {
        self.invalidate_tx = Some(tx);
    }

    /// Append text to the output buffer for live streaming (e.g. bang command output).
    /// Does NOT mark the tool as complete — subsequent `set_result_with_details` finalizes.
    pub fn append_output(&mut self, text: &str) {
        let output = self.output.get_or_insert_with(String::new);
        output.push_str(text);
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

    /// Create an invalidation channel pair for async preview computation.
    pub fn make_invalidation_channel() -> (
        tokio::sync::mpsc::UnboundedSender<()>,
        tokio::sync::mpsc::UnboundedReceiver<()>,
    ) {
        tokio::sync::mpsc::unbounded_channel()
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.cache = None;
    }

    fn live_duration(&self) -> Option<f64> {
        if let Some(dur) = self.final_duration {
            return Some(dur);
        }
        self.started_at.map(|t| t.elapsed().as_secs_f64())
    }

    /// Tick the timer: marks dirty every 1s to trigger re-render.
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

    fn state_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.name.hash(&mut hasher);
        self.args.to_string().hash(&mut hasher);
        self.is_error.hash(&mut hasher);
        self.is_complete.hash(&mut hasher);
        self.live_duration().map(|s| s.to_bits()).hash(&mut hasher);
        self.output.hash(&mut hasher);
        hasher.finish()
    }

    /// Compute the background color key based on execution state.
    fn bg_key(&self) -> &'static str {
        if !self.is_complete {
            "toolPendingBg"
        } else if self.is_error {
            "toolErrorBg"
        } else {
            "toolSuccessBg"
        }
    }

    /// Build the ToolRenderContext from current state.
    fn build_context(&self) -> ToolRenderContext {
        let expand_key = format_key_hint(crate::tui::keybindings::ACTION_APP_TOOLS_EXPAND);
        ToolRenderContext {
            expanded: self.expanded,
            args_complete: self.is_complete,
            is_partial: !self.is_complete,
            is_error: self.is_error,
            tool_call_id: self.tool_call_id.clone(),
            execution_started: self.started_at.is_some(),
            cwd: self.cwd.clone(),
            duration_secs: self.live_duration(),
            exit_code: None,
            cancelled: false,
            was_truncated: false,
            full_output_path: None,
            file_path: None,
            expand_key,
            details: self.details.clone(),
            state: self.state.clone(),
            invalidate: self.invalidate_tx.clone(),
        }
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

        // ── Generic fallback (no tool-specific renderer) ──
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
    /// Render using the tool-specific renderer.
    /// Builds a proper component tree — no string manipulation here.
    fn render_with_renderer(
        &self,
        renderer: &dyn ToolRenderer,
        theme: &RabTheme,
        width: usize,
    ) -> Vec<String> {
        // Pi-style: Spacer(1) before every tool execution
        let mut outer = Container::new();
        outer.add_child(std::boxed::Box::new(Spacer::new(1)));

        let ctx = self.build_context();

        if renderer.render_self() {
            // ── Self-rendering: tool controls its own framing ──
            outer.add_child(renderer.render_call(&self.args, theme, &ctx));
            if let Some(ref output) = self.output
                && let Some(result_comp) = renderer.render_result(output, theme, &ctx)
            {
                outer.add_child(result_comp);
            }
        } else {
            // ── Default shell: colored box wrapping ──
            let bg_key = self.bg_key();
            let bg_ansi = theme.bg_ansi(bg_key);
            let mut msg_box = TuiBox::new(1, 1, Some(Style::new().bg(bg_ansi)));

            msg_box.add_child(renderer.render_call(&self.args, theme, &ctx));

            if let Some(ref output) = self.output
                && let Some(result_comp) = renderer.render_result(output, theme, &ctx)
            {
                msg_box.add_child(result_comp);
            }

            outer.add_child(std::boxed::Box::new(msg_box));
        }

        outer.render(width)
    }

    /// Generic fallback rendering for tools with no renderer.
    fn render_generic(&self, theme: &RabTheme, width: usize) -> Vec<String> {
        let mut outer = Container::new();
        outer.add_child(std::boxed::Box::new(Spacer::new(1)));

        let bg_key = self.bg_key();
        let bg_ansi = theme.bg_ansi(bg_key);
        let mut msg_box = TuiBox::new(1, 1, Some(Style::new().bg(bg_ansi)));

        // Header: bold tool name + JSON args
        let args_str = serde_json::to_string(&self.args).unwrap_or_default();
        let header = if args_str.is_empty() || args_str == "{}" {
            theme.fg("toolTitle", &theme.bold(&self.name))
        } else {
            format!(
                "{}  {}",
                theme.fg("toolTitle", &theme.bold(&self.name)),
                theme.fg("muted", &args_str),
            )
        };
        msg_box.add_child(std::boxed::Box::new(Text::new(header, 0, 0, None)));

        // Output text (collapsed if longer than PREVIEW_LINES, no tool-specific formatting)
        if let Some(ref output) = self.output {
            let display_text = if self.expanded {
                output.clone()
            } else {
                let lines: Vec<&str> = output.lines().collect();
                if lines.len() > PREVIEW_LINES {
                    let preview = lines[..PREVIEW_LINES].join("\n");
                    format!(
                        "{}\n{}",
                        preview,
                        theme.fg(
                            "muted",
                            &format!("... ({} more lines)", lines.len() - PREVIEW_LINES),
                        ),
                    )
                } else {
                    output.clone()
                }
            };

            let fg_key = if self.is_error { "error" } else { "toolOutput" };
            let styled = display_text
                .lines()
                .map(|line| {
                    if line.is_empty() {
                        String::new()
                    } else {
                        theme.fg(fg_key, line)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            msg_box.add_child(std::boxed::Box::new(Text::new(styled, 0, 0, None)));
        }

        outer.add_child(std::boxed::Box::new(msg_box));
        outer.render(width)
    }
}

/// Format a keybinding action as a concise key hint string.
fn format_key_hint(action_id: &str) -> String {
    let keys = crate::tui::keybindings::get_keybindings().get_keys(action_id);
    if keys.is_empty() {
        return String::new();
    }
    keys[0].clone()
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
        None
    }

    fn set_cached_render(&mut self, cache: RenderCache) {
        self.0.borrow_mut().set_cached_render(cache);
    }
}
