//! ConfirmOverlay — generic confirmation dialog (matches pi's confirm dialog pattern).
//!
//! Shows a titled message with [Y]es / [N]o / Enter choices.
//! Communicates the result via a callback.

use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::keybindings::{ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM, get_keybindings};
use crossterm::event::KeyEvent;

/// Confirmation overlay with yes/no buttons.
pub struct ConfirmOverlay {
    title: String,
    message: String,
    on_confirm: Option<Box<dyn FnOnce()>>,
    on_cancel: Option<Box<dyn FnOnce()>>,
    selected: bool, // true = Yes selected, false = No selected
    done: bool,
}

impl ConfirmOverlay {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            on_confirm: None,
            on_cancel: None,
            selected: true, // Default to Yes
            done: false,
        }
    }

    /// Set the confirmation callback.
    pub fn on_confirm<F>(&mut self, f: F)
    where
        F: FnOnce() + 'static,
    {
        self.on_confirm = Some(Box::new(f));
    }

    /// Set the cancel callback.
    pub fn on_cancel<F>(&mut self, f: F)
    where
        F: FnOnce() + 'static,
    {
        self.on_cancel = Some(Box::new(f));
    }
}

impl Component for ConfirmOverlay {
    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let mut lines: Vec<String> = Vec::new();

        // Top border
        lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));
        lines.push(String::new());

        // Title
        lines.push(format!("  {}", theme.bold(&theme.accent(&self.title))));
        lines.push(String::new());

        // Message (word-wrapped to fit width)
        let max_text_width = width.saturating_sub(4); // 2 spaces padding each side
        if max_text_width > 10 {
            let mut remaining = self.message.as_str();
            while !remaining.is_empty() {
                let break_at = if remaining.len() <= max_text_width {
                    remaining.len()
                } else {
                    // Try to break at a space
                    let slice = &remaining[..max_text_width];
                    let last_space = slice.rfind(' ').unwrap_or(max_text_width);
                    if last_space == 0 {
                        max_text_width
                    } else {
                        last_space
                    }
                };
                lines.push(format!("  {}", &remaining[..break_at]));
                remaining = remaining[break_at..].trim_start();
            }
        } else {
            lines.push(format!("  {}", self.message));
        }
        lines.push(String::new());

        // Yes/No buttons
        let yes_style = if self.selected {
            theme.bold(&theme.fg("success", "[Y] Yes"))
        } else {
            theme.dim("[Y] Yes")
        };
        let no_style = if !self.selected {
            theme.bold(&theme.fg("error", "[N] No"))
        } else {
            theme.dim("[N] No")
        };

        lines.push(format!("  {}    {}", yes_style, no_style));
        lines.push(String::new());

        // Key hints
        lines.push(format!(
            "  {}",
            theme.dim("Tab/← →: switch · Enter: confirm · Esc: cancel")
        ));

        lines.push(String::new());
        // Bottom border
        lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        if self.done {
            return false;
        }

        let kb = get_keybindings();

        // Escape cancels
        if kb.matches(key, ACTION_SELECT_CANCEL) {
            self.done = true;
            if let Some(cb) = self.on_cancel.take() {
                cb();
            }
            return false;
        }

        // Enter or 'y' confirms (when Yes selected)
        if kb.matches(key, ACTION_SELECT_CONFIRM)
            || key.code == crossterm::event::KeyCode::Char('y')
        {
            self.done = true;
            if let Some(cb) = self.on_confirm.take() {
                cb();
            }
            return false;
        }

        // 'n' cancels
        if key.code == crossterm::event::KeyCode::Char('n') {
            self.done = true;
            if let Some(cb) = self.on_cancel.take() {
                cb();
            }
            return false;
        }

        // Tab / Right / Left toggles selection
        if key.code == crossterm::event::KeyCode::Tab
            || key.code == crossterm::event::KeyCode::Right
            || key.code == crossterm::event::KeyCode::Left
        {
            self.selected = !self.selected;
            return true;
        }

        false
    }
}
