//! OAuthSelector component — matching pi's OAuthSelectorComponent.
//!
//! Provider selector with search for login/logout flows.
//! Shows provider name, ID, and current auth status (configured, env, etc.).
//! Supports fuzzy filtering via search input.

use crate::agent::ui::theme::color;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keybindings::{
    ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM, ACTION_SELECT_DOWN, ACTION_SELECT_UP,
    get_keybindings,
};
use crossterm::event::{KeyCode, KeyEvent};

// ── Provider item types ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AuthType {
    OAuth,
    ApiKey,
}

impl AuthType {
    fn as_str(&self) -> &'static str {
        match self {
            AuthType::OAuth => "oauth",
            AuthType::ApiKey => "api_key",
        }
    }
}

/// A provider option shown in the selector (matching pi's AuthSelectorProvider).
#[derive(Debug, Clone)]
pub struct AuthSelectorProvider {
    pub id: String,
    pub name: String,
    pub auth_type: AuthType,
}

/// Login or logout mode for the selector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SelectorMode {
    Login,
    Logout,
}

// ── Internal provider item ─────────────────────────────────────────

#[derive(Clone)]
struct ProviderItem {
    id: String,
    name: String,
    /// Whether the provider has matching credentials stored.
    has_stored: bool,
    /// Whether some other auth is available (env var, runtime, etc.)
    has_other_auth: bool,
    /// Label for the auth status, e.g. "configured", "env: API_KEY", "unconfigured"
    status_label: String,
    /// Pre-computed search text.
    search_text: String,
}

impl ProviderItem {
    fn search_text(&self) -> &str {
        &self.search_text
    }
}

// ── Auth status function ───────────────────────────────────────────

/// Status information for a provider (matching pi's AuthStatus interface).
pub struct ProviderAuthStatus {
    pub configured: bool,
    pub source: Option<String>,
    pub label: Option<String>,
}

// ── OAuthSelector component ────────────────────────────────────────

/// Provider selector with search — matching pi's OAuthSelectorComponent.
pub struct OAuthSelector {
    mode: SelectorMode,
    items: Vec<ProviderItem>,
    filtered_indices: Vec<usize>,
    selected_index: usize,
    search_query: String,
    max_visible: usize,
    /// Callbacks
    on_select: Option<Box<dyn FnOnce(String)>>, // provider ID
    on_cancel: Option<Box<dyn FnOnce()>>,
}

impl OAuthSelector {
    /// Create a new OAuth selector.
    ///
    /// `providers` — list of providers available for login/logout.
    /// `auth_status` — function returning auth status for each provider.
    /// `mode` — login or logout mode.
    pub fn new(
        providers: Vec<AuthSelectorProvider>,
        auth_status: impl Fn(&str) -> ProviderAuthStatus,
        mode: SelectorMode,
    ) -> Self {
        let items: Vec<ProviderItem> = providers
            .into_iter()
            .map(|p| {
                let status = auth_status(&p.id);
                let has_stored =
                    status.configured && matches!(status.source.as_deref(), Some("stored"));
                let has_other_auth = status.configured && !has_stored;
                let status_label = if has_stored {
                    "configured".to_string()
                } else if let Some(label) = status.label {
                    format!("env: {}", label)
                } else if status.configured {
                    "configured".to_string()
                } else {
                    "unconfigured".to_string()
                };
                let id = p.id;
                let name = p.name;
                let auth_type = p.auth_type;
                let search_text =
                    format!("{} {} {} {}", id, name, auth_type.as_str(), status_label);
                ProviderItem {
                    id,
                    name,
                    has_stored,
                    has_other_auth,
                    status_label,
                    search_text,
                }
            })
            .collect();

        // Sort: stored first, then other auth, then alphabetically
        let mut sorted = items;
        sorted.sort_by(|a, b| {
            let a_priority = if a.has_stored {
                0
            } else if a.has_other_auth {
                1
            } else {
                2
            };
            let b_priority = if b.has_stored {
                0
            } else if b.has_other_auth {
                1
            } else {
                2
            };
            a_priority.cmp(&b_priority).then(a.name.cmp(&b.name))
        });

        let filtered: Vec<usize> = (0..sorted.len()).collect();

        Self {
            mode,
            items: sorted,
            filtered_indices: filtered,
            selected_index: 0,
            search_query: String::new(),
            max_visible: 10,
            on_select: None,
            on_cancel: None,
        }
    }

