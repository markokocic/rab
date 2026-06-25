use crate::agent::SessionRepo;
use crate::agent::session::SessionInfo;
use crate::tui::Theme;
use crate::tui::theme::ThemeKey;
use std::path::PathBuf;

/// Interactive session picker state.
/// Not a full TUI Component — emits a result for the app to act on.
pub struct SessionPicker {
    /// All loaded sessions.
    sessions: Vec<SessionInfo>,
    /// Current filter text (matched against name, id, cwd).
    filter: String,
    /// Selected index (in the filtered list).
    selected: usize,
    /// Filtered session indices (into self.sessions).
    filtered: Vec<usize>,
    /// Whether we're still loading.
    loading: bool,
    /// Loading progress.
    loaded_count: usize,
    total_count: usize,
}

#[derive(Debug, Clone)]
pub enum SessionPickerResult {
    /// Switch to the session at the given path.
    Select(PathBuf),
    /// Dismiss without selecting.
    Cancel,
    /// Show session info for the selected session.
    Info(PathBuf),
    /// Delete the selected session.
    Delete(PathBuf),
}

impl SessionPicker {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            filter: String::new(),
            selected: 0,
            filtered: Vec::new(),
            loading: true,
            loaded_count: 0,
            total_count: 0,
        }
    }

    /// Load sessions from disk (call from async context).
    pub fn load_sessions(&mut self, repo: &dyn SessionRepo) {
        self.loading = true;
        self.loaded_count = 0;
        self.total_count = 0;

        // Track progress via interior counters
        let loaded = std::cell::Cell::new(0usize);
        let total = std::cell::Cell::new(0usize);

        let sessions = repo.list_all(Some(&|l, t| {
            loaded.set(l);
            total.set(t);
        }));

        self.loaded_count = loaded.get();
        self.total_count = total.get();
        self.sessions = sessions;
        self.loading = false;
        self.selected = 0;
        self.rebuild_filter();
    }

    /// Set the filter string and rebuild the filtered list.
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_lowercase();
        self.rebuild_filter();
    }

    /// Get the current filter string.
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Move selection up.
    pub fn select_prev(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    /// Move selection down.
    pub fn select_next(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = std::cmp::min(self.selected + 1, self.filtered.len() - 1);
        }
    }

    /// Get the currently selected session info, if any.
    pub fn selected_info(&self) -> Option<&SessionInfo> {
        self.filtered.get(self.selected).map(|&i| &self.sessions[i])
    }

    /// Get the path of the selected session.
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.selected_info().map(|s| s.path.clone())
    }

    /// Whether the picker is still loading.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Loading progress.
    pub fn progress(&self) -> (usize, usize) {
        (self.loaded_count, self.total_count)
    }

    /// Whether there are any sessions matching the filter.
    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    /// Number of sessions matching the filter.
    pub fn len(&self) -> usize {
        self.filtered.len()
    }

    fn rebuild_filter(&mut self) {
        if self.filter.is_empty() {
            self.filtered = (0..self.sessions.len()).collect();
        } else {
            self.filtered = self
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    let name = s.name.as_deref().unwrap_or("").to_lowercase();
                    let cwd = s.cwd.to_lowercase();
                    let id = s.id.to_lowercase();
                    name.contains(&self.filter)
                        || cwd.contains(&self.filter)
                        || id.contains(&self.filter)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.selected = 0;
    }

    /// Render the session list into lines for display.
    /// Returns (lines, cursor_y) where cursor_y is the selected row.
    pub fn render(&self, _width: usize, theme: &dyn Theme) -> (Vec<String>, usize) {
        let mut lines = Vec::new();

        if self.loading {
            lines.push(theme.fg_key(
                ThemeKey::Dim,
                &format!(
                    "Loading sessions... ({}/{})",
                    self.loaded_count, self.total_count
                ),
            ));
            return (lines, 0);
        }

        if self.sessions.is_empty() {
            lines.push(theme.fg_key(ThemeKey::Dim, "No sessions found."));
            return (lines, 0);
        }

        // Header
        lines.push(theme.bold("Sessions"));
        lines.push(theme.fg_key(
            ThemeKey::Dim,
            &format!(
                "{} total, {} shown",
                self.sessions.len(),
                self.filtered.len()
            ),
        ));
        lines.push(String::new());

        let mut cursor_y = 0;

        for (display_idx, &session_idx) in self.filtered.iter().enumerate() {
            let session = &self.sessions[session_idx];
            let is_selected = display_idx == self.selected;

            let name = session.name.as_deref().unwrap_or("unnamed").to_string();
            let cwd_short = shorten_cwd(&session.cwd);

            let marker = if is_selected { "▸ " } else { "  " };
            let line = format!(
                "{}{}  {}  {}  ({} msgs)",
                marker,
                name,
                cwd_short,
                fmt_time(&session.created),
                session.message_count,
            );

            if is_selected {
                lines.push(theme.fg("accent", &line));
                cursor_y = lines.len() - 1;
            } else {
                lines.push(line);
            }
        }

        // Footer hint
        lines.push(String::new());
        lines.push(theme.fg_key(
            ThemeKey::Dim,
            "↑↓ navigate · Enter select · / filter · Esc cancel",
        ));

        (lines, cursor_y)
    }
}

impl Default for SessionPicker {
    fn default() -> Self {
        Self::new()
    }
}

fn shorten_cwd(cwd: &str) -> String {
    // Replace home dir with ~/
    let home = directories::BaseDirs::new()
        .map(|d| d.home_dir().to_string_lossy().to_string())
        .unwrap_or_default();
    if let Some(rest) = cwd.strip_prefix(&home) {
        format!("~{}", rest)
    } else {
        cwd.to_string()
    }
}

fn fmt_time(dt: &chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M").to_string()
}
