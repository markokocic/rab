#![allow(clippy::type_complexity)]

use crate::tui::component::Component;
use crate::tui::components::input::Input;
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keybindings::{
    ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM, ACTION_SELECT_DOWN, ACTION_SELECT_UP,
    get_keybindings,
};
use crate::tui::util::{truncate_to_width, visible_width, wrap_text_with_ansi};
use crossterm::event::KeyEvent;

/// A setting item that can be toggled or expanded into a submenu.
pub struct SettingItem {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub current_value: String,
    pub values: Option<Vec<String>>,
    /// Optional submenu: takes current value and a done callback.
    /// When provided, Enter/Space opens this submenu instead of cycling values.
    pub submenu: Option<Box<dyn Fn(String, Box<dyn Fn(Option<String>)>) -> Box<dyn Component>>>,
}

impl SettingItem {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        current_value: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            current_value: current_value.into(),
            values: None,
            submenu: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_values(mut self, values: Vec<String>) -> Self {
        self.values = Some(values);
        self
    }

    pub fn with_submenu(
        mut self,
        submenu: Box<dyn Fn(String, Box<dyn Fn(Option<String>)>) -> Box<dyn Component>>,
    ) -> Self {
        self.submenu = Some(submenu);
        self
    }
}

/// Theme for SettingsList.
pub struct SettingsListTheme {
    pub selected_prefix: Box<dyn Fn(&str) -> String>,
    pub selected_label: Box<dyn Fn(&str) -> String>,
    pub normal_label: Box<dyn Fn(&str) -> String>,
    pub value_text: Box<dyn Fn(&str) -> String>,
    pub description: Box<dyn Fn(&str) -> String>,
    pub scroll_info: Box<dyn Fn(&str) -> String>,
    pub hint: Box<dyn Fn(&str) -> String>,
}

impl Default for SettingsListTheme {
    fn default() -> Self {
        Self {
            selected_prefix: Box::new(|s| format!("\x1b[1m> {}\x1b[0m", s)),
            selected_label: Box::new(|s| format!("\x1b[1m{}\x1b[0m", s)),
            normal_label: Box::new(|s| format!("  {}", s)),
            value_text: Box::new(|s| s.to_string()),
            description: Box::new(|s| format!("  {}", s)),
            scroll_info: Box::new(|s| s.to_string()),
            hint: Box::new(|s| s.to_string()),
        }
    }
}

/// Options for SettingsList.
#[derive(Default)]
pub struct SettingsListOptions {
    pub enable_search: bool,
}

/// Scrollable settings list where items can toggle values or open submenus.
/// Supports two-column layout (label + value) and submenu components (matching pi).
pub struct SettingsList {
    items: Vec<SettingItem>,
    selected_index: usize,
    max_visible: usize,
    scroll_offset: usize,
    search_input: Input,
    search_active: bool,
    enable_search: bool,
    filtered_indices: Vec<usize>,
    theme: SettingsListTheme,
    on_change: Option<Box<dyn FnMut(&str, &str)>>,
    on_cancel: Option<Box<dyn FnMut()>>,
    // Submenu state
    submenu_component: Option<Box<dyn Component>>,
    submenu_item_index: Option<usize>,
}

impl SettingsList {
    pub fn new(
        items: Vec<SettingItem>,
        max_visible: usize,
        theme: SettingsListTheme,
        on_change: Box<dyn FnMut(&str, &str)>,
        on_cancel: Box<dyn FnMut()>,
        options: SettingsListOptions,
    ) -> Self {
        let filtered_indices: Vec<usize> = (0..items.len()).collect();
        Self {
            items,
            selected_index: 0,
            max_visible: max_visible.max(1),
            scroll_offset: 0,
            search_input: Input::new().with_prompt("> "),
            search_active: options.enable_search,
            enable_search: options.enable_search,
            filtered_indices,
            theme,
            on_change: Some(on_change),
            on_cancel: Some(on_cancel),
            submenu_component: None,
            submenu_item_index: None,
        }
    }

