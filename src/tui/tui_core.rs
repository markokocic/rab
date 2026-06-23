use std::io::{self, Write};

use crossterm::event::KeyEvent;

use crate::tui::Component;
use crate::tui::container::Container;
use crate::tui::overlay::{OverlayAnchor, OverlayEntry, OverlayLayout, OverlayOptions, SizeValue};
use crate::tui::screen::Screen;
use crate::tui::util::{
    extract_segments, normalize_terminal_output, slice_by_column, visible_width,
};

/// Marker appended to lines after extraction — matches pi's SEGMENT_RESET
const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

/// Cursor marker constant (matches pi: APC pi:c ST)
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

// =============================================================================
// TUI — Main class for managing terminal UI with differential rendering
// and overlay compositing. Wraps Screen and adds overlay stack, focus
// management, and input pipeline.
//
// Pi reference: packages/tui/src/tui.ts
// =============================================================================

pub struct TUI {
    /// The root container — all top-level children are added here.
    /// Matches pi's `class TUI extends Container` — the TUI itself renders
    /// this root container first, then applies overlays.
    pub root: Container,

    /// The diff renderer
    screen: Screen,
    /// Terminal dimensions (cached)
    width: usize,
    height: usize,
    /// Whether content changed since last render
    dirty: bool,

    // Overlay stack
    overlay_stack: Vec<OverlayEntry>,
    next_overlay_id: u64,
    focus_order_counter: u64,

    // Focus state
    /// Currently focused component index (within overlay stack, or None for base)
    focused_component: Option<usize>,
}

impl TUI {
    pub fn new() -> Self {
        Self {
            root: Container::new(),
            screen: Screen::new(),
            width: 80,
            height: 24,
            dirty: true,
            overlay_stack: Vec::new(),
            next_overlay_id: 0,
            focus_order_counter: 0,
            focused_component: None,
        }
    }

    // ── Screen delegation ─────────────────────────────────────────

    pub fn screen_mut(&mut self) -> &mut Screen {
        &mut self.screen
    }

    pub fn full_redraw_count(&self) -> usize {
        self.screen.full_redraw_count()
    }

    pub fn set_clear_on_shrink(&mut self, enabled: bool) {
        self.screen.set_clear_on_shrink(enabled);
    }

    pub fn set_dimensions(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
    }

