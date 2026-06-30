use crate::agent::session::{
    SessionInfo, delete_session as delete_session_file, fork_session, load_session_info,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

/// Maximum number of concurrent session file loads (pi-compatible).
const MAX_CONCURRENT_LOADS: usize = 10;

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
        list_sessions(session_dir, filter_cwd, progress)
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
            let sessions = list_sessions_concurrent(session_dir, None);
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

// ── Sequential listing (used by `list`) ────────────────────────────

/// List session files sequentially with optional cwd filtering and progress callback.
/// Uses the public `list_sessions` from `session.rs` for the core listing.
fn list_sessions(
    session_dir: &Path,
    filter_cwd: Option<&Path>,
    progress: Option<&dyn Fn(usize, usize)>,
) -> Vec<SessionInfo> {
    let sessions = crate::agent::session::list_sessions(session_dir);
    let total = sessions.len();
    let mut loaded = 0;

    let filtered: Vec<SessionInfo> = sessions
        .into_iter()
        .filter(|s| {
            loaded += 1;
            if let Some(ref cb) = progress {
                cb(loaded, total);
            }
            if let Some(filter) = filter_cwd {
                s.cwd == filter.to_string_lossy().as_ref()
            } else {
                true
            }
        })
        .collect();

    filtered
}

// ── Concurrent listing (used by `list_all`) ────────────────────────

/// List session files with concurrent loading (pi-compatible: up to 10 workers).
/// Uses a channel to collect results; the calling thread gathers them.
fn list_sessions_concurrent(session_dir: &Path, filter_cwd: Option<&Path>) -> Vec<SessionInfo> {
    let dir = match std::fs::read_dir(session_dir) {
        Ok(d) => d,
        Err(_) => return vec![],
    };

    let file_paths: Vec<PathBuf> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .map(|e| e.path())
        .collect();

    let total = file_paths.len();
    if total == 0 {
        return vec![];
    }

    // For a single file, avoid threading overhead
    if total == 1 {
        let mut sessions = Vec::new();
        if let Some(info) = load_session_info(&file_paths[0]) {
            sessions.push(info);
        }
        return sessions;
    }

    let (tx, rx) = mpsc::channel::<Option<SessionInfo>>();
    let next_index = Arc::new(AtomicUsize::new(0));
    let filter_cwd_owned = Arc::new(filter_cwd.map(|p| p.to_path_buf()));
    let file_paths = Arc::new(file_paths);

    let worker_count = MAX_CONCURRENT_LOADS.min(total);

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let tx = tx.clone();
            let next_index = Arc::clone(&next_index);
            let filter_cwd_owned = Arc::clone(&filter_cwd_owned);
            let file_paths = Arc::clone(&file_paths);

            scope.spawn(move || {
                loop {
                    let idx = next_index.fetch_add(1, Ordering::Relaxed);
                    if idx >= total {
                        break;
                    }

                    let path = &file_paths[idx];

                    // Quick cwd filter check
                    let header = crate::agent::session::read_session_header(path);
                    if let Some(ref h) = header
                        && let Some(ref filter) = *filter_cwd_owned
                        && h.cwd != filter.to_string_lossy().as_ref()
                    {
                        let _ = tx.send(None);
                        continue;
                    }

                    let info = load_session_info(path);
                    let _ = tx.send(info);
                }
            });
        }
        // Drop the original tx so rx doesn't block forever
        drop(tx);
    });

    let mut sessions: Vec<SessionInfo> = Vec::with_capacity(total);
    for info in rx.into_iter().flatten() {
        sessions.push(info);
    }

    sessions.sort_by_key(|b| std::cmp::Reverse(b.created));
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::session::SessionManager;
    use crate::agent::types::{assistant_message, user_message};
    use tempfile::TempDir;

    fn make_user_msg(content: &str) -> yoagent::types::AgentMessage {
        user_message(content)
    }

    fn make_asst_msg(content: &str) -> yoagent::types::AgentMessage {
        assistant_message(content)
    }

    #[test]
    fn test_list_empty_dir() {
        let repo = DefaultSessionRepo::new();
        let tmp = TempDir::new().unwrap();
        let sessions = repo.list(tmp.path(), None, None);
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_list_sessions_concurrent_with_files() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        // Create a few session files
        for i in 0..3 {
            let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
            sm.append_message(&make_user_msg(&format!("msg {}", i)));
            sm.append_message(&make_asst_msg(&format!("response {}", i)));
            drop(sm);
        }

        let sessions = list_sessions_concurrent(&sessions_dir, None);
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn test_list_sessions_concurrent_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let sessions = list_sessions_concurrent(tmp.path(), None);
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_list_sessions_concurrent_single_file() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_user_msg("only"));
        sm.append_message(&make_asst_msg("one"));
        drop(sm);

        let sessions = list_sessions_concurrent(&sessions_dir, None);
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn test_list_sessions_concurrent_filter_cwd() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd1 = tmp.path().join("project1");
        let cwd2 = tmp.path().join("project2");
        std::fs::create_dir_all(&cwd1).unwrap();
        std::fs::create_dir_all(&cwd2).unwrap();

        // Session in project1
        let mut sm1 = SessionManager::create(&cwd1, Some(&sessions_dir));
        sm1.append_message(&make_user_msg("p1 msg"));
        sm1.append_message(&make_asst_msg("p1 resp"));
        let _id1 = sm1.session().session_id().to_string();
        drop(sm1);

        // Session in project2
        let mut sm2 = SessionManager::create(&cwd2, Some(&sessions_dir));
        sm2.append_message(&make_user_msg("p2 msg"));
        sm2.append_message(&make_asst_msg("p2 resp"));
        drop(sm2);

        // Filter by project1
        let sessions = list_sessions_concurrent(&sessions_dir, Some(&cwd1));
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].cwd.ends_with("project1"));
    }
}
