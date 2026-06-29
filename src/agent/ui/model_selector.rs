//! ModelSelector component — matching pi's ModelSelectorComponent.
//!
//! Full-screen overlay for selecting a model with search.
//! Supports switching between "all" and "scoped" model views (Tab).

use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keybindings::{
    ACTION_INPUT_TAB, ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM, ACTION_SELECT_DOWN,
    ACTION_SELECT_UP, get_keybindings,
};
use crossterm::event::{KeyCode, KeyEvent};

// ── Model item for display ─────────────────────────────────────────

#[derive(Clone)]
struct ModelItem {
    provider: String,
    id: String,
    name: String,
    full_id: String,     // "provider/id"
    search_text: String, // pre-computed search string
}

impl ModelItem {
    fn new(provider: String, id: String, name: String) -> Self {
        let full_id = format!("{}/{}", provider, id);
        let search_text = format!("{} {} {} {}", provider, id, name, full_id);
        Self {
            provider,
            id,
            name,
            full_id,
            search_text,
        }
    }

    fn search_text(&self) -> &str {
        &self.search_text
    }
}

// ── Visibility style ───────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum ModelScope {
    All,
    Scoped,
}

// ── ModelSelector component ────────────────────────────────────────

pub struct ModelSelector {
    all_models: Vec<ModelItem>,
    scoped_model_ids: Vec<String>, // "provider/id" strings
    scope: ModelScope,
    active_items: Vec<ModelItem>,
    filtered_indices: Vec<usize>,
    selected_index: usize,
    search_query: String,
    current_model: String,
    max_visible: usize,
    callbacks: ModelSelectorCallbacks,
}

pub struct ModelSelectorCallbacks {
    /// Called when user selects a model (receives full "provider/id" string).
    pub on_select: Box<dyn Fn(String)>,
    /// Called when user cancels.
    pub on_cancel: Box<dyn Fn()>,
}

impl ModelSelector {
    pub fn new(
        all_models: Vec<(String, String, String)>, // (provider, id, name)
        scoped_model_ids: Vec<String>,             // "provider/id" strings
        current_model: String,
        callbacks: ModelSelectorCallbacks,
    ) -> Self {
        let mut items: Vec<ModelItem> = all_models
            .into_iter()
            .map(|(p, id, name)| ModelItem::new(p, id, name))
            .collect();

        // Deduplicate by full_id (provider/id) — the model registry may list
        // the same model ID under multiple providers, but provider_for_model
        // can resolve all of them to the same provider, creating true duplicates.
        let mut seen = std::collections::HashSet::new();
        items.retain(|item| seen.insert(item.full_id.clone()));

        // Sort: current model first, then by provider (matches pi's sortModels)
        items.sort_by(|a, b| {
            let a_is_current = a.full_id == current_model;
            let b_is_current = b.full_id == current_model;
            if a_is_current && !b_is_current {
                return std::cmp::Ordering::Less;
            }
            if !a_is_current && b_is_current {
                return std::cmp::Ordering::Greater;
            }
            a.provider.cmp(&b.provider)
        });

        let has_scoped = !scoped_model_ids.is_empty();
        let scope = if has_scoped {
            ModelScope::Scoped
        } else {
            ModelScope::All
        };

        let active = if has_scoped {
            // Respect scoped model order: iterate scoped_model_ids, find matching item.
            let mut active: Vec<ModelItem> = Vec::new();
            for full_id in &scoped_model_ids {
                if let Some(item) = items.iter().find(|i| &i.full_id == full_id) {
                    active.push(item.clone());
                }
            }
            active
        } else {
            items.clone()
        };

        let current_idx = active
            .iter()
            .position(|m| m.full_id == current_model)
            .unwrap_or(0);
        let filtered: Vec<usize> = (0..active.len()).collect();

        Self {
            all_models: items,
            scoped_model_ids,
            scope,
            active_items: active,
            filtered_indices: filtered,
            selected_index: current_idx,
            search_query: String::new(),
            current_model,
            max_visible: 10,
            callbacks,
        }
    }

