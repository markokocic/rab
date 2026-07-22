//! Session resolution helpers for CLI flags --session, --fork, --resume.
//!
//! Matches pi's `packages/coding-agent/src/cli/session-picker.ts` and parts of
//! `packages/coding-agent/src/core/session-manager.ts`.

use std::path::PathBuf;

/// A resolved session reference from a CLI argument.
pub enum ResolvedSession {
    /// Direct file path.
    Path(PathBuf),
    /// Found by ID lookup (carries the session's original cwd).
    Found { path: PathBuf, cwd: String },
}

impl ResolvedSession {
    pub fn path(&self) -> &std::path::Path {
        match self {
            ResolvedSession::Path(p) => p.as_path(),
            ResolvedSession::Found { path, .. } => path.as_path(),
        }
    }

    pub fn cwd(&self) -> Option<&str> {
        match self {
            ResolvedSession::Path(_) => None,
            ResolvedSession::Found { cwd, .. } => Some(cwd.as_str()),
        }
    }
}

/// Resolve a session argument (path or partial ID) for --session and --fork.
pub fn resolve_session_arg(
    arg: &str,
    cwd: &std::path::Path,
    session_dir: Option<&std::path::Path>,
) -> Result<ResolvedSession, String> {
    // If it looks like a path (contains separator or ends with .jsonl), use as-is
    if arg.contains('/') || arg.contains('\\') || arg.ends_with(".jsonl") {
        let path = PathBuf::from(arg);
        if path.is_absolute() {
            return Ok(ResolvedSession::Path(path));
        }
        return Ok(ResolvedSession::Path(cwd.join(&path)));
    }

    // Try to match as session ID prefix (first exact, then prefix)
    let session_dir = session_dir
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| crate::agent::session::get_default_session_dir(cwd));
    let sessions = crate::agent::session::list_sessions(&session_dir);

    // Exact match first
    if let Some(s) = sessions.iter().find(|s| s.id == arg) {
        return Ok(ResolvedSession::Found {
            path: s.path.clone(),
            cwd: s.cwd.clone(),
        });
    }

    // Prefix match
    let matches: Vec<_> = sessions.iter().filter(|s| s.id.starts_with(arg)).collect();
    if matches.len() == 1 {
        return Ok(ResolvedSession::Found {
            path: matches[0].path.clone(),
            cwd: matches[0].cwd.clone(),
        });
    }

    Err(format!("No session found matching '{}'", arg))
}
