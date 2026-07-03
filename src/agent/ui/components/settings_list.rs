//! SettingsList component — matching pi's SettingsList.
//!
//! A list of settings items with label/value layout, optional search,
//! and value cycling (Enter/Space).

use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keybindings::{
    ACTION_EDITOR_DELETE_CHAR_BACKWARD, ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM,
    ACTION_SELECT_DOWN, ACTION_SELECT_UP, get_keybindings,
};
use crate::tui::util::{truncate_to_width, visible_width, wrap_text_with_ansi};
use crossterm::event::KeyEvent;

// ── SettingItem ─────────────────────────────────────────────────

/// A single setting item in the list.
#[derive(Clone)]
pub struct SettingItem {
    /// Unique identifier for this setting.
    pub id: String,
    /// Display label (left side).
    pub label: String,
    /// Optional description shown when selected.
    pub description: Option<String>,
    /// Current value to display (right side).
    pub current_value: String,
    /// If Some, Enter/Space cycles through these values.
    pub values: Option<Vec<String>>,
}

// ── SettingsList component ──────────────────────────────────────

/// A scrollable list of settings items with optional search.
pub struct SettingsList {
    items: Vec<SettingItem>,
    filtered_indices: Vec<usize>,
    selected_index: usize,
    max_visible: usize,
    search_query: String,
    search_enabled: bool,
    /// Callback when a setting value changes: (id, new_value).
    on_change: Box<dyn FnMut(String, String)>,
    /// Callback when the user cancels (Escape).
    on_cancel: Box<dyn FnMut()>,
}

impl SettingsList {
    pub fn new(
        items: Vec<SettingItem>,
        max_visible: usize,
        on_change: Box<dyn FnMut(String, String)>,
        on_cancel: Box<dyn FnMut()>,
        search_enabled: bool,
    ) -> Self {
        let filtered_indices: Vec<usize> = (0..items.len()).collect();
        Self {
            items,
            filtered_indices,
            selected_index: 0,
            max_visible: max_visible.max(1),
            search_query: String::new(),
            search_enabled,
            on_change,
            on_cancel,
        }
    }

    /// Update an item's current_value in-place.
    pub fn update_value(&mut self, id: &str, new_value: String) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.current_value = new_value;
        }
    }

    /// Get a reference to the items.
    pub fn items(&self) -> &[SettingItem] {
        &self.items
    }

    fn effective_item(&self, index: usize) -> Option<&SettingItem> {
        self.filtered_indices
            .get(index)
            .and_then(|&i| self.items.get(i))
    }

    fn effective_count(&self) -> usize {
        self.filtered_indices.len()
    }

    fn move_up(&mut self) {
        let count = self.effective_count();
        if count == 0 {
            return;
        }
        self.selected_index = if self.selected_index == 0 {
            count - 1
        } else {
            self.selected_index - 1
        };
    }

    fn move_down(&mut self) {
        let count = self.effective_count();
        if count == 0 {
            return;
        }
        self.selected_index = if self.selected_index >= count - 1 {
            0
        } else {
            self.selected_index + 1
        };
    }

    fn activate_selected(&mut self) {
        let item_idx = match self.filtered_indices.get(self.selected_index) {
            Some(&i) => i,
            None => return,
        };
        let values = match self.items[item_idx].values.clone() {
            Some(v) if !v.is_empty() => v,
            _ => return,
        };
        let current = self.items[item_idx].current_value.clone();
        let current_idx = values.iter().position(|v| v == &current);
        let next_idx = match current_idx {
            Some(i) => (i + 1) % values.len(),
            None => 0,
        };
        let new_value = values[next_idx].clone();
        let id = self.items[item_idx].id.clone();
        self.items[item_idx].current_value = new_value.clone();
        (self.on_change)(id, new_value);
    }

    fn apply_search(&mut self) {
        if self.search_query.trim().is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            self.filtered_indices =
                fuzzy_filter(&self.items, &self.search_query, |item| &item.label);
        }
        self.selected_index = 0;
    }
}

impl Component for SettingsList {
    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let mut lines: Vec<String> = Vec::new();

        // Search input line
        if self.search_enabled {
            let search_text = if self.search_query.is_empty() {
                theme.dim("  Type to search...")
            } else {
                format!("  {}", theme.accent(&format!("🔍 {}", self.search_query)))
            };
            lines.push(truncate_to_width(&search_text, width, "", true));
            lines.push(String::new());
        }

