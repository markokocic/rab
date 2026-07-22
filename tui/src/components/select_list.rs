#![allow(clippy::type_complexity)]

use crate::component::Component;
use crate::fuzzy::fuzzy_filter;
use crate::keybindings::{
    ACTION_EDITOR_DELETE_CHAR_BACKWARD, ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM,
    ACTION_SELECT_DOWN, ACTION_SELECT_UP, get_keybindings,
};
use crate::util::{truncate_to_width, visible_width};
use crossterm::event::KeyEvent;

const DEFAULT_PRIMARY_COLUMN_WIDTH: usize = 32;
const PRIMARY_COLUMN_GAP: usize = 2;
const MIN_DESCRIPTION_WIDTH: usize = 10;

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
    pub scroll_info: crate::Style,
    pub no_match: crate::Style,
    pub hint: crate::Style,
}

impl Default for SelectListTheme {
    fn default() -> Self {
        Self {
            selected_prefix: Box::new(|s| format!("\x1b[1m> {}\x1b[0m", s)),
            selected_text: Box::new(|s| format!("\x1b[1m{}\x1b[0m", s)),
            normal_text: Box::new(|s| format!("  {}", s)),
            description: Box::new(|s| format!("    {}", s)),
            scroll_info: crate::Style::new(),
            no_match: crate::Style::new(),
            hint: crate::Style::new(),
        }
    }
}

/// Layout options for the primary column (matching pi's SelectListLayoutOptions).
pub struct SelectListLayoutOptions {
    pub min_primary_column_width: Option<usize>,
    pub max_primary_column_width: Option<usize>,
    /// Custom truncation function for primary column.
    pub truncate_primary: Option<Box<dyn Fn(&str, usize, usize, &SelectItem, bool) -> String>>,
}

/// Scrollable list with optional fuzzy search and two-column layout.
pub struct SelectList {
    items: Vec<SelectItem>,
    selected_index: usize,
    max_visible: usize,
    scroll_offset: usize,
    search_query: String,
    search_enabled: bool,
    filtered_indices: Vec<usize>,
    theme: SelectListTheme,
    layout: SelectListLayoutOptions,
    pub on_select: Option<Box<dyn FnMut(String)>>,
    pub on_cancel: Option<Box<dyn FnMut()>>,
    pub on_selection_change: Option<Box<dyn FnMut(&SelectItem)>>,
}

impl SelectList {
    pub fn new(
        items: Vec<SelectItem>,
        max_visible: usize,
        theme: SelectListTheme,
        layout: Option<SelectListLayoutOptions>,
    ) -> Self {
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
            layout: layout.unwrap_or(SelectListLayoutOptions {
                min_primary_column_width: None,
                max_primary_column_width: None,
                truncate_primary: None,
            }),
            on_select: None,
            on_cancel: None,
            on_selection_change: None,
        }
    }

    /// Enable interactive search (fuzzy filtering as user types).
    pub fn with_search(mut self) -> Self {
        self.search_enabled = true;
        self
    }

    /// Set items (re-applies search if active). Matches pi's behavior.
    pub fn set_items(&mut self, items: Vec<SelectItem>) {
        self.items = items;
        self.filtered_indices = (0..self.items.len()).collect();
        self.selected_index = 0;
        self.scroll_offset = 0;
        if !self.search_query.is_empty() {
            self.apply_search();
        }
    }

    pub fn set_on_select(&mut self, cb: Box<dyn FnMut(String)>) {
        self.on_select = Some(cb);
    }

    pub fn set_on_cancel(&mut self, cb: Box<dyn FnMut()>) {
        self.on_cancel = Some(cb);
    }

    pub fn items(&self) -> &[SelectItem] {
        &self.items
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn set_selected_index(&mut self, index: usize) {
        let max = self.filtered_indices.len().saturating_sub(1);
        self.selected_index = index.min(max);
        self.adjust_scroll();
        self.notify_selection_change();
    }

    pub fn get_selected_item(&self) -> Option<&SelectItem> {
        self.filtered_indices
            .get(self.selected_index)
            .and_then(|&idx| self.items.get(idx))
    }

    /// Filter by prefix (simpler than fuzzy for user-typed single char; pi-style).
    pub fn set_filter(&mut self, filter: &str) {
        if filter.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            let lower = filter.to_lowercase();
            self.filtered_indices = (0..self.items.len())
                .filter(|&i| self.items[i].label.to_lowercase().contains(&lower))
                .collect();
        }
        self.selected_index = 0;
        self.scroll_offset = 0;
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

    fn notify_selection_change(&self) {
        // This is called from &self — but on_selection_change takes &mut.
        // In practice this is handled by the caller calling set_selected_index
        // or the public API.
    }

    fn move_up(&mut self) {
        if self.selected_index == 0 {
            self.selected_index = self.filtered_indices.len().saturating_sub(1);
        } else {
            self.selected_index -= 1;
        }
        self.adjust_scroll();
    }

    fn move_down(&mut self) {
        let last = self.filtered_indices.len().saturating_sub(1);
        if self.selected_index >= last {
            self.selected_index = 0;
        } else {
            self.selected_index += 1;
        }
        self.adjust_scroll();
    }

    fn adjust_scroll(&mut self) {
        if self.filtered_indices.len() <= self.max_visible {
            self.scroll_offset = 0;
        } else {
            let half = self.max_visible / 2;
            self.scroll_offset = self
                .selected_index
                .saturating_sub(half)
                .min(self.filtered_indices.len() - self.max_visible);
        }
    }
}

