use std::io::{self, Write};

use crossterm::event::KeyEvent;

use crate::tui::Component;
use crate::tui::container::Container;
use crate::tui::overlay::OverlayOptions;
use crate::tui::screen::Screen;
use crate::tui::util::normalize_terminal_output;

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
    /// Overlays are also managed through Container's overlay stack.
    /// Matches pi's `class TUI extends Container`.
    pub root: Container,

    /// The diff renderer
    screen: Screen,
    /// Terminal dimensions (cached)
    width: usize,
    height: usize,
    /// Whether content changed since last render
    dirty: bool,
}

impl TUI {
    pub fn new() -> Self {
        Self {
            root: Container::new(),
            screen: Screen::new(),
            width: 80,
            height: 24,
            dirty: true,
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
        self.root.set_term_height(height);
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

    // ── Overlay system (delegates to Container) ───────────────────

    pub fn show_overlay(&mut self, component: Box<dyn Component>, options: OverlayOptions) -> u64 {
        let id = self.root.show_overlay(component, options);
        self.dirty = true;
        id
    }

    pub fn hide_overlay(&mut self, id: u64) {
        self.root.hide_overlay(id);
        self.dirty = true;
    }

    pub fn pop_overlay(&mut self) {
        self.root.pop_overlay();
        self.dirty = true;
    }

    pub fn has_overlays(&self) -> bool {
        self.root.has_overlays()
    }

    // ── Input routing ──────────────────────────────────────────────

    /// Route a keyboard event through the overlay input pipeline.
    /// Should be called BEFORE the application handles the key itself,
    /// so overlays get first crack at input.
    pub fn route_input(&mut self, key: &KeyEvent) -> bool {
        self.root.handle_input(key)
    }

    /// Route a paste event to overlays or root.
    pub fn route_paste(&mut self, text: &str) -> bool {
        // Try overlays first
        for entry in self.root.overlay_stack_mut().iter_mut().rev() {
            if !entry.hidden {
                entry.component.handle_paste(text);
                return true;
            }
        }
        false
    }

    // ── Rendering ──────────────────────────────────────────────────

    /// Render the root component tree (including composited overlays),
    /// then diff-render to screen via Screen.
    pub fn render(
        &mut self,
        width: usize,
        height: usize,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        self.width = width;
        self.height = height;
        self.root.set_term_height(height);

        // Render root container (includes overlay compositing internally)
        let mut lines = self.root.render(width);

        // Normalize terminal output
        for line in lines.iter_mut() {
            *line = normalize_terminal_output(line);
        }

        // Diff render via Screen (extracts cursor markers internally)
        let cursor_pos = self
            .screen
            .render(lines.clone(), width as u16, height as u16, writer)?;

        // Position hardware cursor if marker was found
        if let Some((row, col)) = cursor_pos {
            self.position_hard_cursor(row, col, writer)?;
        }

        self.dirty = false;
        Ok(())
    }

    /// Move cursor to clean position on exit — past all content
    pub fn finalize(&mut self, writer: &mut dyn Write) -> io::Result<()> {
        self.screen.finalize(writer)
    }

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
    use crate::tui::overlay::{OverlayAnchor, OverlayOptions, SizeValue};

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
        use crate::tui::screen::Screen;
        let screen = Screen::new();
        let mut lines = vec![
            "line 1".to_string(),
            format!("before{}after", CURSOR_MARKER),
            "line 3".to_string(),
        ];
        let pos = screen.extract_cursor_marker(&mut lines, 10);
        assert!(pos.is_some());
        let (row, col) = pos.unwrap();
        assert_eq!(row, 1);
        assert_eq!(col, 6); // visible_width("before") = 6
        assert_eq!(lines[1], "beforeafter");
        assert!(!lines[1].contains(CURSOR_MARKER));
    }

    #[test]
    fn test_cursor_marker_outside_viewport() {
        use crate::tui::screen::Screen;
        let screen = Screen::new();
        // Marker on line 0 but viewport is last 2 lines of 5
        let mut lines = vec![
            format!("{}marker", CURSOR_MARKER),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ];
        let pos = screen.extract_cursor_marker(&mut lines, 2);
        assert!(pos.is_none()); // line 0 is not in last 2 of 5
    }

    #[test]
    fn test_composite_line_at_basic() {
        let mut c = Container::new();
        // composite_line_at is private on Container; test through public API
        // by rendering an overlay that uses it internally
        let child = crate::tui::components::Text::new("overlay", 0, 0, None);
        c.show_overlay(
            Box::new(child),
            OverlayOptions {
                width: Some(SizeValue::Absolute(2)),
                anchor: Some(OverlayAnchor::TopLeft),
                ..Default::default()
            },
        );
        // Smoke test: rendering with overlay doesn't panic
        let _lines = c.render(80);
    }

    #[test]
    fn test_overlay_layout_center_default() {
        // Layout resolution is now on Container - we test via overlay rendering
        let mut c = Container::new();
        c.set_term_height(24);
        let child = crate::tui::components::Text::new("test", 0, 0, None);
        c.show_overlay(Box::new(child), OverlayOptions::default());
        let lines = c.render(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_overlay_layout_percent_width() {
        let mut c = Container::new();
        c.set_term_height(24);
        let child = crate::tui::components::Text::new("x", 0, 0, None);
        c.show_overlay(
            Box::new(child),
            OverlayOptions {
                width: Some(SizeValue::Percent(50.0)),
                ..Default::default()
            },
        );
        let lines = c.render(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_overlay_layout_margin() {
        let mut c = Container::new();
        c.set_term_height(24);
        let child = crate::tui::components::Text::new("test", 0, 0, None);
        c.show_overlay(
            Box::new(child),
            OverlayOptions {
                margin: Some(crate::tui::overlay::OverlayMargin {
                    top: 2,
                    right: 2,
                    bottom: 2,
                    left: 2,
                }),
                anchor: Some(OverlayAnchor::TopLeft),
                ..Default::default()
            },
        );
        let lines = c.render(80);
        assert!(!lines.is_empty());
    }
}
