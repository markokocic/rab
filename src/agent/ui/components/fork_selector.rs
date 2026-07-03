//! User message selector for the `/fork` command.
//!
//! Shows a list of user messages the user can pick from to fork the session.
//! Matching pi's `UserMessageSelectorComponent`.

use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::keybindings::{
    ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM, ACTION_SELECT_DOWN, ACTION_SELECT_UP,
    get_keybindings,
};
use crate::tui::util::truncate_to_width;
use crossterm::event::KeyEvent;

/// A user message item shown in the fork selector.
pub struct UserMessageItem {
    pub id: String,
    pub text: String,
    pub index: usize,
    pub total: usize,
}

/// Selector overlay for choosing a user message to fork from.
pub struct ForkSelector {
    messages: Vec<UserMessageItem>,
    selected_index: usize,
    /// Called when user selects a message (carries entry ID).
    pub on_select: Option<Box<dyn FnOnce(String)>>,
    /// Called when user cancels.
    pub on_cancel: Option<Box<dyn FnOnce()>>,
}

impl ForkSelector {
    pub fn new(messages: Vec<UserMessageItem>) -> Self {
        let selected_index = if messages.is_empty() {
            0
        } else {
            messages.len() - 1 // Default to most recent
        };
        Self {
            messages,
            selected_index,
            on_select: None,
            on_cancel: None,
        }
    }
}

impl Component for ForkSelector {
    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let mut lines = vec![
            String::new(),
            theme.bold(&theme.accent("  Fork from Message")),
            theme.dim("  Select a user message to fork the session from that point."),
            String::new(),
        ];

        if self.messages.is_empty() {
            lines.push(theme.dim("  No user messages found"));
            lines.push(String::new());
            lines.push(theme.dim("  Press any key to close."));
            return lines;
        }

        // Calculate visible range (max ~10 visible)
        let max_visible = 10usize.min(self.messages.len());
        let start = if self.selected_index < max_visible / 2 {
            0
        } else {
            (self.selected_index - max_visible / 2).min(self.messages.len() - max_visible)
        };
        let end = start + max_visible;

        for i in start..end {
            let msg = &self.messages[i];
            let is_selected = i == self.selected_index;

            let prefix = if is_selected {
                theme.accent("  › ")
            } else {
                "    ".to_string()
            };

            let text = msg.text.replace('\n', " ").trim().to_string();
            let max_w = width.saturating_sub(6);
            let truncated = truncate_to_width(&text, max_w, "", false);
            let line = format!(
                "{}{}",
                prefix,
                if is_selected {
                    theme.bold(&truncated)
                } else {
                    theme.dim(&truncated)
                }
            );
            lines.push(line);

            // Metadata line
            let meta = format!("    Message {} of {}", msg.index + 1, msg.total);
            lines.push(theme.dim(&meta));
            lines.push(String::new());
        }

        // Scroll indicator
        if self.messages.len() > max_visible {
            lines.push(theme.dim(&format!(
                "  ({}/{})",
                self.selected_index + 1,
                self.messages.len()
            )));
        }

        lines.push(String::new());
        lines.push(theme.dim("  ↑↓ navigate · ↵ select · Esc cancel"));

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        let kb = get_keybindings();

        if kb.matches(key, ACTION_SELECT_UP) {
            if self.selected_index > 0 {
                self.selected_index -= 1;
            } else if !self.messages.is_empty() {
                self.selected_index = self.messages.len() - 1;
            }
            return true;
        }

        if kb.matches(key, ACTION_SELECT_DOWN) {
            if self.selected_index + 1 < self.messages.len() {
                self.selected_index += 1;
            } else if !self.messages.is_empty() {
                self.selected_index = 0;
            }
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CONFIRM) {
            if let Some(msg) = self.messages.get(self.selected_index)
                && let Some(cb) = self.on_select.take()
            {
                cb(msg.id.clone());
            }
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CANCEL) {
            if let Some(cb) = self.on_cancel.take() {
                cb();
            }
            return true;
        }

        false
    }
}