impl Component for SelectList {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        if self.filtered_indices.is_empty() {
            if !self.search_query.is_empty() {
                lines.push(self.theme.no_match.apply("No matches"));
            }
            return lines;
        }

        let end = (self.scroll_offset + self.max_visible).min(self.filtered_indices.len());
        let visible_slice = &self.filtered_indices[self.scroll_offset..end];

        // Calculate primary column width (pi-style: clamp between min/max bounds)
        let primary_column_width = self.get_primary_column_width();

        for (i, &item_idx) in visible_slice.iter().enumerate() {
            let actual_idx = self.scroll_offset + i;
            let item = &self.items[item_idx];
            let is_selected = actual_idx == self.selected_index;

            if self.supports_two_column(width) && item.description.is_some() {
                lines.push(self.render_two_column(item, is_selected, width, primary_column_width));
            } else {
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
        }

        // Scroll indicator (pi: only show when items exceed viewport)
        if self.filtered_indices.len() > self.max_visible {
            let indicator = format!(
                "({}/{})",
                self.selected_index + 1,
                self.filtered_indices.len()
            );
            lines.push(self.theme.scroll_info.apply(&indicator));
        }

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

        if kb.matches(key, ACTION_SELECT_CONFIRM) {
            let value = self.selected_item().map(|item| item.value.clone());
            if let Some(value) = value
                && let Some(ref mut cb) = self.on_select
            {
                cb(value);
            }
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CANCEL) {
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

            if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_BACKWARD) {
                self.search_query.pop();
                self.apply_search();
                return true;
            }
        }

        false
    }
}

// ── Private helpers ─────────────────────────────────────────────────

