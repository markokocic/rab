use crate::agent::session::{
    SessionInfo, delete_session as delete_session_file, fork_session, load_session_info,
};
use std::path::{Path, PathBuf};

/// Session lifecycle management: create, open, list, delete, fork.
///
/// Default implementation uses JSONL files on disk.
pub trait SessionRepo {
    /// List sessions in a directory, optionally filtered by cwd.
    /// `progress` receives `(loaded_count, total_count)` for UI updates.
    fn list(
        &self,
        session_dir: &Path,
        filter_cwd: Option<&Path>,
        progress: Option<&dyn Fn(usize, usize)>,
    ) -> Vec<SessionInfo>;

    /// List sessions across all project directories under `~/.rab/sessions/`.
    fn list_all(&self, progress: Option<&dyn Fn(usize, usize)>) -> Vec<SessionInfo>;

    /// Delete a session file.
    fn delete(&self, path: &Path) -> std::io::Result<()>;

    /// Fork a session: create a new session file containing entries up to (and including)
    /// the given entry_id, or all entries if entry_id is None.
    fn fork(
        &self,
        source_path: &Path,
        target_dir: &Path,
        entry_id: Option<&str>,
        position: Option<&str>,
    ) -> std::io::Result<String>;

    /// Load metadata for a single session file.
    fn load_info(&self, path: &Path) -> Option<SessionInfo>;
}

// ── Default JSONL-based repo ───────────────────────────────────────

/// Default session repo backed by JSONL files.
pub struct DefaultSessionRepo;

impl Default for DefaultSessionRepo {
    fn default() -> Self {
        Self
    }
}

impl DefaultSessionRepo {
    pub fn new() -> Self {
        Self
    }
}

impl SessionRepo for DefaultSessionRepo {
    fn list(
        &self,
        session_dir: &Path,
        filter_cwd: Option<&Path>,
        progress: Option<&dyn Fn(usize, usize)>,
    ) -> Vec<SessionInfo> {
        list_sessions_with_progress(session_dir, filter_cwd, progress, 1)
    }

    fn list_all(&self, progress: Option<&dyn Fn(usize, usize)>) -> Vec<SessionInfo> {
        let dir = directories::BaseDirs::new()
            .map(|d| d.home_dir().join(".rab").join("sessions"))
            .unwrap_or_else(|| PathBuf::from("/tmp/.rab/sessions"));

        let mut all_sessions: Vec<SessionInfo> = Vec::new();

        // Collect all session dirs + root
        let mut dirs = vec![dir.clone()];
        if let Ok(read_dir) = std::fs::read_dir(&dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                }
            }
        }

        let total_dirs = dirs.len();
        let mut loaded = 0;

        for session_dir in &dirs {
            let sessions = list_sessions_with_progress(session_dir, None, progress, 1);
            loaded += 1;
            if let Some(ref cb) = progress {
                cb(loaded, total_dirs);
            }
            all_sessions.extend(sessions);
        }

        all_sessions.sort_by_key(|b| std::cmp::Reverse(b.created));
        all_sessions
    }

    fn delete(&self, path: &Path) -> std::io::Result<()> {
        delete_session_file(path)
    }

    fn fork(
        &self,
        source_path: &Path,
        target_dir: &Path,
        entry_id: Option<&str>,
        position: Option<&str>,
    ) -> std::io::Result<String> {
        fork_session(source_path, target_dir, entry_id, position)
    }

    fn load_info(&self, path: &Path) -> Option<SessionInfo> {
        load_session_info(path)
    }
}

// ── Progress-aware listing ─────────────────────────────────────────

/// List session files in a directory with optional cwd filtering and progress callback.
/// Uses concurrent loading for better performance on large directories.
fn list_sessions_with_progress(
    session_dir: &Path,
    filter_cwd: Option<&Path>,
    progress: Option<&dyn Fn(usize, usize)>,
    _concurrency: usize,
) -> Vec<SessionInfo> {
    let dir = match std::fs::read_dir(session_dir) {
        Ok(d) => d,
        Err(_) => return vec![],
    };

    // Collect all jsonl file paths first
    let file_paths: Vec<PathBuf> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .map(|e| e.path())
        .collect();

    let total = file_paths.len();
    let mut sessions: Vec<SessionInfo> = Vec::with_capacity(total);
    let mut loaded = 0;

    // Sequential loading for now; concurrent loading can be added later
    // using rayon or tokio::task::spawn_blocking.
    for path in &file_paths {
        // Parse header first for cwd filtering (cheap)
        let header = crate::agent::session::read_session_header(path);
        if let Some(ref h) = header
            && let Some(filter) = filter_cwd
            && h.cwd != filter.to_string_lossy().as_ref()
        {
            loaded += 1;
            if let Some(ref cb) = progress {
                cb(loaded, total);
            }
            continue;
        }

        // Load full session info
        if let Some(info) = load_session_info(path) {
            sessions.push(info);
        }
        loaded += 1;
        if let Some(ref cb) = progress {
            cb(loaded, total);
        }
    }

    sessions.sort_by_key(|b| std::cmp::Reverse(b.created));
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_list_empty_dir() {
        let repo = DefaultSessionRepo::new();
        let tmp = TempDir::new().unwrap();
        let sessions = repo.list(tmp.path(), None, None);
        assert!(sessions.is_empty());
    }
}
