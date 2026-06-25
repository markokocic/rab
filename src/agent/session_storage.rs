use crate::agent::session::{SessionEntry, SessionHeader};
use std::path::{Path, PathBuf};

/// Low-level storage abstraction for session persistence.
///
/// `SessionManager` manages all state in memory and calls into this trait
/// only when reading from or writing to permanent storage.
pub trait SessionStorage: Send {
    /// Load the session header and all entries from storage.
    fn load(&self) -> (Option<SessionHeader>, Vec<SessionEntry>);

    /// Append a single entry to the storage.
    fn append(&self, entry: &SessionEntry) -> std::io::Result<()>;

    /// Atomically write the full header and all entries, overwriting existing data.
    fn write_full(&self, header: &SessionHeader, entries: &[SessionEntry]) -> std::io::Result<()>;

    /// The file path on disk, if this storage is file-backed.
    fn path(&self) -> Option<&Path>;

    /// Whether the backing file exists on disk.
    fn exists(&self) -> bool;
}

// ── JSONL file-backed storage ──────────────────────────────────────

/// Persists session data to a JSONL file on disk.
pub struct JsonlSessionStorage {
    path: PathBuf,
}

impl JsonlSessionStorage {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path_ref(&self) -> &Path {
        &self.path
    }
}

impl SessionStorage for JsonlSessionStorage {
    fn load(&self) -> (Option<SessionHeader>, Vec<SessionEntry>) {
        crate::agent::session::load_session_from_file(&self.path)
    }

    fn append(&self, entry: &SessionEntry) -> std::io::Result<()> {
        crate::agent::session::append_entry_to_file(&self.path, entry)
    }

    fn write_full(&self, header: &SessionHeader, entries: &[SessionEntry]) -> std::io::Result<()> {
        crate::agent::session::write_entries_to_file(&self.path, header, entries)
    }

    fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }

    fn exists(&self) -> bool {
        self.path.exists()
    }
}

// ── In-memory storage ──────────────────────────────────────────────

/// No-op storage that discards all writes and never reads from disk.
/// Used for `--no-session` mode.
pub struct InMemorySessionStorage;

impl Default for InMemorySessionStorage {
    fn default() -> Self {
        Self
    }
}

impl InMemorySessionStorage {
    pub fn new() -> Self {
        Self
    }
}

impl SessionStorage for InMemorySessionStorage {
    fn load(&self) -> (Option<SessionHeader>, Vec<SessionEntry>) {
        (None, vec![])
    }

    fn append(&self, _entry: &SessionEntry) -> std::io::Result<()> {
        Ok(())
    }

    fn write_full(
        &self,
        _header: &SessionHeader,
        _entries: &[SessionEntry],
    ) -> std::io::Result<()> {
        Ok(())
    }

    fn path(&self) -> Option<&Path> {
        None
    }

    fn exists(&self) -> bool {
        true // Always "exists" in the sense that there's no broken file to recover
    }
}
