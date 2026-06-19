use crate::tui::component::Component;
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keys::{Key, matches_key};
use crate::tui::util::truncate_to_width;
use crossterm::event::KeyEvent;

/// An item in a SelectList.
#[derive(Debug, Clone)]
pub struct SelectItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl SelectItem {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Theme functions for SelectList styling.
pub struct SelectListTheme {
    pub selected_prefix: Box<dyn Fn(&str) -> String>,
    pub selected_text: Box<dyn Fn(&str) -> String>,
    pub normal_text: Box<dyn Fn(&str) -> String>,
    pub description: Box<dyn Fn(&str) -> String>,
    pub scroll_info: Box<dyn Fn(&str) -> String>,
    pub no_match: Box<dyn Fn(&str) -> String>,
    pub hint: Box<dyn Fn(&str) -> String>,
}

impl Default for SelectListTheme {
    fn default() -> Self {
        Self {
            selected_prefix: Box::new(|s| format!("\x1b[1m> {}\x1b[0m", s)),
            selected_text: Box::new(|s| format!("\x1b[1m{}\x1b[0m", s)),
            normal_text: Box::new(|s| format!("  {}", s)),
            description: Box::new(|s| format!("    {}", s)),
            scroll_info: Box::new(|s| s.to_string()),
            no_match: Box::new(|s| s.to_string()),
            hint: Box::new(|s| s.to_string()),
        }
    }
}

/// Scrollable list with optional fuzzy search and selection.
pub struct SelectList {
    items: Vec<SelectItem>,
    selected_index: usize,
    max_visible: usize,
    scroll_offset: usize,
    search_query: String,
    search_enabled: bool,
    filtered_indices: Vec<usize>,
    theme: SelectListTheme,
    pub on_select: Option<Box<dyn FnMut(String)>>,
    pub on_cancel: Option<Box<dyn FnMut()>>,
}

impl SelectList {
    /// Create a new SelectList.
    ///
    /// - `items`: The items to display.
    /// - `max_visible`: Maximum number of visible items at once.
    /// - `theme`: Styling functions.
    pub fn new(items: Vec<SelectItem>, max_visible: usize, theme: SelectListTheme) -> Self {
        let filtered_indices: Vec<usize> = (0..items.len()).collect();
        Self {
            items,
            selected_index: 0,
            max_visible: max_visible.max(1),
            scroll_offset: 0,
            search_query: String::new(),
            search_enabled: false,
            filtered_indices,
            theme,
            on_select: None,
            on_cancel: None,
        }
    }

    /// Enable interactive search (fuzzy filtering as user types).
    pub fn with_search(mut self) -> Self {
        self.search_enabled = true;
        self
    }

    /// Update the items list.
    pub fn set_items(&mut self, items: Vec<SelectItem>) {
        self.items = items;
        self.filtered_indices = (0..self.items.len()).collect();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.search_query.clear();

        // Re-apply any existing search
        if !self.search_query.is_empty() {
            self.apply_search();
        }
    }

    /// Get the currently selected item, if any.
    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.filtered_indices
            .get(self.selected_index)
            .and_then(|&idx| self.items.get(idx))
    }

    fn apply_search(&mut self) {
        if self.search_query.trim().is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            self.filtered_indices =
                fuzzy_filter(&self.items, &self.search_query, |item| &item.label);
        }
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
        self.adjust_scroll();
    }

    fn move_down(&mut self) {
        if self.selected_index + 1 < self.filtered_indices.len() {
            self.selected_index += 1;
        }
        self.adjust_scroll();
    }

    fn adjust_scroll(&mut self) {
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected_index - self.max_visible + 1;
        }
    }
}

impl Component for SelectList {
    fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        if self.filtered_indices.is_empty() {
            if !self.search_query.is_empty() {
                lines.push((self.theme.no_match)("No matches"));
            }
            return lines;
        }

        let end = (self.scroll_offset + self.max_visible).min(self.filtered_indices.len());
        let visible_slice = &self.filtered_indices[self.scroll_offset..end];

        for (i, &item_idx) in visible_slice.iter().enumerate() {
            let actual_idx = self.scroll_offset + i;
            let item = &self.items[item_idx];
            let is_selected = actual_idx == self.selected_index;

            let prefix = if is_selected {
                (self.theme.selected_prefix)("")
            } else {
                "  ".to_string()
            };

            let label = if is_selected {
                (self.theme.selected_text)(&item.label)
            } else {
                (self.theme.normal_text)(&item.label)
            };

            let desc = if let Some(ref d) = item.description {
                format!(" {}", (self.theme.description)(d))
            } else {
                String::new()
            };

            let line = format!("{}{}{}", prefix, label, desc);
            lines.push(truncate_to_width(&line, width, "", false));
        }

        // Scroll indicator
        if self.filtered_indices.len() > self.max_visible {
            let indicator = format!(
                "({}/{})",
                self.selected_index + 1,
                self.filtered_indices.len()
            );
            lines.push((self.theme.scroll_info)(&indicator));
        }

        // Hint line
        lines.push((self.theme.hint)("↑↓ navigate • enter select • esc cancel"));

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        if matches_key(key, &Key::Up) {
            self.move_up();
            return true;
        }

        if matches_key(key, &Key::Down) {
            self.move_down();
            return true;
        }

        if matches_key(key, &Key::Enter) {
            let value = self.selected_item().map(|item| item.value.clone());
            if let Some(value) = value
                && let Some(ref mut cb) = self.on_select
            {
                cb(value);
            }
            return true;
        }

        if matches_key(key, &Key::Escape) {
            if let Some(ref mut cb) = self.on_cancel {
                cb();
            }
            return true;
        }

        // Search: printable characters update search query
        if self.search_enabled {
            if let crossterm::event::KeyCode::Char(c) = key.code
                && !key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL)
                && !key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
            {
                self.search_query.push(c);
                self.apply_search();
                return true;
            }

            if matches_key(key, &Key::Backspace) {
                self.search_query.pop();
                self.apply_search();
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_items() -> Vec<SelectItem> {
        vec![
            SelectItem::new("a", "Alpha"),
            SelectItem::new("b", "Beta"),
            SelectItem::new("c", "Gamma"),
        ]
    }

    #[test]
    fn test_basic_navigation() {
        let mut list = SelectList::new(make_items(), 10, SelectListTheme::default());
        assert_eq!(list.selected_item().unwrap().value, "a");

        list.move_down();
        assert_eq!(list.selected_item().unwrap().value, "b");

        list.move_up();
        assert_eq!(list.selected_item().unwrap().value, "a");
    }

    #[test]
    fn test_selection_wraps() {
        let mut list = SelectList::new(make_items(), 10, SelectListTheme::default());
        // Can't go above 0
        list.move_up();
        assert_eq!(list.selected_item().unwrap().value, "a");

        // Can't go past end
        for _ in 0..5 {
            list.move_down();
        }
        assert_eq!(list.selected_item().unwrap().value, "c");
    }

    #[test]
    fn test_render() {
        let list = SelectList::new(make_items(), 10, SelectListTheme::default());
        let lines = list.render(40);
        assert!(lines.len() >= 3); // items + hint
    }
}