    pub fn get_dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub fn request_render(&mut self) {
        self.dirty = true;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    // ── Overlay system ─────────────────────────────────────────────

    /// Show an overlay component with configurable positioning and sizing.
    /// Returns an overlay ID that can be used with `hide_overlay`.
    pub fn show_overlay(&mut self, component: Box<dyn Component>, options: OverlayOptions) -> u64 {
        let id = self.next_overlay_id;
        self.next_overlay_id += 1;

        let is_capturing = !options.non_capturing;

        let entry = OverlayEntry {
            component,
            options,
            pre_focus: self.focused_component,
            hidden: false,
            focus_order: self.focus_order_counter,
            id,
        };
        self.focus_order_counter += 1;
        self.overlay_stack.push(entry);

        // Focus the overlay if it's capturing
        if is_capturing {
            let idx = self.overlay_stack.len() - 1;
            self.focused_component = Some(idx);
        }

        self.dirty = true;
        id
    }

    /// Hide an overlay by ID
    pub fn hide_overlay(&mut self, id: u64) {
        let pos = self.overlay_stack.iter().position(|e| e.id == id);
        if let Some(idx) = pos {
            let entry = self.overlay_stack.remove(idx);

            // If this overlay had focus, restore to previous focus
            if self.focused_component == Some(idx) {
                let restored = self.topmost_visible_overlay();
                self.focused_component = restored.or(entry.pre_focus);
            } else if let Some(focused) = self.focused_component {
                // Adjust focus index if removal shifted it
                if focused > idx {
                    self.focused_component = Some(focused - 1);
                }
            }

            self.dirty = true;
        }
    }

    /// Hide the topmost overlay and restore previous focus
    pub fn pop_overlay(&mut self) {
        if let Some(entry) = self.overlay_stack.pop() {
            if self.focused_component == Some(self.overlay_stack.len()) {
                let restored = self.topmost_visible_overlay();
                self.focused_component = restored.or(entry.pre_focus);
            }
            self.dirty = true;
        }
    }

    /// Check if there are any visible overlays
    pub fn has_overlays(&self) -> bool {
        self.overlay_stack.iter().any(|e| !e.hidden)
    }

    /// Get the topmost visible capturing overlay index
    fn topmost_visible_overlay(&self) -> Option<usize> {
        self.overlay_stack
            .iter()
            .enumerate()
            .rev()
            .find(|(_, e)| !e.hidden && !e.options.non_capturing)
            .map(|(i, _)| i)
    }

    // ── Focus management ───────────────────────────────────────────

    /// Set focus to a specific overlay or None for base content
    pub fn set_focus(&mut self, overlay_idx: Option<usize>) {
        self.focused_component = overlay_idx;
    }

    /// Get current focus target
    pub fn focused_overlay(&self) -> Option<usize> {
        self.focused_component
    }

    // ── Input routing ──────────────────────────────────────────────

    /// Route a keyboard event through the overlay input pipeline.
    /// Returns true if the input was consumed by an overlay.
    ///
    /// Should be called BEFORE the application handles the key itself,
    /// so overlays get first crack at input.
    pub fn route_input(&mut self, key: &KeyEvent) -> bool {
        // If an overlay is focused, route input to it first
        if let Some(idx) = self.focused_component
            && let Some(entry) = self.overlay_stack.get_mut(idx)
            && !entry.hidden
            && entry.component.handle_input(key)
        {
            return true;
        }

        // Route to all visible non-capturing overlays in reverse order
        for entry in self.overlay_stack.iter_mut().rev() {
            if !entry.hidden && entry.options.non_capturing && entry.component.handle_input(key) {
                return true;
            }
        }

        false
    }

    /// Route a paste event to the focused overlay component or root.
    /// Matches pi's input pipeline where paste is sent to handleInput.
    pub fn route_paste(&mut self, text: &str) -> bool {
        if let Some(idx) = self.focused_component
            && let Some(entry) = self.overlay_stack.get_mut(idx)
            && !entry.hidden
        {
            entry.component.handle_paste(text);
            return true;
        }
        false
    }

    // ── Rendering ──────────────────────────────────────────────────

    /// Render the root component tree, composite overlays, then diff-render to screen.
    ///
    /// Matches pi's TUI.render() which extends Container:
    /// 1. Render root Container (all permanent children)
    /// 2. Append chat_buffer (bridge from compose_ui during migration)
    /// 3. Composite any overlays on top
    /// 4. Diff-render via Screen
    pub fn render(
        &mut self,
        width: usize,
        height: usize,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        self.width = width;
        self.height = height;

        // 1. Render root container (all sections in correct order).
        //    Root children: header_section → chat_container → pending_section →
        //    status_section → queued_section → working_section → editor → footer
        let mut lines = self.root.render(width);

        // 3. Composite overlays into the rendered lines
        if !self.overlay_stack.is_empty() {
            lines = self.composite_overlays(&lines, width, height);
        }

        // 3. Extract cursor marker and strip it from lines
        let cursor_pos = self.extract_cursor_position(&mut lines, height);

        // 4. Apply segment reset (normalize terminal output)
        for line in lines.iter_mut() {
            *line = normalize_terminal_output(line);
        }

        // 5. Delegate to Screen for diff rendering
        self.screen
            .render(lines.clone(), width as u16, height as u16, writer)?;

        // 6. Position hardware cursor if marker was found
        if let Some((row, col)) = cursor_pos {
            self.position_hard_cursor(row, col, writer)?;
            // Sync Screen's cursor tracking with actual hardware cursor position
            self.screen.set_hardware_cursor_row(row);
        }

        self.dirty = false;
        Ok(())
    }

    /// Move cursor to clean position on exit — past all content
    pub fn finalize(&mut self, writer: &mut dyn Write) -> io::Result<()> {
        self.screen.finalize(writer)
    }

    // ── Private helpers ────────────────────────────────────────────

    /// Composite all visible overlays into the content lines.
    ///
    /// Each overlay is pre-rendered at the width determined by its options.
    /// Lines are then composited at the calculated row/col position over the
    /// base content. Overlays with higher focus_order appear on top.
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

        // Pre-render each overlay and calculate layout
        struct RenderedOverlay {
            overlay_lines: Vec<String>,
            layout: OverlayLayout,
        }

        let mut rendered: Vec<RenderedOverlay> = Vec::new();
        for &idx in &indices {
            // Resolve layout with height=0 first (options accessed immutably)
            let options = self.overlay_stack[idx].options.clone();
            let layout = self.resolve_overlay_layout(&options, 0, term_width, term_height);

            // Render component at calculated width (mutable access)
            let mut overlay_lines = self.overlay_stack[idx].component.render(layout.width);

            // Apply max_height
            let overlay_height = if let Some(max_h) = layout.max_height {
                overlay_lines.truncate(max_h);
                overlay_lines.len()
            } else {
                overlay_lines.len()
            };

            // Re-resolve with actual height
            let layout =
                self.resolve_overlay_layout(&options, overlay_height, term_width, term_height);

            min_lines_needed = min_lines_needed.max(layout.row + overlay_lines.len());

            rendered.push(RenderedOverlay {
                overlay_lines,
                layout,
            });
        }

        // Ensure result has enough lines
        let working_height = result.len().max(term_height).max(min_lines_needed);
        while result.len() < working_height {
            result.push(String::new());
        }

        let viewport_start = working_height.saturating_sub(term_height);

        // Composite each overlay
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
    /// Single-pass optimized — matches pi's compositeLineAt.
    fn composite_line_at(
        &self,
        base_line: &str,
        overlay_line: &str,
        start_col: usize,
        overlay_width: usize,
        total_width: usize,
    ) -> String {
        let after_start = start_col + overlay_width;

        // Extract before and after segments from base line
        let (before, before_width, after, after_width) = extract_segments(
            base_line,
            start_col,
            after_start,
            total_width.saturating_sub(after_start),
            true,
        );

        // Slice overlay to declared width (strict to exclude wide chars at boundary)
        let overlay = slice_by_column(overlay_line, 0, overlay_width);
        let overlay_vis = visible_width(&overlay);

        // Pad segments to target widths
        let before_pad = start_col.saturating_sub(before_width);
        let overlay_pad = overlay_width.saturating_sub(overlay_vis);
        let actual_before_width = before_width.max(start_col);
        let actual_overlay_width = overlay_vis.max(overlay_width);
        let after_target = total_width.saturating_sub(actual_before_width + actual_overlay_width);
        let after_pad = after_target.saturating_sub(after_width);

        // Compose result with segment resets
        let mut result = String::new();
        result.push_str(&before);
        result.push_str(&" ".repeat(before_pad));
        result.push_str(SEGMENT_RESET);
        result.push_str(&overlay);
        result.push_str(&" ".repeat(overlay_pad));
        result.push_str(SEGMENT_RESET);
        result.push_str(&after);
        result.push_str(&" ".repeat(after_pad));

        // Safety truncation
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
        // Parse margin
        let margin = options.margin.unwrap_or_default();
        let margin_top = margin.top;
        let margin_right = margin.right;
        let margin_bottom = margin.bottom;
        let margin_left = margin.left;

        let avail_width = (term_width - margin_left - margin_right).max(1);
        let avail_height = (term_height - margin_top - margin_bottom).max(1);

        // Resolve width
        let width = options
            .width
            .map(|sv| sv.resolve(term_width))
            .unwrap_or_else(|| 80.min(avail_width));
        let width = options.min_width.map(|mw| width.max(mw)).unwrap_or(width);
        let width = width.max(1).min(avail_width);

        // Resolve max_height
        let max_height = options.max_height.map(|sv| sv.resolve(term_height));
        let max_height = max_height.map(|mh| mh.max(1).min(avail_height));

        // Effective overlay height
        let effective_height = match max_height {
            Some(mh) => overlay_height.min(mh),
            None => overlay_height,
        };

        // Resolve position
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
            self.resolve_anchor_row(anchor, effective_height, avail_height, margin_top)
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
            self.resolve_anchor_col(anchor, width, avail_width, margin_left)
        };

        // Apply offsets
        let row = (row as isize + options.offset_y.unwrap_or(0)) as usize;
        let col = (col as isize + options.offset_x.unwrap_or(0)) as usize;

        // Clamp to terminal bounds
        let row = row
            .max(margin_top)
            .min(term_height - margin_bottom - effective_height);
        let col = col.max(margin_left).min(term_width - margin_right - width);

        OverlayLayout {
            width,
            row,
            col,
            max_height,
        }
    }

