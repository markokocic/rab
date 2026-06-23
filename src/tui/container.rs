use crate::tui::Component;
use crate::tui::component::RenderCache;
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

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    // ── Overlay management ──

    /// Show an overlay. Returns the overlay ID for later removal.
    pub fn show_overlay(&mut self, component: Box<dyn Component>, options: OverlayOptions) -> u64 {
        let id = self.overlay_stack.len() as u64;
        self.overlay_stack.push(OverlayEntry {
            component,
            options,
            pre_focus: None,
            hidden: false,
            focus_order: id,
            id,
        });
        id
    }

    /// Hide an overlay by ID.
    pub fn hide_overlay(&mut self, id: u64) {
        self.overlay_stack.retain(|e| e.id != id);
    }

    /// Hide the topmost overlay.
    pub fn pop_overlay(&mut self) {
        self.overlay_stack.pop();
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
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Container {
    /// Composite all visible overlays into the content lines.
    fn composite_overlays(
        &mut self,
        base_lines: &[String],
        term_width: usize,
        term_height: usize,
    ) -> Vec<String> {
        let mut result = base_lines.to_vec();

        // Collect visible overlay indices sorted by focus order
        let mut indices: Vec<usize> = self
            .overlay_stack
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.hidden)
            .map(|(i, _)| i)
            .collect();
        indices.sort_by_key(|&i| self.overlay_stack[i].focus_order);

        let mut min_lines_needed = result.len();

        struct RenderedOverlay {
            overlay_lines: Vec<String>,
            layout: OverlayLayout,
        }

        let mut rendered: Vec<RenderedOverlay> = Vec::new();
        for &idx in &indices {
            let options = self.overlay_stack[idx].options.clone();
            let layout = self.resolve_overlay_layout(&options, 0, term_width, term_height);

            let mut overlay_lines = self.overlay_stack[idx].component.render(layout.width);

            let overlay_height = if let Some(max_h) = layout.max_height {
                overlay_lines.truncate(max_h);
                overlay_lines.len()
            } else {
                overlay_lines.len()
            };

            let layout =
                self.resolve_overlay_layout(&options, overlay_height, term_width, term_height);

            min_lines_needed = min_lines_needed.max(layout.row + overlay_lines.len());

            rendered.push(RenderedOverlay {
                overlay_lines,
                layout,
            });
        }

        let working_height = result.len().max(term_height).max(min_lines_needed);
        while result.len() < working_height {
            result.push(String::new());
        }

        let viewport_start = working_height.saturating_sub(term_height);

        for ro in &rendered {
            for (i, overlay_line) in ro.overlay_lines.iter().enumerate() {
                let idx = viewport_start + ro.layout.row + i;
                if idx < result.len() {
                    let truncated = if visible_width(overlay_line) > ro.layout.width {
                        slice_by_column(overlay_line, 0, ro.layout.width)
                    } else {
                        overlay_line.clone()
                    };
                    result[idx] = self.composite_line_at(
                        &result[idx],
                        &truncated,
                        ro.layout.col,
                        ro.layout.width,
                        term_width,
                    );
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

        let row = (row as isize + options.offset_y.unwrap_or(0)) as usize;
        let col = (col as isize + options.offset_x.unwrap_or(0)) as usize;

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
        for child in self.children.iter_mut() {
            let child_lines = child.render(width);
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

/// CachedContainer - a Container that caches its rendered output.
/// Used for components that need efficient re-rendering (e.g., chat area).
pub struct CachedContainer {
    inner: Container,
    cache: Option<RenderCache>,
    dirty: bool,
}

impl CachedContainer {
    pub fn new() -> Self {
        Self {
            inner: Container::new(),
            cache: None,
            dirty: true,
        }
    }

    pub fn inner(&self) -> &Container {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut Container {
        self.dirty = true;
        &mut self.inner
    }

    /// Add a child component.
    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.inner.add_child(component);
        self.dirty = true;
    }

    /// Remove a child component.
    pub fn remove_child(&mut self, component: &dyn Component) {
        self.inner.remove_child(component);
        self.dirty = true;
    }

    /// Clear all children.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.cache = None;
        self.dirty = true;
    }

    /// Mark as needing re-render.
    pub fn invalidate(&mut self) {
        self.dirty = true;
        self.cache = None;
        self.inner.invalidate_all();
    }

    /// Get children.
    pub fn children(&self) -> &[Box<dyn Component>] {
        self.inner.children()
    }

    /// Get mutable children.
    pub fn children_mut(&mut self) -> &mut [Box<dyn Component>] {
        self.dirty = true;
        self.inner.children_mut()
    }

    /// Get number of children.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for CachedContainer {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for CachedContainer {
    fn render(&mut self, width: usize) -> Vec<String> {
        // For now, just delegate to inner container.
        // Full caching requires &mut self which render() doesn't have.
        // This is handled at the TUI level with render_with_cache().
        self.inner.render(width)
    }

    fn handle_input(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        let result = self.inner.handle_input(key);
        if result {
            self.dirty = true;
        }
        result
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        self.cache = None;
        self.inner.invalidate();
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.inner.is_dirty()
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
        self.inner.clear_dirty();
    }
}