    pub fn update_value(&mut self, id: &str, new_value: &str) {
        for item in &mut self.items {
            if item.id == id {
                item.current_value = new_value.to_string();
                break;
            }
        }
    }

    fn apply_search(&mut self) {
        let query = self.search_input.get_value();
        if query.trim().is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            self.filtered_indices = fuzzy_filter(&self.items, query, |item| &item.label);
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

    fn activate_item(&mut self) {
        let item_idx = *self.filtered_indices.get(self.selected_index).unwrap_or(&0);
        let item = &mut self.items[item_idx];

        // Submenu takes priority
        if let Some(ref submenu_fn) = item.submenu {
            let current_value = item.current_value.clone();
            let item_index = self.selected_index;

            // Temporarily take the on_change to move into the closure
            let mut saved_on_change = None;
            std::mem::swap(&mut self.on_change, &mut saved_on_change);

            let done_cb: Box<dyn Fn(Option<String>)> =
                Box::new(move |selected_value: Option<String>| {
                    if let Some(_val) = selected_value {
                        // Update the item's value — need a way to reach back
                        // This is a simplified version; a full implementation would use
                        // channels or shared state. For now, the submenu caller handles persistence.
                    }
                });

            self.submenu_component = Some(submenu_fn(current_value, done_cb));
            self.submenu_item_index = Some(item_index);

            // Restore on_change
            std::mem::swap(&mut self.on_change, &mut saved_on_change);
        } else if let Some(ref values) = item.values.clone()
            && !values.is_empty()
        {
            let current_pos = values
                .iter()
                .position(|v| v == &item.current_value)
                .unwrap_or(0);
            let next_pos = (current_pos + 1) % values.len();
            item.current_value = values[next_pos].clone();
            let id = item.id.clone();
            let val = item.current_value.clone();
            if let Some(ref mut cb) = self.on_change {
                cb(&id, &val);
            }
        }
    }

    fn close_submenu(&mut self) {
        self.submenu_component = None;
        if let Some(idx) = self.submenu_item_index {
            self.selected_index = idx;
            self.submenu_item_index = None;
        }
    }

    fn add_hint_line(&self, lines: &mut Vec<String>, width: usize) {
        lines.push(String::new());
        lines.push(truncate_to_width(
            &(self.theme.hint)(if self.enable_search {
                "  Type to search · Enter/Space to change · Esc to cancel"
            } else {
                "  Enter/Space to change · Esc to cancel"
            }),
            width,
            "",
            false,
        ));
    }
}

impl Component for SettingsList {
    fn render(&self, width: usize) -> Vec<String> {
        // If submenu is active, render it instead
        if let Some(ref sub) = self.submenu_component {
            return sub.render(width);
        }

        let mut lines = Vec::new();

        // Search box
        if self.enable_search {
            lines.extend(self.search_input.render(width));
            lines.push(String::new());
        }

        if self.filtered_indices.is_empty() {
            if !self.search_input.get_value().is_empty() {
                lines.push((self.theme.hint)("No matching settings"));
            }
            self.add_hint_line(&mut lines, width);
            return lines;
        }

        let end = (self.scroll_offset + self.max_visible).min(self.filtered_indices.len());
        let visible_slice = &self.filtered_indices[self.scroll_offset..end];

        // Calculate max label width for alignment (pi-style: max 30)
        let max_label_width = self
            .filtered_indices
            .iter()
            .map(|&i| visible_width(&self.items[i].label))
            .max()
            .unwrap_or(0)
            .min(30);

        for (i, &item_idx) in visible_slice.iter().enumerate() {
            let actual_idx = self.scroll_offset + i;
            let is_selected = actual_idx == self.selected_index;
            let item = &self.items[item_idx];

            let prefix = if is_selected {
                (self.theme.selected_prefix)("")
            } else {
                "  ".to_string()
            };
            let prefix_width = visible_width(&prefix);

            // Pad label for alignment
            let label_padded = format!(
                "{}{}",
                item.label,
                " ".repeat(max_label_width.saturating_sub(visible_width(&item.label)))
            );
            let label = if is_selected {
                (self.theme.selected_label)(&label_padded)
            } else {
                (self.theme.normal_label)(&label_padded)
            };

            // Value with remaining space
            let separator = "  ";
            let used = prefix_width + max_label_width + visible_width(separator);
            let value_max = width.saturating_sub(used + 2);
            let value = (self.theme.value_text)(&truncate_to_width(
                &item.current_value,
                value_max,
                "",
                false,
            ));

            let line = format!("{}{}{}{}", prefix, label, separator, value);
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

        // Description of selected item
        if let Some(item_idx) = self.filtered_indices.get(self.selected_index).copied()
            && let Some(ref desc) = self.items[item_idx].description
        {
            lines.push(String::new());
            for desc_line in wrap_text_with_ansi(desc, width.saturating_sub(2)) {
                lines.push((self.theme.description)(&desc_line));
            }
        }

        self.add_hint_line(&mut lines, width);
        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        // If submenu is active, delegate all input to it
        if let Some(ref mut sub) = self.submenu_component {
            let consumed = sub.handle_input(key);
            if consumed {
                return true;
            }
            // Submenu didn't consume — it may have closed via done()
            self.close_submenu();
            return true;
        }

        let kb = get_keybindings();

        // Search input handling
        if self.search_active {
            if kb.matches(key, ACTION_SELECT_DOWN) || kb.matches(key, ACTION_SELECT_UP) {
                self.search_active = false;
                return self.handle_input(key);
            }
            self.search_input.handle_input(key);
            self.apply_search();
            return true;
        }

        if kb.matches(key, ACTION_SELECT_UP) {
            self.move_up();
            return true;
        }

        if kb.matches(key, ACTION_SELECT_DOWN) {
            self.move_down();
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CONFIRM)
            || matches!(key.code, crossterm::event::KeyCode::Char(' '))
        {
            self.activate_item();
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CANCEL) {
            if let Some(ref mut cb) = self.on_cancel {
                cb();
            }
            return true;
        }

        // If search is enabled, any printable char activates search
        if self.enable_search
            && let crossterm::event::KeyCode::Char(_) = key.code
            && !key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
            && !key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
        {
            self.search_active = true;
            self.search_input.handle_input(key);
            self.apply_search();
            return true;
        }

        false
    }

    fn invalidate(&mut self) {
        self.search_input.invalidate();
        if let Some(ref mut sub) = self.submenu_component {
            sub.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_items() -> Vec<SettingItem> {
        vec![
            SettingItem::new("verbose", "Verbose mode", "off")
                .with_values(vec!["on".to_string(), "off".to_string()])
                .with_description("Enable verbose logging"),
            SettingItem::new("color", "Color output", "on")
                .with_values(vec!["on".to_string(), "off".to_string()]),
        ]
    }

    #[test]
    fn test_cycle_value() {
        let mut list = SettingsList::new(
            make_items(),
            10,
            SettingsListTheme::default(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            SettingsListOptions::default(),
        );

        let item = &list.items[0];
        assert_eq!(item.current_value, "off");

        list.activate_item();

        let item = &list.items[0];
        assert_eq!(item.current_value, "on");
    }

    #[test]
    fn test_render() {
        let list = SettingsList::new(
            make_items(),
            10,
            SettingsListTheme::default(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            SettingsListOptions::default(),
        );
        let lines = list.render(60);
        assert!(lines.len() >= 2);
    }

    #[test]
    fn test_hint_line_shown() {
        let list = SettingsList::new(
            make_items(),
            10,
            SettingsListTheme::default(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            SettingsListOptions::default(),
        );
        let lines = list.render(60);
        let has_hint = lines.iter().any(|l| l.contains("Esc to cancel"));
        assert!(has_hint, "Hint should be visible");
    }
}