    fn resolve_anchor_row(
        &self,
        anchor: OverlayAnchor,
        height: usize,
        avail_height: usize,
        margin_top: usize,
    ) -> usize {
        match anchor {
            OverlayAnchor::TopLeft | OverlayAnchor::TopCenter | OverlayAnchor::TopRight => {
                margin_top
            }
            OverlayAnchor::BottomLeft
            | OverlayAnchor::BottomCenter
            | OverlayAnchor::BottomRight => margin_top + avail_height - height,
            OverlayAnchor::LeftCenter | OverlayAnchor::Center | OverlayAnchor::RightCenter => {
                margin_top + (avail_height - height) / 2
            }
        }
    }

    fn resolve_anchor_col(
        &self,
        anchor: OverlayAnchor,
        width: usize,
        avail_width: usize,
        margin_left: usize,
    ) -> usize {
        match anchor {
            OverlayAnchor::TopLeft | OverlayAnchor::LeftCenter | OverlayAnchor::BottomLeft => {
                margin_left
            }
            OverlayAnchor::TopRight | OverlayAnchor::RightCenter | OverlayAnchor::BottomRight => {
                margin_left + avail_width - width
            }
            OverlayAnchor::TopCenter | OverlayAnchor::Center | OverlayAnchor::BottomCenter => {
                margin_left + (avail_width - width) / 2
            }
        }
    }