    fn set_scope(&mut self, scope: ModelScope) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        self.active_items = match scope {
            ModelScope::All => self.all_models.clone(),
            ModelScope::Scoped => {
                // Respect scoped model order
                let mut active: Vec<ModelItem> = Vec::new();
                for full_id in &self.scoped_model_ids {
                    if let Some(item) = self.all_models.iter().find(|i| &i.full_id == full_id) {
                        active.push(item.clone());
                    }
                }
                active
            }
        };
        let current_idx = self
            .active_items
            .iter()
            .position(|m| m.full_id == self.current_model)
            .unwrap_or(0);
        self.selected_index = current_idx;
        self.refresh();
    }

    fn refresh(&mut self) {
        let query = self.search_query.clone();
        self.filtered_indices = if query.trim().is_empty() {
            (0..self.active_items.len()).collect()
        } else {
            fuzzy_filter(&self.active_items, &query, |item| item.search_text())
        };
        self.selected_index = self
            .selected_index
            .min(self.filtered_indices.len().saturating_sub(1));
    }

    fn get_item(&self, filtered_idx: usize) -> Option<&ModelItem> {
        self.filtered_indices
            .get(filtered_idx)
            .and_then(|&idx| self.active_items.get(idx))
    }
}

impl Component for ModelSelector {
    fn render(&mut self, width: usize) -> Vec<String> {
        use crate::tui::util::truncate_to_width;
        let theme = current_theme();
        let mut lines: Vec<String> = Vec::new();

        // Top border (matches pi's DynamicBorder)
        lines.push(theme.dim(&"─".repeat(width)));
        lines.push(String::new());

        // Scope / hint (matches pi's scopeText + scopeHintText layout)
        let has_scoped = !self.scoped_model_ids.is_empty();
        if has_scoped {
            let all_text = match self.scope {
                ModelScope::All => theme.fg_key(ThemeKey::Accent, "all"),
                ModelScope::Scoped => theme.dim("all"),
            };
            let scoped_text = match self.scope {
                ModelScope::Scoped => theme.fg_key(ThemeKey::Accent, "scoped"),
                ModelScope::All => theme.dim("scoped"),
            };
            lines.push(format!(
                " {} {} | {}",
                theme.dim("Scope:"),
                all_text,
                scoped_text,
            ));
            lines.push(format!(" {}", theme.dim("Tab scope (all/scoped)")));
        } else {
            lines.push(format!(
                " {}",
                theme.fg_key(
                    ThemeKey::Warning,
                    "Only showing models from configured providers. Use /login to add providers."
                )
            ));
        }
        lines.push(String::new());

        // Search input line (matches pi's Input widget — displayed as single line)
        let search_value = if self.search_query.is_empty() {
            String::new()
        } else {
            self.search_query.clone()
        };
        lines.push(format!(" {}{}", theme.dim("Search: "), search_value));
        lines.push(String::new());

        // Model list
        let count = self.filtered_indices.len();
        if count == 0 {
            lines.push(theme.dim("  No matching models"));
        } else {
            let start = self
                .selected_index
                .saturating_sub(self.max_visible / 2)
                .min(count.saturating_sub(self.max_visible));
            let end = (start + self.max_visible).min(count);

            for i in start..end {
                let item = &self.active_items[self.filtered_indices[i]];
                let is_selected = i == self.selected_index;
                let is_current = item.full_id == self.current_model;

                let prefix = if is_selected {
                    theme.fg_key(ThemeKey::Accent, "→ ")
                } else {
                    "  ".to_string()
                };
                let model_text = if is_selected {
                    theme.fg_key(ThemeKey::Accent, &item.id)
                } else {
                    item.id.clone()
                };
                let provider_badge = theme.dim(&format!(" [{}]", item.provider));
                let checkmark = if is_current {
                    theme.fg_key(ThemeKey::Success, " ✓")
                } else {
                    String::new()
                };

                lines.push(truncate_to_width(
                    &format!("{}{}{}{}", prefix, model_text, provider_badge, checkmark),
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
                lines.push(theme.dim(&format!("  Model Name: {}", item.name)));
            }
        }

        // Bottom border
        lines.push(theme.dim(&"─".repeat(width)));

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        let kb = get_keybindings();

        // Tab toggles scope
        if kb.matches(key, ACTION_INPUT_TAB) {
            if !self.scoped_model_ids.is_empty() {
                let next = match self.scope {
                    ModelScope::All => ModelScope::Scoped,
                    ModelScope::Scoped => ModelScope::All,
                };
                self.set_scope(next);
            }
            return true;
        }

        // Up/Down navigation with wrapping
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

        // Enter selects model
        if kb.matches(key, ACTION_SELECT_CONFIRM) {
            if let Some(item) = self.get_item(self.selected_index) {
                (self.callbacks.on_select)(item.full_id.clone());
            }
            return true;
        }

        // Escape cancels - call callback then pop overlay via app
        if kb.matches(key, ACTION_SELECT_CANCEL) {
            (self.callbacks.on_cancel)();
            return false; // Let App's fallback pop the overlay
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
