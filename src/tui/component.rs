use crossterm::event::KeyEvent;

use crate::tui::util::visible_width;

/// Key for render caching — components return this to indicate when cache is valid.
/// Two renders with the same cache key produce identical output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderCacheKey {
    /// Viewport width.
    pub width: usize,
    /// Whether expanded (for collapsible components).
    pub expanded: bool,
    /// Additional state hash (tool name, args hash, etc.).
    pub state_hash: u64,
}

/// Cached render output.
#[derive(Debug, Clone)]
pub struct RenderCache {
    /// The cache key used to generate this output.
    pub key: RenderCacheKey,
    /// Rendered lines.
    pub lines: Vec<String>,
}

/// Every renderable UI element.
pub trait Component {
    /// Render to lines for the given viewport width.
    /// Each returned string MUST NOT exceed `width` in visible width.
    fn render(&mut self, width: usize) -> Vec<String>;

    /// Render and pad each line to exactly `width` visible columns.
    /// Default implementation calls `render(width)` and pads each line
    /// with trailing spaces if its visible width is less than `width`.
    fn render_padded(&mut self, width: usize) -> Vec<String> {
        let lines = self.render(width);
        lines
            .into_iter()
            .map(|line| {
                let vw = visible_width(&line);
                if vw < width {
                    format!("{}{}", line, " ".repeat(width - vw))
                } else {
                    line
                }
            })
            .collect()
    }

    /// Handle keyboard input. Return `true` if consumed.
    fn handle_input(&mut self, _key: &KeyEvent) -> bool {
        false
    }

    /// Handle a paste event (text from bracketed paste mode).
    /// Default no-op; override to process pasted content.
    fn handle_paste(&mut self, _text: &str) {}

    /// Mark this component as needing re-render.
    /// Called when internal state changes (output received, expanded toggled, etc.).
    fn invalidate(&mut self) {}

    /// Check if this component needs re-render.
    /// Default: false — the Container's per-child cache tracking determines
    /// whether to re-render. Override to return true for components whose
    /// state can change without explicit invalidation (e.g. ToolExecComponent
    /// receiving streaming output).
    fn is_dirty(&self) -> bool {
        false
    }

    /// Clear dirty flag after successful render.
    fn clear_dirty(&mut self) {}

    /// Get the cache key for this component's current state.
    /// Return None to disable caching (always re-render).
    fn cache_key(&self, _width: usize) -> Option<RenderCacheKey> {
        None
    }

    /// Get cached render output, if available and valid.
    fn get_cached_render(&self) -> Option<&RenderCache> {
        None
    }

    /// Store render output in cache.
    fn set_cached_render(&mut self, _cache: RenderCache) {}

    /// Whether this component wants focus (for IME cursor positioning).
    fn is_focusable(&self) -> bool {
        false
    }

    /// Toggle expanded/collapsed state. No-op by default.
    /// Override for components that support expand/collapse (tool results, messages, etc.).
    fn set_expanded(&mut self, _expanded: bool) {}

    /// Toggle thinking block visibility. No-op by default.
    /// Override for components that display thinking content (AssistantMessageComponent).
    fn set_hide_thinking(&mut self, _hide: bool) {}
}