    /// Find and extract cursor position from rendered lines.
    /// Searches for CURSOR_MARKER, calculates its position, and strips it.
    /// Only scans the bottom `height` lines (visible viewport).
    fn extract_cursor_position(
        &self,
        lines: &mut [String],
        height: usize,
    ) -> Option<(usize, usize)> {
        let viewport_top = lines.len().saturating_sub(height);
        for row in (viewport_top..lines.len()).rev() {
            let line = &lines[row];
            if let Some(marker_idx) = line.find(CURSOR_MARKER) {
                let col = visible_width(&line[..marker_idx]);
                // Strip marker
                let before = &line[..marker_idx];
                let after = &line[marker_idx + CURSOR_MARKER.len()..];
                lines[row] = format!("{}{}", before, after);
                return Some((row, col));
            }
        }
        None
    }

    /// Position hardware cursor at the given row/col (relative to viewport).
    fn position_hard_cursor(
        &self,
        row: usize,
        col: usize,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        // Calculate viewport position
        let viewport_top = self.screen.prev_viewport_top();
        if row < viewport_top {
            return Ok(());
        }
        let screen_row = row - viewport_top;
        if screen_row >= self.height {
            return Ok(());
        }
        let screen_col = col.min(self.width - 1);

        // Move cursor: CSI <row> ; <col> H (1-based)
        write!(writer, "\x1b[{};{}H", screen_row + 1, screen_col + 1)?;
        writer.flush()?;
        Ok(())
    }
}

impl Default for TUI {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::{OverlayMargin, OverlayOptions};

    struct TestComponent {
        text: String,
    }

    impl Component for TestComponent {
        fn render(&mut self, _width: usize) -> Vec<String> {
            vec![self.text.clone()]
        }

        fn handle_input(&mut self, _key: &crossterm::event::KeyEvent) -> bool {
            false
        }

