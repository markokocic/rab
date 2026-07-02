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

    // ── Centralized focus management (matching pi's TUI) ──────────
    /// Currently focused target.
    focused: crate::tui::FocusTarget,
    /// Callback to set/unset focus on the editor component.
    /// The editor is behind Rc<RefCell<>> and can't be accessed through
    /// the Component trait's as_focusable(), so we use this callback.
    set_editor_focus: Option<Box<dyn FnMut(bool)>>,
}

impl TUI {
    pub fn new() -> Self {
        Self {
            root: Container::new(),
            screen: Screen::new(),
            width: 80,
            height: 24,
            dirty: true,
            focused: crate::tui::FocusTarget::None,
            set_editor_focus: None,
        }
    }

    // ── Focus management ──────────────────────────────────────────

    /// Register a callback to set/unset focus on the editor.
    /// Called during app initialization before showing any overlays.
    pub fn register_editor_focus(&mut self, callback: Box<dyn FnMut(bool)>) {
        self.set_editor_focus = Some(callback);
    }

    /// Set focus to a specific target. Handles clearing old focus and
    /// setting new focus through the appropriate channel.
    pub fn set_focus(&mut self, target: crate::tui::FocusTarget) {
        self.clear_focused();
        match target {
            crate::tui::FocusTarget::None => {}
            crate::tui::FocusTarget::Editor => {
                if let Some(ref mut cb) = self.set_editor_focus {
                    cb(true);
                }
            }
            crate::tui::FocusTarget::Overlay(id) => {
                self.root.set_overlay_focused(id, true);
            }
        }
        self.focused = target;
    }

    /// Clear the currently focused component's `focused` flag.
    fn clear_focused(&mut self) {
        match self.focused {
            crate::tui::FocusTarget::None => {}
            crate::tui::FocusTarget::Editor => {
                if let Some(ref mut cb) = self.set_editor_focus {
                    cb(false);
                }
            }
            crate::tui::FocusTarget::Overlay(id) => {
                self.root.set_overlay_focused(id, false);
            }
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

    // ── Overlay system (delegates to Container, manages focus) ────

    /// Show an overlay. Captures current focus as `pre_focus` in the overlay
    /// entry. If the overlay component is Focusable and the overlay is not
    /// `non_capturing`, gives focus to the overlay.
    pub fn show_overlay(&mut self, component: Box<dyn Component>, options: OverlayOptions) -> u64 {
        let non_capturing = options.non_capturing;
        let pre_focus = self.focused;
        let id = self.root.show_overlay(component, options, pre_focus);

        if !non_capturing {
            // Unfocus current component
            self.clear_focused();
            // Give focus to the overlay
            self.root.set_overlay_focused(id, true);
            self.focused = crate::tui::FocusTarget::Overlay(id);
        }

        self.dirty = true;
        id
    }

    /// Convenience: show an overlay anchored at top-left, full width.
    /// The pattern used by most agent UI overlays (model selector, auth dialogs, etc.).
    pub fn show_top_overlay(&mut self, component: Box<dyn Component>) -> u64 {
        use crate::tui::overlay::{OverlayAnchor, OverlayOptions, SizeValue};
        self.show_overlay(
            component,
            OverlayOptions {
                width: Some(SizeValue::Percent(100.0)),
                anchor: Some(OverlayAnchor::TopLeft),
                ..Default::default()
            },
        )
    }

    /// Hide an overlay by ID and restore the focus that was active before
    /// the overlay was shown.
    pub fn hide_overlay(&mut self, id: u64) {
        if self.focused == crate::tui::FocusTarget::Overlay(id) {
            self.clear_focused();
        }
        let pre_focus = self.root.hide_overlay(id);
        self.dirty = true;
        if let Some(target) = pre_focus {
            self.set_focus(target);
        }
    }

    /// Hide the topmost overlay and restore the focus that was active before
    /// the overlay was shown.
    pub fn pop_overlay(&mut self) {
        if let crate::tui::FocusTarget::Overlay(_id) = self.focused {
            self.clear_focused();
        }
        let pre_focus = self.root.pop_overlay();
        self.dirty = true;
        if let Some(target) = pre_focus {
            self.set_focus(target);
        }
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
        &mut self,
        row: usize,
        col: usize,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let total = self.screen.total_lines();
        if total == 0 {
            return Ok(());
        }
        let target_row = row.min(total.saturating_sub(1));
        let target_col = col.min(self.width.saturating_sub(1));

        // Relative row movement from the physical cursor position (pi-style).
        // This avoids absolute CSI H which assumes content starts at terminal row 0.
        let current_row = self.screen.hardware_cursor_row();
        let row_delta = target_row as i32 - current_row as i32;
        let mut buf = String::new();
        if row_delta > 0 {
            buf.push_str(&format!("\x1b[{}B", row_delta));
        } else if row_delta < 0 {
            buf.push_str(&format!("\x1b[{}A", -row_delta));
        }
        // Absolute column within the row
        buf.push_str(&format!("\x1b[{}G", target_col + 1));

        if !buf.is_empty() {
            write!(writer, "{}", buf)?;
            writer.flush()?;
        }

        // Update Screen tracking to match the new physical cursor position
        // (matching pi's hardwareCursorRow = targetRow in positionHardwareCursor).
        self.screen.set_hardware_cursor_row(target_row);

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
    fn test_overlay_layout_center_default() {
        // Layout resolution is now on Container - we test via overlay rendering
        let mut c = Container::new();
        c.set_term_height(24);
        let child = crate::tui::components::Text::new("test", 0, 0, None);
        c.show_overlay(
            Box::new(child),
            OverlayOptions::default(),
            crate::tui::FocusTarget::None,
        );
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
            crate::tui::FocusTarget::None,
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
            crate::tui::FocusTarget::None,
        );
        let lines = c.render(80);
        assert!(!lines.is_empty());
    }
}
