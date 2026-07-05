use std::io::{self, Write};

use crossterm::event::KeyEvent;

use crate::tui::Component;
use crate::tui::container::Container;
use crate::tui::overlay::{OverlayOptions, OverlayPosition};
use crate::tui::screen::Screen;
use crate::tui::terminal_colors::{TerminalColorScheme, parse_osc11_background_color};
use crate::tui::util::normalize_terminal_output;

/// Cursor marker constant (matches pi: APC pi:c ST)
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

/// Result from an input listener — matches pi's `InputListenerResult`.
/// Listeners are checked before normal input routing.
pub enum InputAction {
    /// The listener handled the key — stop processing.
    Consumed,
    /// Continue routing the original key through the component tree.
    Continue,
    /// Continue routing with a transformed key.
    Transform(KeyEvent),
}

/// Unique ID for a registered input listener.
pub type ListenerId = u64;

/// An input listener callback — receives a KeyEvent, returns an action.
type InputListenerFn = Box<dyn FnMut(&KeyEvent) -> InputAction>;

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
    /// Callback to control hardware cursor visibility outside of render.
    /// Called when overlays are shown/hidden to immediately hide/show cursor.
    set_cursor_visible: Option<Box<dyn FnMut(bool)>>,
    /// Registered input listeners (checked before normal routing).
    input_listeners: Vec<(ListenerId, InputListenerFn)>,
    /// Next unique ID for input listener registration.
    next_listener_id: ListenerId,
    /// Listeners for terminal color scheme changes (OSC 11 / DEC 2031).
    terminal_color_scheme_listeners: Vec<Box<dyn FnMut(TerminalColorScheme)>>,
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
            set_cursor_visible: None,
            input_listeners: Vec::new(),
            next_listener_id: 0,
            terminal_color_scheme_listeners: Vec::new(),
        }
    }

    // ── Focus management ──────────────────────────────────────────

    /// Register a callback to control hardware cursor visibility outside of render.
    /// Called immediately when overlays are shown/hidden, not waiting for the
    /// next render cycle. Matches pi's `terminal.hideCursor()` in overlay lifecycle.
    pub fn register_cursor_callback(&mut self, callback: Box<dyn FnMut(bool)>) {
        self.set_cursor_visible = Some(callback);
    }

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

    // ── Child management (pi-style: TUI extends Container) ──────

    /// Add a child component to the root container.
    /// Matches pi's `tui.addChild(component)`.
    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.root.add_child(component);
    }

    /// Remove a child component from the root container.
    pub fn remove_child(&mut self, component: &dyn Component) {
        self.root.remove_child(component);
    }

    /// Remove all children from the root container.
    pub fn clear(&mut self) {
        self.root.clear();
    }

    /// Get a reference to all children.
    pub fn children(&self) -> &[Box<dyn Component>] {
        self.root.children()
    }

    /// Get a mutable reference to all children.
    pub fn children_mut(&mut self) -> &mut [Box<dyn Component>] {
        self.root.children_mut()
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

        // Hide cursor immediately when overlay is shown (pi-style)
        if let Some(ref mut cb) = self.set_cursor_visible {
            cb(false);
        }

        self.dirty = true;
        id
    }

    /// Position for full-width overlays — top (row 0) or bottom
    /// (just above the footer/editor area).
    pub fn show_positioned_overlay(
        &mut self,
        component: Box<dyn Component>,
        position: OverlayPosition,
    ) -> u64 {
        use crate::tui::overlay::{OverlayAnchor, OverlayOptions, SizeValue};
        let anchor = match position {
            OverlayPosition::Top => OverlayAnchor::TopLeft,
            OverlayPosition::Bottom => OverlayAnchor::BottomLeft,
        };
        let offset_y = match position {
            OverlayPosition::Top => None,
            OverlayPosition::Bottom => Some(-2), // just above the footer (2 rows)
        };
        self.show_overlay(
            component,
            OverlayOptions {
                width: Some(SizeValue::Percent(100.0)),
                anchor: Some(anchor),
                offset_y,
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
        // Show cursor when overlay stack is empty (pi-style)
        if !self.has_overlays()
            && let Some(ref mut cb) = self.set_cursor_visible
        {
            cb(true);
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
        // Show cursor when overlay stack is empty (pi-style)
        if !self.has_overlays()
            && let Some(ref mut cb) = self.set_cursor_visible
        {
            cb(true);
        }
    }

    pub fn has_overlays(&self) -> bool {
        self.root.has_overlays()
    }

    // ── Terminal color scheme detection (pi-style) ───────────────

    /// Register a listener for terminal color scheme changes.
    /// Listeners are called when an OSC 11 response or DEC 2031 report
    /// is detected in the input stream.
    pub fn on_terminal_color_scheme_change(
        &mut self,
        listener: Box<dyn FnMut(TerminalColorScheme)>,
    ) {
        self.terminal_color_scheme_listeners.push(listener);
    }

    /// Feed raw terminal data for OSC response parsing.
    /// Returns true if the data was consumed as an OSC response.
    /// Call this from the input pipeline when raw data is available.
    pub fn feed_raw_terminal_data(&mut self, data: &str) -> bool {
        // Check for OSC 11 background color response
        if crate::tui::terminal_colors::is_osc11_background_color_response(data) {
            if let Some(color) = parse_osc11_background_color(data) {
                let scheme = crate::tui::terminal_colors::color_scheme_from_background(&color);
                for listener in &mut self.terminal_color_scheme_listeners {
                    listener(scheme);
                }
            }
            return true;
        }
        // Check for DEC 2031 color scheme report
        if let Some(scheme) = crate::tui::terminal_colors::parse_terminal_color_scheme_report(data)
        {
            for listener in &mut self.terminal_color_scheme_listeners {
                listener(scheme);
            }
            return true;
        }
        false
    }

    // ── Input routing ──────────────────────────────────────────────

    /// Register an input listener. The listener is called for every key event
    /// before normal routing. It can consume, transform, or pass through the key.
    /// Returns a `ListenerId` for later removal.
    /// Matches pi's `TUI.addInputListener()`.
    pub fn add_input_listener(&mut self, listener: InputListenerFn) -> ListenerId {
        let id = self.next_listener_id;
        self.next_listener_id += 1;
        self.input_listeners.push((id, listener));
        id
    }

    /// Remove a previously registered input listener by ID.
    pub fn remove_input_listener(&mut self, id: ListenerId) {
        self.input_listeners.retain(|(i, _)| *i != id);
    }

    /// Route a keyboard event through input listeners, then the overlay
    /// input pipeline. Should be called BEFORE the application handles
    /// the key itself.
    pub fn route_input(&mut self, key: &KeyEvent) -> bool {
        // Check input listeners first (pi-style)
        let mut current = *key;
        for (_, listener) in &mut self.input_listeners {
            match listener(&current) {
                InputAction::Consumed => return true,
                InputAction::Continue => {}
                InputAction::Transform(k) => current = k,
            }
        }

        // Route through component tree (overlays first, then children)
        self.root.handle_input(&current)
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
    ///
    /// Hides the hardware cursor during the diff render to prevent flickering
    /// (cursor movements as lines are cleared and re-written), then shows and
    /// positions it at the cursor marker location afterwards.
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

        // Pad to terminal height so the footer (last child) is always at the
        // bottom of the terminal viewport, even when chat content is short.
        // Overlays handle their own padding via composite_overlays, so this
        // only applies when no overlays are active.
        if !self.has_overlays() && lines.len() < height {
            lines.resize(height, String::new());
        }

        // Normalize terminal output
        for line in lines.iter_mut() {
            *line = normalize_terminal_output(line);
        }

        // Hide cursor before diff render to avoid flicker from cursor movement
        // during line clears and rewrites. The cursor will be shown at the
        // correct position after positioning.
        write!(writer, "\x1b[?25l")?;

        // Diff render via Screen (extracts cursor markers internally)
        let cursor_pos = self
            .screen
            .render(lines.clone(), width as u16, height as u16, writer)?;

        if let Some((row, col)) = cursor_pos {
            // Position hardware cursor at the marker location (needed for IME)
            self.position_hard_cursor(row, col, writer)?;
            // Keep cursor hidden — the visual block cursor is already rendered
            // by the editor via ANSI inversion. Showing both creates a double
            // cursor (block + blinking underscore).
        } else if self.has_overlays() {
            // Keep cursor hidden when overlays are active (pi-style)
            // (already hidden above)
        } else {
            // No marker found — show cursor at current position anyway
            write!(writer, "\x1b[?25h")?;
        }
        writer.flush()?;

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

// ── Component implementation (delegates to root Container) ─────────
// Pi-style: TUI extends Container, so it implements Component by
// delegating to its internal root Container.

impl Component for TUI {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.root.render(width)
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        self.root.handle_input(key)
    }

    fn invalidate(&mut self) {
        self.root.invalidate();
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