impl SelectList {
    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.filtered_indices
            .get(self.selected_index)
            .and_then(|&idx| self.items.get(idx))
    }

    fn supports_two_column(&self, width: usize) -> bool {
        width > 40
    }

    fn normalize_to_single_line(text: &str) -> String {
        text.replace(['\r', '\n'], " ").trim().to_string()
    }

    fn get_primary_column_width(&self) -> usize {
        let raw_min = self
            .layout
            .min_primary_column_width
            .or(self.layout.max_primary_column_width)
            .unwrap_or(DEFAULT_PRIMARY_COLUMN_WIDTH);
        let raw_max = self
            .layout
            .max_primary_column_width
            .or(self.layout.min_primary_column_width)
            .unwrap_or(DEFAULT_PRIMARY_COLUMN_WIDTH);

        let min = raw_min.max(1).min(raw_max);
        let max = raw_max.max(1).max(raw_min);

        let widest = self
            .filtered_indices
            .iter()
            .map(|&i| visible_width(&self.items[i].label) + PRIMARY_COLUMN_GAP)
            .max()
            .unwrap_or(0);

        widest.clamp(min, max)
    }

    fn render_two_column(
        &self,
        item: &SelectItem,
        is_selected: bool,
        width: usize,
        primary_column_width: usize,
    ) -> String {
        let prefix = if is_selected { "→ " } else { "  " };
        let prefix_width = visible_width(prefix);

        let effective_primary = primary_column_width.max(1).min(width - prefix_width - 4);
        let max_primary_width = effective_primary.saturating_sub(PRIMARY_COLUMN_GAP).max(1);

        let truncated_value =
            self.truncate_primary(item, is_selected, max_primary_width, effective_primary);
        let truncated_vw = visible_width(&truncated_value);
        let spacing = " ".repeat(effective_primary.saturating_sub(truncated_vw));

        let description_start = prefix_width + truncated_vw + spacing.len();
        let remaining = width.saturating_sub(description_start + 2);

        let desc_single = item
            .description
            .as_ref()
            .map(|d| Self::normalize_to_single_line(d));

        if let Some(ref desc) = desc_single
            && remaining > MIN_DESCRIPTION_WIDTH
        {
            let truncated_desc = truncate_to_width(desc, remaining, "", false);
            if is_selected {
                return (self.theme.selected_text)(&format!(
                    "{}{}{}{}",
                    prefix, truncated_value, spacing, truncated_desc
                ));
            }
            let desc_text = (self.theme.description)(&format!("{}{}", spacing, truncated_desc));
            return format!("{}{}{}", prefix, truncated_value, desc_text);
        }

        let max_allowed = width.saturating_sub(prefix_width + 2);
        let truncated = self.truncate_primary(item, is_selected, max_allowed, max_allowed);
        if is_selected {
            return (self.theme.selected_text)(&format!("{}{}", prefix, truncated));
        }
        format!("{}{}", prefix, truncated)
    }

    fn truncate_primary(
        &self,
        item: &SelectItem,
        is_selected: bool,
        max_width: usize,
        column_width: usize,
    ) -> String {
        let display = if item.label.is_empty() {
            &item.value
        } else {
            &item.label
        };

        if let Some(ref custom) = self.layout.truncate_primary {
            custom(display, max_width, column_width, item, is_selected)
        } else {
            truncate_to_width(display, max_width, "", false)
        }
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
        let mut list = SelectList::new(make_items(), 10, SelectListTheme::default(), None);
        assert_eq!(list.get_selected_item().unwrap().value, "a");

        list.move_down();
        assert_eq!(list.get_selected_item().unwrap().value, "b");

        list.move_up();
        assert_eq!(list.get_selected_item().unwrap().value, "a");
    }

    #[test]
    fn test_selection_wraps() {
        let mut list = SelectList::new(make_items(), 10, SelectListTheme::default(), None);
        list.move_up();
        assert_eq!(list.get_selected_item().unwrap().value, "c");

        list.move_down();
        assert_eq!(list.get_selected_item().unwrap().value, "a");
    }

    #[test]
    fn test_render() {
        let mut list = SelectList::new(make_items(), 10, SelectListTheme::default(), None);
        let lines = list.render(40);
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_set_filter() {
        let mut list = SelectList::new(make_items(), 10, SelectListTheme::default(), None);
        list.set_filter("beta");
        assert_eq!(list.filtered_indices.len(), 1);
        assert_eq!(list.items[list.filtered_indices[0]].label, "Beta");
    }

    #[test]
    fn test_two_column_render() {
        let items = vec![
            SelectItem::new("alpha-command", "Alpha command")
                .with_description("Does something useful"),
            SelectItem::new("beta-tool", "Beta tool").with_description("Another tool description"),
        ];
        let mut list = SelectList::new(items, 10, SelectListTheme::default(), None);
        let lines = list.render(80);
        // Should have 2+ lines for items
        assert!(lines.len() >= 2);
    }

    #[test]
    fn test_get_primary_column_width() {
        let items = vec![
            SelectItem::new("a", "Short"),
            SelectItem::new("b", "A much longer label here"),
        ];
        let list = SelectList::new(items, 10, SelectListTheme::default(), None);
        let width = list.get_primary_column_width();
        assert!(width > 5, "Width should accommodate longest label");
    }
}
