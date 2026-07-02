use crate::agent::SessionRepo;
use crate::agent::session::SessionInfo;
use crate::tui::Theme;
use crate::tui::theme::ThemeKey;
use std::path::{Path, PathBuf};

/// Which group a session belongs to in the picker display.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionGroup {
    /// Session from the current project directory (cwd matches).
    CurrentProject,
    /// Session from another project directory.
    OtherProjects,
}

/// A session entry tagged with its group.
#[derive(Debug, Clone)]
struct GroupedSession {
    info: SessionInfo,
    group: SessionGroup,
}

/// Interactive session picker state.
/// Not a full TUI Component — emits a result for the app to act on.
pub struct SessionPicker {
    /// All loaded sessions with their group tag.
    sessions: Vec<GroupedSession>,
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
    /// Whether rename mode is active.
    rename_mode: bool,
    /// Rename input buffer.
    rename_buffer: String,
    /// Index (into self.sessions) of the session being renamed.
    rename_target: Option<usize>,
    /// Pending rename result (set when user submits in rename mode).
    pending_rename: Option<(PathBuf, String)>,
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
    /// Rename a session (path, new name).
    Rename(PathBuf, String),
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
            rename_mode: false,
            rename_buffer: String::new(),
            rename_target: None,
            pending_rename: None,
        }
    }

    /// Load sessions from disk without cwd grouping.
    pub fn load_sessions(&mut self, repo: &dyn SessionRepo) {
        self.load_sessions_with_cwd(repo, None, None);
    }

    /// Load sessions with cwd-based grouping.
    /// - `repo` — Session repo for loading
    /// - `cwd` — Current working directory (sessions with matching cwd are "Current project")
    /// - `session_dir` — Session directory for current project (if None, uses default)
    pub fn load_sessions_with_cwd(
        &mut self,
        repo: &dyn SessionRepo,
        cwd: Option<&Path>,
        session_dir: Option<&Path>,
    ) {
        self.loading = true;
        self.loaded_count = 0;
        self.total_count = 0;

        let mut all_sessions: Vec<GroupedSession> = Vec::new();

        // Load current-project sessions first
        if let Some(cwd) = cwd {
            let dir = session_dir
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| crate::agent::session::get_default_session_dir(cwd));
            let current_count = std::cell::Cell::new(0usize);
            let current_total = std::cell::Cell::new(0usize);

            let current_sessions = repo.list(
                &dir,
                Some(cwd),
                Some(&|l, t| {
                    current_count.set(l);
                    current_total.set(t);
                }),
            );

            self.loaded_count += current_count.get();
            self.total_count += current_total.get();

            for s in current_sessions {
                all_sessions.push(GroupedSession {
                    info: s,
                    group: SessionGroup::CurrentProject,
                });
            }
        }

        // Collect paths we already have to avoid duplication
        let existing_paths: std::collections::HashSet<PathBuf> =
            all_sessions.iter().map(|gs| gs.info.path.clone()).collect();

        let loaded = std::cell::Cell::new(0usize);
        let total = std::cell::Cell::new(0usize);

        let all_loaded = repo.list_all(Some(&|l, t| {
            loaded.set(l);
            total.set(t);
        }));

        self.loaded_count = loaded.get();
        self.total_count = total.get();

        for s in all_loaded {
            if !existing_paths.contains(&s.path) {
                let group = if let Some(cwd) = cwd {
                    if s.cwd == cwd.to_string_lossy().as_ref() {
                        SessionGroup::CurrentProject
                    } else {
                        SessionGroup::OtherProjects
                    }
                } else {
                    SessionGroup::OtherProjects
                };
                all_sessions.push(GroupedSession { info: s, group });
            }
        }

        // Sort: current project first, then other projects
        all_sessions.sort_by_key(|gs| match gs.group {
            SessionGroup::CurrentProject => 0,
            SessionGroup::OtherProjects => 1,
        });

        self.sessions = all_sessions;
        self.loading = false;
        self.selected = 0;
        self.rename_mode = false;
        self.rename_buffer.clear();
        self.rename_target = None;
        self.pending_rename = None;
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
        self.filtered
            .get(self.selected)
            .map(|&i| &self.sessions[i].info)
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

    /// Whether rename mode is active.
    pub fn is_rename_mode(&self) -> bool {
        self.rename_mode
    }

    /// Get the rename buffer.
    pub fn rename_buffer(&self) -> &str {
        &self.rename_buffer
    }

    /// Handle a character in rename mode. Returns true if the key was consumed.
    pub fn handle_rename_char(&mut self, c: char) -> bool {
        if !self.rename_mode {
            return false;
        }
        match c {
            '\n' | '\r' => {
                // Submit rename
                let name = self.rename_buffer.trim().to_string();
                if !name.is_empty()
                    && let Some(idx) = self.rename_target
                    && let Some(path) = self.sessions.get(idx).map(|gs| gs.info.path.clone())
                {
                    self.pending_rename = Some((path, name));
                }
                self.rename_mode = false;
                self.rename_buffer.clear();
                self.rename_target = None;
                true
            }
            '\x1b' => {
                // Cancel rename
                self.rename_mode = false;
                self.rename_buffer.clear();
                self.rename_target = None;
                true
            }
            '\x7f' | '\x08' => {
                // Backspace
                self.rename_buffer.pop();
                true
            }
            c if !c.is_control() => {
                self.rename_buffer.push(c);
                true
            }
            _ => true,
        }
    }

    /// Start renaming the currently selected session.
    pub fn start_rename(&mut self) {
        // We need to retrieve the name without holding a borrow
        let name = self.selected_info().and_then(|info| info.name.clone());
        let idx = self.filtered.get(self.selected).copied();
        self.rename_target = idx;
        self.rename_buffer = name.unwrap_or_default();
        self.rename_mode = true;
    }

    /// Cancel rename mode.
    pub fn cancel_rename(&mut self) {
        self.rename_mode = false;
        self.rename_buffer.clear();
        self.rename_target = None;
    }

    /// Take any pending rename result. Returns (path, new_name) if user submitted a rename.
    pub fn take_pending_rename(&mut self) -> Option<(PathBuf, String)> {
        self.pending_rename.take()
    }

    fn rebuild_filter(&mut self) {
        if self.filter.is_empty() {
            self.filtered = (0..self.sessions.len()).collect();
        } else {
            self.filtered = self
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, gs)| {
                    let s = &gs.info;
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
        let mut prev_group: Option<SessionGroup> = None;

        for (display_idx, &session_idx) in self.filtered.iter().enumerate() {
            let gs = &self.sessions[session_idx];
            let session = &gs.info;
            let is_selected = display_idx == self.selected;

            // Section header when group changes
            if prev_group.as_ref() != Some(&gs.group) {
                let section_title = match gs.group {
                    SessionGroup::CurrentProject => "Current Project",
                    SessionGroup::OtherProjects => "Other Projects",
                };
                lines.push(theme.bold(&theme.fg_key(ThemeKey::Accent, section_title)));
                prev_group = Some(gs.group.clone());
            }

            // In rename mode and this is the selected session: show rename input
            if self.rename_mode && is_selected {
                let display = if self.rename_buffer.is_empty() {
                    String::new()
                } else {
                    self.rename_buffer.clone()
                };
                let cursor = "\u{2588}"; // full block
                lines.push(format!(
                    "  {} {}",
                    theme.fg_key(ThemeKey::Accent, "Rename:"),
                    theme.fg_key(ThemeKey::Text, &format!("{} {}", display, cursor))
                ));
                cursor_y = lines.len() - 1;
                continue;
            }

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
        if self.rename_mode {
            lines.push(theme.fg_key(ThemeKey::Dim, "Enter: confirm rename · Esc: cancel"));
        } else {
            lines.push(theme.fg_key(
                ThemeKey::Dim,
                "↑↓ navigate · Enter select · / filter · r rename · Esc cancel",
            ));
        }

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
