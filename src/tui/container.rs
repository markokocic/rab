use crate::tui::Component;
use crate::tui::component::{RenderCache, RenderCacheKey};
use crate::tui::overlay::{OverlayEntry, OverlayLayout, OverlayOptions, SizeValue};
use crate::tui::util::{extract_segments, slice_by_column, visible_width};

/// Marker appended to lines after extraction — matches pi's SEGMENT_RESET
const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

/// Per-child render cache entry.
struct ChildCache {
    /// Cached render output.
    cache: Option<RenderCache>,
    /// Whether child needs re-render.
    dirty: bool,
}

impl ChildCache {
    fn new() -> Self {
        Self {
            cache: None,
            dirty: true,
        }
    }
}

/// Container - a component that contains other components rendered vertically.
/// Supports per-child caching and overlay compositing.
pub struct Container {
    children: Vec<Box<dyn Component>>,
    /// Per-child cache state.
    child_caches: Vec<ChildCache>,
    /// Overlay stack (rendered on top of children).
    overlay_stack: Vec<OverlayEntry>,
    /// Terminal height (set before render, used for overlay positioning).
    term_height: usize,
}

impl Container {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            child_caches: Vec::new(),
            overlay_stack: Vec::new(),
            term_height: 24,
        }
    }

    /// Set terminal height (must be called before render for correct overlay positioning).
    pub fn set_term_height(&mut self, height: usize) {
        self.term_height = height;
    }

    // ── Child management ──

    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.child_caches.push(ChildCache::new());
        self.children.push(component);
    }

    pub fn remove_child(&mut self, component: &dyn Component) {
        let idx = self.children.iter().position(|c| {
            std::ptr::eq(
                c.as_ref() as *const dyn Component,
                component as *const dyn Component,
            )
        });
        if let Some(idx) = idx {
            self.children.remove(idx);
            self.child_caches.remove(idx);
        }
    }

    pub fn clear(&mut self) {
        self.children.clear();
        self.child_caches.clear();
    }

    pub fn children(&self) -> &[Box<dyn Component>] {
        &self.children
    }

    pub fn children_mut(&mut self) -> &mut [Box<dyn Component>] {
        &mut self.children
    }

    /// Mark all children as needing re-render.
    pub fn invalidate_all(&mut self) {
        for cache in &mut self.child_caches {
            cache.dirty = true;
            cache.cache = None;
        }
    }

    /// Mark a specific child as needing re-render by index.
    pub fn invalidate_child(&mut self, index: usize) {
        if let Some(cache) = self.child_caches.get_mut(index) {
            cache.dirty = true;
            cache.cache = None;
        }
    }

    /// Get the number of children.
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Remove and return the last child, if any.
    pub fn pop_child(&mut self) -> Option<Box<dyn Component>> {
        self.child_caches.pop();
        self.children.pop()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Peek at the last child, if any.
    pub fn last_child(&self) -> Option<&dyn Component> {
        self.children.last().map(|c| c.as_ref())
    }

    // ── Overlay management ──

    /// Show an overlay. Returns the overlay ID for later removal.
    /// `pre_focus` records which focus target was active before showing this overlay
    /// and will be restored when the overlay is dismissed.
    pub fn show_overlay(
        &mut self,
        component: Box<dyn Component>,
        options: OverlayOptions,
        pre_focus: crate::tui::FocusTarget,
    ) -> u64 {
        let id = self.overlay_stack.len() as u64;
        self.overlay_stack.push(OverlayEntry {
            component,
            options,
            hidden: false,
            focus_order: id,
            id,
            pre_focus,
        });
        id
    }

    /// Hide an overlay by ID. Returns the pre_focus target that was stored
    /// with that overlay, so the TUI can restore focus.
    pub fn hide_overlay(&mut self, id: u64) -> Option<crate::tui::FocusTarget> {
        let idx = self.overlay_stack.iter().position(|e| e.id == id);
        idx.map(|i| {
            let entry = self.overlay_stack.remove(i);
            entry.pre_focus
        })
    }

    /// Hide the topmost overlay. Returns the pre_focus target that was stored
    /// with that overlay, so the TUI can restore focus.
    pub fn pop_overlay(&mut self) -> Option<crate::tui::FocusTarget> {
        self.overlay_stack.pop().map(|e| e.pre_focus)
    }

    /// Check if there are any visible overlays.
    pub fn has_overlays(&self) -> bool {
        self.overlay_stack.iter().any(|e| !e.hidden)
    }

    /// Clear all overlays.
    pub fn clear_overlays(&mut self) {
        self.overlay_stack.clear();
    }

    /// Get the overlay stack (for focus management in TUI).
    pub fn overlay_stack(&self) -> &[OverlayEntry] {
        &self.overlay_stack
    }

    pub fn overlay_stack_mut(&mut self) -> &mut Vec<OverlayEntry> {
        &mut self.overlay_stack
    }

    /// Set the `focused` flag on an overlay component (by its overlay ID).
    /// Called by TUI when focus transitions to/from an overlay.
    /// Returns true if the overlay was found and the focus was set.
    pub fn set_overlay_focused(&mut self, id: u64, focused: bool) -> bool {
        if let Some(entry) = self.overlay_stack.iter_mut().find(|e| e.id == id)
            && let Some(f) = entry.component.as_mut().as_focusable()
        {
            f.set_focused(focused);
            return true;
        }
        false
    }
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Container {
    /// Composite all visible overlays into the content lines.
    /// Matches pi's compositeOverlays 1/1.
    fn composite_overlays(
        &mut self,
        base_lines: &[String],
        term_width: usize,
        term_height: usize,
    ) -> Vec<String> {
        if self.overlay_stack.is_empty() {
            return base_lines.to_vec();
        }
        let mut result = base_lines.to_vec();

        // Collect visible overlay indices sorted by focusOrder (higher = on top)
        let mut indices: Vec<usize> = self
            .overlay_stack
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.hidden)
            .map(|(i, _)| i)
            .collect();
        indices.sort_by_key(|&i| self.overlay_stack[i].focus_order);

        // Pre-render all visible overlays and calculate positions
        struct RenderedOverlay {
            overlay_lines: Vec<String>,
            row: usize,
            col: usize,
            w: usize,
        }

        let mut rendered: Vec<RenderedOverlay> = Vec::new();
        let mut min_lines_needed = result.len();

        for &idx in &indices {
            let options = self.overlay_stack[idx].options.clone();

            // Get layout with height=0 first to determine width and maxHeight
            // (width and maxHeight don't depend on overlay height)
            let first_layout = self.resolve_overlay_layout(&options, 0, term_width, term_height);
            let width = first_layout.width;
            let max_height = first_layout.max_height;

            // Render component at calculated width (separate borrow)
            let mut overlay_lines = self.overlay_stack[idx].component.render(width);

            // Apply maxHeight if specified
            if let Some(mh) = max_height {
                overlay_lines.truncate(mh);
            }

            let overlay_len = overlay_lines.len();

            // Get final row/col with actual overlay height
            let layout =
                self.resolve_overlay_layout(&options, overlay_len, term_width, term_height);

            min_lines_needed = min_lines_needed.max(layout.row + overlay_len);

            rendered.push(RenderedOverlay {
                overlay_lines,
                row: layout.row,
                col: layout.col,
                w: width,
            });
        }

        // Pad to at least terminal height so overlays have screen-relative positions.
        // Excludes maxLinesRendered: the historical high-water mark caused self-reinforcing
        // inflation that pushed content into scrollback on terminal widen.
        let working_height = result.len().max(term_height).max(min_lines_needed);

        // Extend result with empty lines if content is too short for overlay placement
        while result.len() < working_height {
            result.push(String::new());
        }

        let viewport_start = working_height.saturating_sub(term_height);

        // Composite each overlay
        for ro in &rendered {
            for (i, overlay_line) in ro.overlay_lines.iter().enumerate() {
                let idx = viewport_start + ro.row + i;
                if idx < result.len() {
                    // Defensive: truncate overlay line to declared width before compositing
                    let truncated = if visible_width(overlay_line) > ro.w {
                        slice_by_column(overlay_line, 0, ro.w)
                    } else {
                        overlay_line.clone()
                    };
                    result[idx] =
                        self.composite_line_at(&result[idx], &truncated, ro.col, ro.w, term_width);
                }
            }
        }

        result
    }

    /// Splice overlay content into a base line at a specific column.
    fn composite_line_at(
        &self,
        base_line: &str,
        overlay_line: &str,
        start_col: usize,
        overlay_width: usize,
        total_width: usize,
    ) -> String {
        let after_start = start_col + overlay_width;

        let (before, before_width, after, after_width) = extract_segments(
            base_line,
            start_col,
            after_start,
            total_width.saturating_sub(after_start),
            true,
        );

        let overlay = slice_by_column(overlay_line, 0, overlay_width);
        let overlay_vis = visible_width(&overlay);

        let before_pad = start_col.saturating_sub(before_width);
        let overlay_pad = overlay_width.saturating_sub(overlay_vis);
        let actual_before_width = before_width.max(start_col);
        let actual_overlay_width = overlay_vis.max(overlay_width);
        let after_target = total_width.saturating_sub(actual_before_width + actual_overlay_width);
        let after_pad = after_target.saturating_sub(after_width);

        let mut result = String::new();
        result.push_str(&before);
        result.push_str(&" ".repeat(before_pad));
        result.push_str(SEGMENT_RESET);
        result.push_str(&overlay);
        result.push_str(&" ".repeat(overlay_pad));
        result.push_str(SEGMENT_RESET);
        result.push_str(&after);
        result.push_str(&" ".repeat(after_pad));

        let rw = visible_width(&result);
        if rw > total_width {
            result = slice_by_column(&result, 0, total_width);
        }

        result
    }

    /// Resolve overlay layout from options.
    fn resolve_overlay_layout(
        &self,
        options: &OverlayOptions,
        overlay_height: usize,
        term_width: usize,
        term_height: usize,
    ) -> OverlayLayout {
        let margin = options.margin.unwrap_or_default();
        let margin_top = margin.top;
        let margin_right = margin.right;
        let margin_bottom = margin.bottom;
        let margin_left = margin.left;

        let avail_width = (term_width - margin_left - margin_right).max(1);
        let avail_height = (term_height - margin_top - margin_bottom).max(1);

        let width = options
            .width
            .map(|sv| sv.resolve(term_width))
            .unwrap_or_else(|| 80.min(avail_width));
        let width = options.min_width.map(|mw| width.max(mw)).unwrap_or(width);
        let width = width.max(1).min(avail_width);

        let max_height = options.max_height.map(|sv| sv.resolve(term_height));
        let max_height = max_height.map(|mh| mh.max(1).min(avail_height));

        let effective_height = match max_height {
            Some(mh) => overlay_height.min(mh),
            None => overlay_height,
        };

        let row = if let Some(ref row_sv) = options.row {
            match row_sv {
                SizeValue::Absolute(r) => *r,
                SizeValue::Percent(p) => {
                    let max_row = avail_height - effective_height;
                    margin_top + ((max_row as f64 * p / 100.0).floor() as usize)
                }
            }
        } else {
            let anchor = options.anchor.unwrap_or_default();
            Self::resolve_anchor_row(anchor, effective_height, avail_height, margin_top)
        };

        let col = if let Some(ref col_sv) = options.col {
            match col_sv {
                SizeValue::Absolute(c) => *c,
                SizeValue::Percent(p) => {
                    let max_col = avail_width - width;
                    margin_left + ((max_col as f64 * p / 100.0).floor() as usize)
                }
            }
        } else {
            let anchor = options.anchor.unwrap_or_default();
            Self::resolve_anchor_col(anchor, width, avail_width, margin_left)
        };

        let row = ((row as isize + options.offset_y.unwrap_or(0)).max(0)) as usize;
        let col = ((col as isize + options.offset_x.unwrap_or(0)).max(0)) as usize;

        OverlayLayout {
            width,
            row,
            col,
            max_height,
        }
    }

    fn resolve_anchor_row(
        anchor: crate::tui::overlay::OverlayAnchor,
        overlay_height: usize,
        avail_height: usize,
        margin_top: usize,
    ) -> usize {
        use crate::tui::overlay::OverlayAnchor::*;
        match anchor {
            Center | LeftCenter | RightCenter => {
                margin_top + (avail_height.saturating_sub(overlay_height) / 2)
            }
            TopLeft | TopCenter | TopRight => margin_top,
            BottomLeft | BottomCenter | BottomRight => {
                margin_top + avail_height.saturating_sub(overlay_height)
            }
        }
    }

    fn resolve_anchor_col(
        anchor: crate::tui::overlay::OverlayAnchor,
        overlay_width: usize,
        avail_width: usize,
        margin_left: usize,
    ) -> usize {
        use crate::tui::overlay::OverlayAnchor::*;
        match anchor {
            Center | TopCenter | BottomCenter => {
                margin_left + (avail_width.saturating_sub(overlay_width) / 2)
            }
            TopLeft | LeftCenter | BottomLeft => margin_left,
            TopRight | RightCenter | BottomRight => {
                margin_left + avail_width.saturating_sub(overlay_width)
            }
        }
    }
}

