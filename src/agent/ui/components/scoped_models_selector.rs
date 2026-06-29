//! ScopedModelsSelector component — matching pi's ScopedModelsSelectorComponent.
//!
//! Full-screen overlay for enabling/disabling models for Ctrl+P cycling.
//! Changes are session-only until explicitly persisted with Ctrl+S.
//!
//! Uses shared `Rc<RefCell<bool>>` for close signalling: the component sets
//! it to true when the user cancels or persists, and the main loop polls it.

use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keybindings::{
    ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM, ACTION_SELECT_DOWN, ACTION_SELECT_UP,
    get_keybindings,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ── Types ──────────────────────────────────────────────────────────

/// EnabledIds: null = all enabled (no filter), Some(vec) = explicit ordered subset.
pub type EnabledIds = Option<Vec<String>>;

fn is_enabled(enabled_ids: &EnabledIds, id: &str) -> bool {
    match enabled_ids {
        None => true,
        Some(ids) => ids.contains(&id.to_string()),
    }
}

fn toggle(enabled_ids: &EnabledIds, id: &str) -> EnabledIds {
    match enabled_ids {
        None => Some(vec![id.to_string()]),
        Some(ids) => {
            let id_s = id.to_string();
            if ids.contains(&id_s) {
                let result: Vec<String> = ids.iter().filter(|i| *i != &id_s).cloned().collect();
                Some(result)
            } else {
                let mut result = ids.clone();
                result.push(id_s);
                Some(result)
            }
        }
    }
}

fn enable_all(
    enabled_ids: &EnabledIds,
    all_ids: &[String],
    target_ids: Option<&[String]>,
) -> EnabledIds {
    match enabled_ids {
        None => None, // Already all enabled
        Some(ids) => {
            let targets = target_ids.unwrap_or(all_ids);
            let mut result = ids.clone();
            for id in targets {
                if !result.contains(id) {
                    result.push(id.clone());
                }
            }
            if result.len() == all_ids.len() {
                None
            } else {
                Some(result)
            }
        }
    }
}

fn clear_all(
    enabled_ids: &EnabledIds,
    all_ids: &[String],
    target_ids: Option<&[String]>,
) -> EnabledIds {
    match enabled_ids {
        None => match target_ids {
            Some(targets) => {
                let result: Vec<String> = all_ids
                    .iter()
                    .filter(|id| !targets.contains(id))
                    .cloned()
                    .collect();
                Some(result)
            }
            None => Some(vec![]),
        },
        Some(ids) => {
            let targets_set: std::collections::HashSet<&str> = target_ids
                .unwrap_or(ids)
                .iter()
                .map(|s| s.as_str())
                .collect();
            let result: Vec<String> = ids
                .iter()
                .filter(|id| !targets_set.contains(id.as_str()))
                .cloned()
                .collect();
            Some(result)
        }
    }
}

fn move_item(enabled_ids: &EnabledIds, id: &str, delta: isize) -> EnabledIds {
    match enabled_ids {
        None => None,
        Some(ids) => {
            let mut list = ids.clone();
            let pos = list.iter().position(|i| i == id);
            match pos {
                Some(idx) => {
                    let new_idx = idx as isize + delta;
                    if new_idx < 0 || new_idx >= list.len() as isize {
                        return Some(list);
                    }
                    list.swap(idx, new_idx as usize);
                    Some(list)
                }
                None => Some(list),
            }
        }
    }
}

fn get_sorted_ids(enabled_ids: &EnabledIds, all_ids: &[String]) -> Vec<String> {
    match enabled_ids {
        None => all_ids.to_vec(),
        Some(ids) => {
            let enabled_set: std::collections::HashSet<&str> =
                ids.iter().map(|s| s.as_str()).collect();
            let mut result = ids.clone();
            for id in all_ids {
                if !enabled_set.contains(id.as_str()) {
                    result.push(id.clone());
                }
            }
            result
        }
    }
}

// ── Model item for display ─────────────────────────────────────────

#[derive(Clone)]
struct ModelItem {
    full_id: String,
    provider: String,
    model_id: String,
    model_name: String,
    enabled: bool,
}

// ── Config and callbacks ───────────────────────────────────────────

pub struct ModelsConfig {
    pub all_models: Vec<(String, String, String)>, // (provider, id, name)
    pub enabled_model_ids: Option<Vec<String>>,    // null = all enabled
}

pub struct ModelsCallbacks {
    /// Called whenever the enabled model set or order changes (session-only, no persist).
    pub on_change: Box<dyn Fn(Option<Vec<String>>)>,
    /// Called when user wants to persist current selection to settings.
    pub on_persist: Box<dyn Fn(Option<Vec<String>>)>,
    /// Called when user cancels.
    pub on_cancel: Box<dyn Fn()>,
}

// ── ScopedModelsSelector component ──────────────────────────────────

pub struct ScopedModelsSelector {
    items: Vec<ModelItem>,
    all_ids: Vec<String>,
    enabled_ids: EnabledIds,
    all_items_sorted: Vec<ModelItem>,
    filtered_indices: Vec<usize>,
    selected_index: usize,
    search_query: String,
    max_visible: usize,
    is_dirty: bool,
    callbacks: ModelsCallbacks,
}

impl ScopedModelsSelector {
    pub fn new(config: ModelsConfig, callbacks: ModelsCallbacks) -> Self {
        let all_ids: Vec<String> = config
            .all_models
            .iter()
            .map(|(p, id, _)| format!("{}/{}", p, id))
            .collect();

        let items: Vec<ModelItem> = config
            .all_models
            .iter()
            .map(|(provider, model_id, name)| ModelItem {
                full_id: format!("{}/{}", provider, model_id),
                provider: provider.clone(),
                model_id: model_id.clone(),
                model_name: name.clone(),
                enabled: is_enabled(
                    &config.enabled_model_ids,
                    &format!("{}/{}", provider, model_id),
                ),
            })
            .collect();

        let enabled_ids = config.enabled_model_ids;

        let sorted = get_sorted_ids(&enabled_ids, &all_ids);
        let all_items_sorted: Vec<ModelItem> = sorted
            .iter()
            .filter_map(|full_id| {
                items
                    .iter()
                    .find(|item| item.full_id == *full_id)
                    .cloned()
                    .map(|mut item| {
                        item.enabled = is_enabled(&enabled_ids, &item.full_id);
                        item
                    })
            })
            .collect();

        let filtered_indices: Vec<usize> = (0..all_items_sorted.len()).collect();

        Self {
            items,
            all_ids,
            enabled_ids,
            all_items_sorted,
            filtered_indices,
            selected_index: 0,
            search_query: String::new(),
            max_visible: 10,
            is_dirty: false,
            callbacks,
        }
    }

    fn rebuild_sorted(&mut self) {
        let sorted = get_sorted_ids(&self.enabled_ids, &self.all_ids);
        self.all_items_sorted = sorted
            .iter()
            .filter_map(|full_id| {
                self.items
                    .iter()
                    .find(|item| item.full_id == *full_id)
                    .cloned()
                    .map(|mut item| {
                        item.enabled = is_enabled(&self.enabled_ids, &item.full_id);
                        item
                    })
            })
            .collect();
    }

    fn refresh(&mut self) {
        self.rebuild_sorted();
        let query = self.search_query.clone();
        self.filtered_indices = if query.trim().is_empty() {
            (0..self.all_items_sorted.len()).collect()
        } else {
            fuzzy_filter(&self.all_items_sorted, &query, |item| &item.full_id)
        };
        self.selected_index = self
            .selected_index
            .min(self.filtered_indices.len().saturating_sub(1));
    }

    fn get_item(&self, filtered_idx: usize) -> Option<&ModelItem> {
        self.filtered_indices
            .get(filtered_idx)
            .and_then(|&idx| self.all_items_sorted.get(idx))
    }

    fn notify_change(&self) {
        (self.callbacks.on_change)(self.enabled_ids.clone());
    }
}

impl Component for ScopedModelsSelector {
    fn render(&mut self, width: usize) -> Vec<String> {
        use crate::tui::util::truncate_to_width;
        let theme = current_theme();
        let mut lines: Vec<String> = Vec::new();

        // Top border
        lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));
        lines.push(String::new());

        // Title
        lines.push(format!(
            "  {}",
            theme.bold(&theme.fg_key(ThemeKey::Accent, "Model Configuration"))
        ));

        // Status / hint
        let enabled_count = match &self.enabled_ids {
            None => self.all_ids.len(),
            Some(ids) => ids.len(),
        };
        let _all_enabled = self.enabled_ids.is_none();
        let dirty_mark = if self.is_dirty {
            theme.fg_key(ThemeKey::Warning, " (unsaved)")
        } else {
            String::new()
        };
        lines.push(format!(
            "  {}",
            theme.dim(&format!(
                "Session-only. Ctrl+S to save to settings. {}/{} enabled{}",
                enabled_count,
                self.all_ids.len(),
                dirty_mark,
            ))
        ));
        lines.push(String::new());

        // Search input line
        let search_label = theme.dim("  Search: ");
        let search_value = if self.search_query.is_empty() {
            theme.dim("(type to filter)")
        } else {
            self.search_query.clone()
        };
        lines.push(format!("{}{}", search_label, search_value));
        lines.push(String::new());

        // Model list
        let count = self.filtered_indices.len();
        let start = self
            .selected_index
            .saturating_sub(self.max_visible / 2)
            .min(count.saturating_sub(self.max_visible));
        let end = (start + self.max_visible).min(count);

        if count == 0 {
            lines.push(theme.dim("  No matching models"));
        } else {
            for i in start..end {
                let item = &self.all_items_sorted[self.filtered_indices[i]];
                let is_selected = i == self.selected_index;
                let prefix = if is_selected {
                    theme.fg_key(ThemeKey::Accent, "→ ")
                } else {
                    "  ".to_string()
                };
                let model_text = if is_selected {
                    theme.fg_key(ThemeKey::Accent, &item.model_id)
                } else {
                    item.model_id.clone()
                };
                let provider_badge = theme.dim(&format!(" [{}]", item.provider));
                let enabled = is_enabled(&self.enabled_ids, &item.full_id);
                let status = if enabled {
                    theme.fg_key(ThemeKey::Success, " ✓")
                } else {
                    theme.dim(" ✗")
                };
                lines.push(truncate_to_width(
                    &format!("{}{}{}{}", prefix, model_text, provider_badge, status),
                    width.saturating_sub(4),
                    "",
                    false,
                ));
            }

            // Scroll indicator
            if count > self.max_visible {
                lines.push(theme.dim(&format!("  ({}/{})", self.selected_index + 1, count)));
            }

            // Show model name for selected item
            if let Some(item) = self.get_item(self.selected_index) {
                lines.push(String::new());
                lines.push(theme.dim(&format!("  Model Name: {}", item.model_name)));
            }
        }

        // Footer hints
        lines.push(String::new());
        let hints = [
            "Enter: toggle",
            "Ctrl+A: all",
            "Ctrl+D: clear",
            "Ctrl+P: provider",
            "Ctrl+\u{2191}/\u{2193}: reorder",
            "Ctrl+S: save",
        ];
        lines.push(theme.dim(&format!("  {}", hints.join(" · "))));

        // Bottom border
        lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        let kb = get_keybindings();

        // Up/Down navigation
        if kb.matches(key, ACTION_SELECT_UP) {
            if self.filtered_indices.is_empty() {
                return true;
            }
            self.selected_index = if self.selected_index == 0 {
                self.filtered_indices.len() - 1
            } else {
                self.selected_index - 1
            };
            return true;
        }

        if kb.matches(key, ACTION_SELECT_DOWN) {
            if self.filtered_indices.is_empty() {
                return true;
            }
            self.selected_index = if self.selected_index >= self.filtered_indices.len() - 1 {
                0
            } else {
                self.selected_index + 1
            };
            return true;
        }

        // Toggle on Enter
        if kb.matches(key, ACTION_SELECT_CONFIRM) {
            if let Some(item) = self.get_item(self.selected_index) {
                self.enabled_ids = toggle(&self.enabled_ids, &item.full_id);
                self.is_dirty = true;
                self.refresh();
                self.notify_change();
            }
            return true;
        }

        // Cancel on Escape
        if kb.matches(key, ACTION_SELECT_CANCEL) {
            (self.callbacks.on_cancel)();
            return false; // Let App's fallback pop the overlay
        }

        // Ctrl+A - Enable all (filtered if search active)
        if key.code == KeyCode::Char('a') && key.modifiers == KeyModifiers::CONTROL {
            let target_ids = if self.search_query.trim().is_empty() {
                None
            } else {
                let ids: Vec<String> = self
                    .filtered_indices
                    .iter()
                    .filter_map(|&idx| self.all_items_sorted.get(idx))
                    .map(|item| item.full_id.clone())
                    .collect();
                Some(ids)
            };
            self.enabled_ids = enable_all(&self.enabled_ids, &self.all_ids, target_ids.as_deref());
            self.is_dirty = true;
            self.refresh();
            self.notify_change();
            return true;
        }

        // Ctrl+D - Clear all (filtered if search active)
        if key.code == KeyCode::Char('d') && key.modifiers == KeyModifiers::CONTROL {
            let target_ids = if self.search_query.trim().is_empty() {
                None
            } else {
                let ids: Vec<String> = self
                    .filtered_indices
                    .iter()
                    .filter_map(|&idx| self.all_items_sorted.get(idx))
                    .map(|item| item.full_id.clone())
                    .collect();
                Some(ids)
            };
            self.enabled_ids = clear_all(&self.enabled_ids, &self.all_ids, target_ids.as_deref());
            self.is_dirty = true;
            self.refresh();
            self.notify_change();
            return true;
        }

        // Ctrl+P - Toggle provider of current item
        if key.code == KeyCode::Char('p') && key.modifiers == KeyModifiers::CONTROL {
            if let Some(item) = self.get_item(self.selected_index) {
                let provider = &item.provider;
                let provider_ids: Vec<String> = self
                    .all_ids
                    .iter()
                    .filter(|id| id.starts_with(&format!("{}/", provider)))
                    .cloned()
                    .collect();
                let all_enabled = provider_ids
                    .iter()
                    .all(|id| is_enabled(&self.enabled_ids, id));
                self.enabled_ids = if all_enabled {
                    clear_all(&self.enabled_ids, &self.all_ids, Some(&provider_ids))
                } else {
                    enable_all(&self.enabled_ids, &self.all_ids, Some(&provider_ids))
                };
                self.is_dirty = true;
                self.refresh();
                self.notify_change();
            }
            return true;
        }

        // Ctrl+Up - Reorder up
        if key.code == KeyCode::Up && key.modifiers == KeyModifiers::CONTROL {
            if let Some(item) = self.get_item(self.selected_index) {
                let full_id = item.full_id.clone();
                let new_ids = move_item(&self.enabled_ids, &full_id, -1);
                if new_ids != self.enabled_ids {
                    self.enabled_ids = new_ids;
                    self.is_dirty = true;
                    self.refresh();
                    // Re-find the item after refresh to track selection
                    if let Some(new_idx) = self
                        .all_items_sorted
                        .iter()
                        .position(|i| i.full_id == full_id)
                    {
                        // Find position in filtered_indices
                        if let Some(pos) = self.filtered_indices.iter().position(|&i| i == new_idx)
                        {
                            self.selected_index = pos;
                        }
                    }
                    self.notify_change();
                }
            }
            return true;
        }

        // Ctrl+Down - Reorder down
        if key.code == KeyCode::Down && key.modifiers == KeyModifiers::CONTROL {
            if let Some(item) = self.get_item(self.selected_index) {
                let full_id = item.full_id.clone();
                let new_ids = move_item(&self.enabled_ids, &full_id, 1);
                if new_ids != self.enabled_ids {
                    self.enabled_ids = new_ids;
                    self.is_dirty = true;
                    self.refresh();
                    // Re-find the item after refresh to track selection
                    if let Some(new_idx) = self
                        .all_items_sorted
                        .iter()
                        .position(|i| i.full_id == full_id)
                        && let Some(pos) = self.filtered_indices.iter().position(|&i| i == new_idx)
                    {
                        self.selected_index = pos;
                    }
                    self.notify_change();
                }
            }
            return true;
        }

        // Ctrl+S - Save/persist to settings
        if key.code == KeyCode::Char('s') && key.modifiers == KeyModifiers::CONTROL {
            (self.callbacks.on_persist)(self.enabled_ids.clone());
            self.is_dirty = false;
            return true;
        }

        // Ctrl+C - Clear search or cancel if empty
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            if !self.search_query.is_empty() {
                self.search_query.clear();
                self.refresh();
                return true;
            }
            return false;
        }

        // Backspace - delete from search
        if key.code == KeyCode::Backspace {
            if !self.search_query.is_empty() {
                self.search_query.pop();
                self.refresh();
            }
            return true;
        }

        // Typeable characters go to search
        if let KeyCode::Char(c) = key.code
            && !c.is_control()
        {
            self.search_query.push(c);
            self.refresh();
            return true;
        }

        false
    }
}