    /// Set the callback for when a provider is selected.
    pub fn on_select<F>(&mut self, f: F)
    where
        F: FnOnce(String) + 'static,
    {
        self.on_select = Some(Box::new(f));
    }

    /// Set the callback for when the user cancels.
    pub fn on_cancel<F>(&mut self, f: F)
    where
        F: FnOnce() + 'static,
    {
        self.on_cancel = Some(Box::new(f));
    }

    fn refresh(&mut self) {
        let query = self.search_query.clone();
        self.filtered_indices = if query.trim().is_empty() {
            (0..self.items.len()).collect()
        } else {
            fuzzy_filter(&self.items, &query, |item| item.search_text())
        };
        self.selected_index = self
            .selected_index
            .min(self.filtered_indices.len().saturating_sub(1));
    }

    fn get_item(&self, filtered_idx: usize) -> Option<&ProviderItem> {
        self.filtered_indices
            .get(filtered_idx)
            .and_then(|&idx| self.items.get(idx))
    }
}

impl Component for OAuthSelector {
    fn render(&mut self, width: usize) -> Vec<String> {
        use crate::tui::util::truncate_to_width;
        let theme = current_theme();
        let mut lines: Vec<String> = Vec::new();

        // Top border (matches pi's DynamicBorder)
        lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));
        lines.push(String::new());

        // Title (matches pi's TruncatedText with theme.fg("accent", theme.bold(title)))
        let title = match self.mode {
            SelectorMode::Login => "Select provider to configure:",
            SelectorMode::Logout => "Select provider to logout:",
        };
        lines.push(format!("  {}", theme.bold(&theme.fg(color::Accent, title))));
        lines.push(String::new());

        // Search input line
        let search_value = if self.search_query.is_empty() {
            String::new()
        } else {
            self.search_query.clone()
        };
        lines.push(format!(" {}{}", theme.dim("Search: "), search_value));
        lines.push(String::new());

        // Provider list
        let count = self.filtered_indices.len();
        if count == 0 {
            let msg = if self.items.is_empty() {
                match self.mode {
                    SelectorMode::Login => "No providers available",
                    SelectorMode::Logout => "No providers logged in. Use /login first.",
                }
            } else {
                "No matching providers"
            };
            lines.push(theme.dim(&format!("  {}", msg)));
        } else {
            let start = self
                .selected_index
                .saturating_sub(self.max_visible / 2)
                .min(count.saturating_sub(self.max_visible));
            let end = (start + self.max_visible).min(count);

            for i in start..end {
                let item = &self.items[self.filtered_indices[i]];
                let is_selected = i == self.selected_index;

                let prefix = if is_selected {
                    theme.fg(color::Accent, "→ ")
                } else {
                    "  ".to_string()
                };
                let name_text = if is_selected {
                    theme.fg(color::Accent, &item.name)
                } else {
                    theme.fg(color::Text, &item.name)
                };

                // Status indicator (matching pi's formatStatusIndicator)
                let status = if item.has_stored {
                    // Exact credential match (credential type == provider auth type)
                    theme.success(" ✓ configured")
                } else if item.has_other_auth {
                    // Env var or other non-stored auth source
                    theme.success(&format!(" ✓ {}", item.status_label))
                } else {
                    theme.dim(" • unconfigured")
                };

                lines.push(truncate_to_width(
                    &format!("{}{}{}", prefix, name_text, status),
                    width.saturating_sub(4),
                    "",
                    false,
                ));
            }

            // Scroll indicator
            if count > self.max_visible {
                lines.push(theme.dim(&format!("  ({}/{})", self.selected_index + 1, count)));
            }
        }

        // Hints
        lines.push(String::new());
        lines.push(format!(
            "  {}",
            theme.dim("Enter: select · Esc: cancel · Type to search")
        ));
        lines.push(String::new());

        // Bottom border
        lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        let kb = get_keybindings();

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

        // Enter selects provider
        if kb.matches(key, ACTION_SELECT_CONFIRM) {
            let selected_id = self
                .get_item(self.selected_index)
                .map(|item| item.id.clone());
            if let Some(id) = selected_id
                && let Some(cb) = self.on_select.take()
            {
                cb(id);
            }
            return true;
        }

        // Escape cancels
        if kb.matches(key, ACTION_SELECT_CANCEL) {
            if let Some(cb) = self.on_cancel.take() {
                cb();
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