        let count = self.effective_count();
        if count == 0 {
            let msg = if self.search_enabled && !self.search_query.is_empty() {
                "  No matching settings"
            } else {
                "  No settings available"
            };
            lines.push(truncate_to_width(&theme.dim(msg), width, "", true));
            add_hint_line(&mut lines, width, self.search_enabled, &theme);
            return lines;
        }

        // Calculate visible range with scrolling
        let half = self.max_visible / 2;
        let start = if count <= self.max_visible {
            0
        } else {
            let raw = self.selected_index.saturating_sub(half);
            raw.min(count.saturating_sub(self.max_visible))
        };
        let end = (start + self.max_visible).min(count);

        // Calculate max label width for alignment (capped at 30)
        let max_label_width = self
            .items
            .iter()
            .map(|item| visible_width(&item.label))
            .max()
            .unwrap_or(0)
            .min(30);

        // Render visible items
        for i in start..end {
            let item = match self
                .filtered_indices
                .get(i)
                .and_then(|&idx| self.items.get(idx))
            {
                Some(item) => item,
                None => continue,
            };

            let is_selected = i == self.selected_index;
            let prefix = if is_selected { "→ " } else { "  " };
            let prefix_width = 2;

            // Pad label to align values
            let label_width = visible_width(&item.label);
            let padding = max_label_width.saturating_sub(label_width);
            let padded_label = format!("{}{}", item.label, " ".repeat(padding));

            let label_styled = if is_selected {
                theme.bold_fg("text", &padded_label)
            } else {
                theme.text_color(&padded_label)
            };

            // Value
            let separator = "  ";
            let used_width = prefix_width + max_label_width + visible_width(separator);
            let value_max_width = width.saturating_sub(used_width + 2);

            let value_styled = if is_selected {
                theme.accent(&truncate_to_width(
                    &item.current_value,
                    value_max_width,
                    "",
                    true,
                ))
            } else {
                theme.muted(&truncate_to_width(
                    &item.current_value,
                    value_max_width,
                    "",
                    true,
                ))
            };

            let line = format!("{}{}{}{}", prefix, label_styled, separator, value_styled);
            lines.push(truncate_to_width(&line, width, "", true));
        }

        // Scroll indicator
        if count > self.max_visible {
            let scroll = format!("  ({}/{})", self.selected_index + 1, count);
            lines.push(theme.dim(&truncate_to_width(&scroll, width - 2, "", true)));
        }

        // Description for selected item
        if let Some(item) = self.effective_item(self.selected_index)
            && let Some(ref desc) = item.description
        {
            lines.push(String::new());
            for wrapped_line in wrap_text_with_ansi(desc, width.saturating_sub(4)) {
                lines.push(theme.muted(&format!("  {}", wrapped_line)));
            }
        }

        // Hint line
        add_hint_line(&mut lines, width, self.search_enabled, &theme);

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        let kb = get_keybindings();

        if kb.matches(key, ACTION_SELECT_UP) {
            self.move_up();
            return true;
        }

        if kb.matches(key, ACTION_SELECT_DOWN) {
            self.move_down();
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CONFIRM) || is_space(key) {
            self.activate_selected();
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CANCEL) {
            (self.on_cancel)();
            return true;
        }

        // Search: printable characters
        if self.search_enabled {
            if let KeyEvent {
                code: crossterm::event::KeyCode::Char(c),
                modifiers,
                ..
            } = key
                && !modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                && !modifiers.contains(crossterm::event::KeyModifiers::ALT)
            {
                self.search_query.push(*c);
                self.apply_search();
                return true;
            }

            if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_BACKWARD) {
                self.search_query.pop();
                self.apply_search();
                return true;
            }
        }

        false
    }

    fn invalidate(&mut self) {
        // No cache to invalidate
    }
}

/// Check if a key event is a Space key press.
fn is_space(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: crossterm::event::KeyCode::Char(' '),
            modifiers: m,
            ..
        } if m.is_empty() || *m == crossterm::event::KeyModifiers::NONE
    )
}

fn add_hint_line(
    lines: &mut Vec<String>,
    width: usize,
    search_enabled: bool,
    theme: &crate::agent::ui::theme::RabTheme,
) {
    lines.push(String::new());
    let hint = if search_enabled {
        "  Type to search · Enter/Space to change · Esc to cancel"
    } else {
        "  Enter/Space to change · Esc to cancel"
    };
    lines.push(truncate_to_width(&theme.dim(hint), width, "", true));
}