        fn invalidate(&mut self) {}
    }

    #[test]
    fn test_tui_new() {
        let tui = TUI::new();
        assert!(!tui.has_overlays());
        assert_eq!(tui.full_redraw_count(), 0);
    }

    #[test]
    fn test_show_and_hide_overlay() {
        let mut tui = TUI::new();
        let id = tui.show_overlay(
            Box::new(TestComponent {
                text: "overlay".into(),
            }),
            OverlayOptions::default(),
        );
        assert!(tui.has_overlays());
        tui.hide_overlay(id);
        assert!(!tui.has_overlays());
    }

    #[test]
    fn test_pop_overlay() {
        let mut tui = TUI::new();
        tui.show_overlay(
            Box::new(TestComponent { text: "a".into() }),
            OverlayOptions::default(),
        );
        tui.show_overlay(
            Box::new(TestComponent { text: "b".into() }),
            OverlayOptions::default(),
        );
        assert!(tui.has_overlays());
        tui.pop_overlay();
        assert!(tui.has_overlays()); // still has "a"
        tui.pop_overlay();
        assert!(!tui.has_overlays());
    }

    #[test]
    fn test_cursor_marker_extraction() {
        let tui = TUI::new();
        let mut lines = vec![
            "line 1".to_string(),
            format!("before{}after", CURSOR_MARKER),
            "line 3".to_string(),
        ];
        let pos = tui.extract_cursor_position(&mut lines, 10);
        assert!(pos.is_some());
        let (row, col) = pos.unwrap();
        assert_eq!(row, 1);
        assert_eq!(col, 6); // visible_width("before") = 6
        assert_eq!(lines[1], "beforeafter");
        assert!(!lines[1].contains(CURSOR_MARKER));
    }

    #[test]
    fn test_cursor_marker_outside_viewport() {
        let tui = TUI::new();
        // Marker on line 0 but viewport is last 2 lines of 5
        let mut lines = vec![
            format!("{}marker", CURSOR_MARKER),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ];
        let pos = tui.extract_cursor_position(&mut lines, 2);
        assert!(pos.is_none()); // line 0 is not in last 2 of 5
    }

    #[test]
    fn test_composite_line_at_basic() {
        let tui = TUI::new();
        let result = tui.composite_line_at("hello world", "!!", 6, 2, 13);
        assert_eq!(visible_width(&result), 13);
        assert!(result.contains("!!"));
    }

    #[test]
    fn test_composite_line_at_no_overflow() {
        let tui = TUI::new();
        let result = tui.composite_line_at("abcdefghij", "12345", 2, 5, 12);
        assert_eq!(visible_width(&result), 12);
    }

    #[test]
    fn test_overlay_layout_center_default() {
        let tui = TUI::new();
        let layout = tui.resolve_overlay_layout(&OverlayOptions::default(), 5, 80, 24);
        // Default: centered, width=min(80,80)=80, row=center
        assert_eq!(layout.width, 80);
        // avail_height = 24, height=5 → center = (24-5)/2 = 9
        assert_eq!(layout.row, 9);
        assert_eq!(layout.col, 0);
        assert!(layout.max_height.is_none());
    }

    #[test]
    fn test_overlay_layout_percent_width() {
        let tui = TUI::new();
        let opts = OverlayOptions {
            width: Some(SizeValue::Percent(50.0)),
            ..Default::default()
        };
        let layout = tui.resolve_overlay_layout(&opts, 5, 80, 24);
        assert_eq!(layout.width, 40); // 50% of 80
    }

    #[test]
    fn test_overlay_layout_margin() {
        let tui = TUI::new();
        let opts = OverlayOptions {
            margin: Some(OverlayMargin {
                top: 2,
                right: 2,
                bottom: 2,
                left: 2,
            }),
            anchor: Some(OverlayAnchor::TopLeft),
            ..Default::default()
        };
        let layout = tui.resolve_overlay_layout(&opts, 5, 80, 24);
        assert_eq!(layout.row, 2);
        assert_eq!(layout.col, 2);
    }
}