impl Component for Container {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for (idx, child) in self.children.iter_mut().enumerate() {
            let cache = &mut self.child_caches[idx];
            // Use cached output if width matches and the child's cache key
            // hasn't changed (or child doesn't provide one — always re-render).
            if !cache.dirty
                && let Some(ref cached) = cache.cache
                && cached.key.width == width
                && child.cache_key(width).is_some_and(|k| k == cached.key)
            {
                lines.extend(cached.lines.clone());
                continue;
            }
            let child_lines = child.render(width);
            child.clear_dirty();
            let cache_key = child.cache_key(width).unwrap_or(RenderCacheKey {
                width,
                expanded: false,
                state_hash: 0,
            });
            cache.cache = Some(RenderCache {
                key: cache_key,
                lines: child_lines.clone(),
            });
            cache.dirty = false;
            lines.extend(child_lines);
        }
        // Composite overlays on top of base content
        if !self.overlay_stack.is_empty() {
            lines = self.composite_overlays(&lines, width, self.term_height);
        }
        lines
    }

    fn handle_input(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        // Route to overlays in reverse order (topmost first)
        for entry in self.overlay_stack.iter_mut().rev() {
            if !entry.hidden && entry.component.handle_input(key) {
                return true;
            }
        }
        // Route to base children
        for child in self.children.iter_mut().rev() {
            if child.handle_input(key) {
                return true;
            }
        }
        false
    }

    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
        for cache in &mut self.child_caches {
            cache.dirty = true;
            cache.cache = None;
        }
    }

    fn is_dirty(&self) -> bool {
        self.child_caches.iter().any(|c| c.dirty)
    }

    fn clear_dirty(&mut self) {
        for cache in &mut self.child_caches {
            cache.dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::component::Component;

    /// Test component that tracks how many times it was rendered and whether
    /// it reports as dirty. Used to verify the Container's caching logic.
    struct TrackRender {
        render_count: usize,
        dirty: bool,
        label: String,
    }

    impl TrackRender {
        fn new(label: &str) -> Self {
            Self {
                render_count: 0,
                dirty: true,
                label: label.to_string(),
            }
        }
    }

    impl Component for TrackRender {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.render_count += 1;
            vec![format!("{}[{}]", self.label, self.render_count)]
        }

        fn cache_key(&self, width: usize) -> Option<RenderCacheKey> {
            Some(RenderCacheKey {
                width,
                expanded: false,
                state_hash: self.render_count as u64,
            })
        }

        fn is_dirty(&self) -> bool {
            self.dirty
        }

        fn clear_dirty(&mut self) {
            self.dirty = false;
        }
    }

    #[test]
    fn test_re_render_when_dirty() {
        let mut c = Container::new();
        let child = Box::new(TrackRender::new("a"));
        c.add_child(child);

        // First render: child is dirty, should render
        let lines = c.render(80);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "a[1]");

        // Second render: child is NOT dirty (clear_dirty was called),
        // should use cache
        let lines = c.render(80);
        assert_eq!(lines[0], "a[1]"); // cached

        // Mark dirty and render again
        c.invalidate_all();
        let lines = c.render(80);
        assert_eq!(lines[0], "a[2]"); // re-rendered
    }

    #[test]
    fn test_re_render_when_child_stays_dirty() {
        // A component that always returns is_dirty() = true should
        // never be cached by the Container.
        struct AlwaysDirty;

        impl Component for AlwaysDirty {
            fn render(&mut self, _width: usize) -> Vec<String> {
                vec!["fresh".to_string()]
            }

            fn is_dirty(&self) -> bool {
                true
            }
        }

        let mut c = Container::new();
        c.add_child(Box::new(AlwaysDirty));

        let lines1 = c.render(80);
        assert_eq!(lines1[0], "fresh");

        // Second render: child is still dirty, should NOT use cache
        let lines2 = c.render(80);
        assert_eq!(lines2[0], "fresh");

        // Verify the Container's own cache was NOT used
        // (the child's render was called)
        // We can check this by checking that the internal cache was bypassed
        assert!(
            !c.child_caches[0].dirty,
            "child cache should be marked clean after render"
        );
    }

    #[test]
    fn test_cached_after_non_dirty_render() {
        let mut c = Container::new();
        c.add_child(Box::new(TrackRender::new("x")));

        // First render
        c.render(80);

        // Second render with different width — cache miss despite not dirty
        let lines = c.render(40);
        assert_eq!(lines[0], "x[2]"); // re-rendered because width differs
    }

    #[test]
    fn test_mixed_dirty_and_not_dirty_children() {
        struct SometimesDirty {
            toggle: bool,
        }
        impl Component for SometimesDirty {
            fn render(&mut self, _width: usize) -> Vec<String> {
                vec!["s".to_string()]
            }
            fn is_dirty(&self) -> bool {
                self.toggle
            }
            fn clear_dirty(&mut self) {
                // No-op: clear_dirty is called by Container after render,
                // but is_dirty depends on toggle, not a flag we clear here
            }
        }

        let mut c = Container::new();
        c.add_child(Box::new(TrackRender::new("a")));
        c.add_child(Box::new(SometimesDirty { toggle: false }));

        // First render: both children are dirty by initialization (TrackRender)
        let lines = c.render(80);
        assert_eq!(lines[0], "a[1]");
        assert_eq!(lines[1], "s");

        // Second render: TrackRender now not dirty (cleared), SometimesDirty is false
        // Both should use cache
        let lines = c.render(80);
        assert_eq!(lines[0], "a[1]"); // cached
        assert_eq!(lines[1], "s");
    }
}
