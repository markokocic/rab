//! Low-level session persistence abstraction — Pi-compatible `SessionStorage`.
//!
//! Pi architecture:
//!   SessionStorage (trait) ← InMemorySessionStorage / JsonlSessionStorage
//!   Session (struct)  ← wraps SessionStorage, provides high-level API
//!   AgentHarness     ← owns Session, drives agent loop
//!
//! This module provides the trait and both implementations.
//! The `Session` struct lives in `session.rs`.

use crate::agent::session::{
    LeafEntry, SessionEntry, SessionHeader, append_entry_to_file, generate_entry_id,
    load_session_from_file,
};
use std::path::{Path, PathBuf};

// ── SessionMetadata ────────────────────────────────────────────────

/// Metadata about a session, derived from the session header.
/// Pi-compatible: wraps header info into a metadata object.
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub id: String,
    pub created_at: String,
    pub cwd: String,
    /// File path on disk, if this is a persisted session.
    pub path: Option<PathBuf>,
    /// Path to the parent session if this was forked.
    pub parent_session_path: Option<String>,
}

// ── SessionStorage trait ───────────────────────────────────────────

/// Low-level CRUD abstraction for session persistence.
///
/// Pi-compatible: provides leaf management, label tracking, path queries,
/// and entry CRUD. `Session` builds on this for the high-level API.
pub trait SessionStorage: Send {
    /// Return header-derived metadata.
    fn metadata(&self) -> SessionMetadata;

    /// Get the current leaf entry ID (the last non-leaf entry, resolved through leaf entries).
    /// Returns `None` if no entries exist.
    fn get_leaf_id(&self) -> Option<String>;

    /// Persist a leaf entry that records the active session-tree leaf.
    /// `None` means reset to no leaf.
    fn set_leaf_id(&mut self, leaf_id: Option<&str>) -> Result<(), String>;

    /// Generate a unique 8-character hex entry ID, collision-checked.
    fn create_entry_id(&self) -> String;

    /// Append a fully-constructed entry. Updates in-memory state and persists to disk.
    fn append_entry(&mut self, entry: SessionEntry) -> Result<(), String>;

    /// Look up an entry by ID.
    fn get_entry(&self, id: &str) -> Option<SessionEntry>;

    /// Find all entries of the given `type` string.
    fn find_entries(&self, type_name: &str) -> Vec<SessionEntry>;

    /// Get the human-readable label for an entry, if any.
    fn get_label(&self, id: &str) -> Option<String>;

    /// Walk from `leaf_id` (or current leaf, if None) to root, returning entries in path order.
    fn get_path_to_root(&self, leaf_id: Option<&str>) -> Result<Vec<SessionEntry>, String>;

    /// Return all entries in insertion order.
    fn get_entries(&self) -> Vec<SessionEntry>;

    /// The file path on disk, if this storage is file-backed.
    fn path(&self) -> Option<&Path>;
}

// ── Helpers shared by both implementations ─────────────────────────

/// Given an entry, return the effective leaf ID after it.
/// For `Leaf` entries, returns `targetId`; for all others, returns `entry.id`.
fn leaf_id_after_entry(entry: &SessionEntry) -> Option<String> {
    match entry {
        SessionEntry::Leaf(e) => e.target_id.clone(),
        _ => Some(entry.id().to_string()),
    }
}

/// Update the label cache from an entry (call after every append).
fn update_label_cache(
    labels_by_id: &mut std::collections::HashMap<String, String>,
    entry: &SessionEntry,
) {
    if let SessionEntry::Label(e) = entry {
        if let Some(label) = &e.label {
            let trimmed = label.trim();
            if trimmed.is_empty() {
                labels_by_id.remove(&e.target_id);
            } else {
                labels_by_id.insert(e.target_id.clone(), trimmed.to_string());
            }
        } else {
            labels_by_id.remove(&e.target_id);
        }
    }
}

/// Build a label cache from a slice of entries.
fn build_labels_by_id(entries: &[SessionEntry]) -> std::collections::HashMap<String, String> {
    let mut labels = std::collections::HashMap::new();
    for entry in entries {
        update_label_cache(&mut labels, entry);
    }
    labels
}

// ── InMemorySessionStorage ─────────────────────────────────────────

/// Fully in-memory storage — no file I/O.
/// Pi-compatible: owns all state (entries, labels, leaf).
pub struct InMemorySessionStorage {
    metadata: SessionMetadata,
    entries: Vec<SessionEntry>,
    by_id: std::collections::HashMap<String, SessionEntry>,
    labels_by_id: std::collections::HashMap<String, String>,
    leaf_id: Option<String>,
}

impl InMemorySessionStorage {
    /// Create empty storage with explicit metadata.
    pub fn new(metadata: SessionMetadata) -> Self {
        Self {
            metadata,
            entries: Vec::new(),
            by_id: std::collections::HashMap::new(),
            labels_by_id: std::collections::HashMap::new(),
            leaf_id: None,
        }
    }
}

impl SessionStorage for InMemorySessionStorage {
    fn metadata(&self) -> SessionMetadata {
        self.metadata.clone()
    }

    fn get_leaf_id(&self) -> Option<String> {
        self.leaf_id.clone()
    }

    fn set_leaf_id(&mut self, leaf_id: Option<&str>) -> Result<(), String> {
        if let Some(id) = leaf_id
            && !self.by_id.contains_key(id)
        {
            return Err(format!("Entry {} not found", id));
        }
        let entry = SessionEntry::Leaf(LeafEntry {
            id: self.create_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            target_id: leaf_id.map(|s| s.to_string()),
        });
        self.leaf_id = leaf_id.map(|s| s.to_string());
        self.entries.push(entry.clone());
        self.by_id.insert(entry.id().to_string(), entry);
        Ok(())
    }

    fn create_entry_id(&self) -> String {
        generate_entry_id(&self.by_id)
    }

    fn append_entry(&mut self, entry: SessionEntry) -> Result<(), String> {
        let id = entry.id().to_string();
        self.by_id.insert(id.clone(), entry);
        self.entries
            .push(self.by_id.get(&id).expect("just inserted").clone());
        self.leaf_id = leaf_id_after_entry(self.by_id.get(&id).expect("just inserted"));
        update_label_cache(
            &mut self.labels_by_id,
            self.by_id.get(&id).expect("just inserted"),
        );
        Ok(())
    }

    fn get_entry(&self, id: &str) -> Option<SessionEntry> {
        self.by_id.get(id).cloned()
    }

    fn find_entries(&self, type_name: &str) -> Vec<SessionEntry> {
        self.entries
            .iter()
            .filter(|e| entry_type_name(e) == type_name)
            .cloned()
            .collect()
    }

    fn get_label(&self, id: &str) -> Option<String> {
        self.labels_by_id.get(id).cloned()
    }

    fn get_path_to_root(&self, leaf_id: Option<&str>) -> Result<Vec<SessionEntry>, String> {
        let start_id = leaf_id.or(self.leaf_id.as_deref());
        if start_id.is_none() {
            return Ok(vec![]);
        }
        let sid = start_id.unwrap();
        let mut path: Vec<SessionEntry> = Vec::new();
        let mut current = self.by_id.get(sid);
        if current.is_none() {
            return Err(format!("Entry {} not found", sid));
        }
        while let Some(entry) = current {
            path.push(entry.clone());
            match entry.parent_id() {
                Some(pid) => {
                    current = self.by_id.get(pid);
                }
                None => break,
            }
        }
        path.reverse();
        Ok(path)
    }

    fn get_entries(&self) -> Vec<SessionEntry> {
        self.entries.clone()
    }

    fn path(&self) -> Option<&Path> {
        None
    }
}

// ── JsonlSessionStorage ────────────────────────────────────────────

/// File-backed storage: holds full state in memory and persists to a JSONL file.
/// Pi-compatible: loads from file on creation, appends on every write.
pub struct JsonlSessionStorage {
    metadata: SessionMetadata,
    file_path: PathBuf,
    entries: Vec<SessionEntry>,
    by_id: std::collections::HashMap<String, SessionEntry>,
    labels_by_id: std::collections::HashMap<String, String>,
    leaf_id: Option<String>,
}

impl JsonlSessionStorage {
    /// Create a new session at the given path. Writes the header.
    pub fn create(
        file_path: PathBuf,
        cwd: &str,
        session_id: &str,
        parent_session_path: Option<String>,
    ) -> Result<Self, String> {
        let created_at = chrono::Utc::now().to_rfc3339();
        let header = SessionHeader {
            type_: "session".to_string(),
            version: Some(crate::agent::session::CURRENT_SESSION_VERSION),
            id: session_id.to_string(),
            timestamp: created_at.clone(),
            cwd: cwd.to_string(),
            parent_session: parent_session_path.clone(),
        };

        // Ensure parent directory exists
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create session directory: {}", e))?;
        }

        // Write header
        let header_json = serde_json::to_string(&header)
            .map_err(|e| format!("Failed to serialize header: {}", e))?;
        std::fs::write(&file_path, header_json + "\n")
            .map_err(|e| format!("Failed to write session file: {}", e))?;

        let metadata = SessionMetadata {
            id: session_id.to_string(),
            created_at,
            cwd: cwd.to_string(),
            path: Some(file_path.clone()),
            parent_session_path,
        };

        Ok(Self {
            metadata,
            file_path,
            entries: Vec::new(),
            by_id: std::collections::HashMap::new(),
            labels_by_id: std::collections::HashMap::new(),
            leaf_id: None,
        })
    }

    /// Open an existing session file. Loads all entries into memory.
    pub fn open(file_path: PathBuf) -> Result<Self, String> {
        let (header, entries) = load_session_from_file(&file_path);
        let header = header
            .ok_or_else(|| format!("Invalid or missing session header: {}", file_path.display()))?;

        let metadata = SessionMetadata {
            id: header.id.clone(),
            created_at: header.timestamp.clone(),
            cwd: header.cwd,
            path: Some(file_path.clone()),
            parent_session_path: header.parent_session,
        };

        let by_id: std::collections::HashMap<_, _> = entries
            .iter()
            .map(|e| (e.id().to_string(), e.clone()))
            .collect();
        let labels_by_id = build_labels_by_id(&entries);
        let leaf_id = entries.last().and_then(leaf_id_after_entry);

        Ok(Self {
            metadata,
            file_path,
            entries,
            by_id,
            labels_by_id,
            leaf_id,
        })
    }

    /// Append a line to the file.
    fn append_to_file(&self, entry: &SessionEntry) -> Result<(), String> {
        append_entry_to_file(&self.file_path, entry)
            .map_err(|e| format!("Failed to append session entry: {}", e))
    }
}

impl SessionStorage for JsonlSessionStorage {
    fn metadata(&self) -> SessionMetadata {
        self.metadata.clone()
    }

    fn get_leaf_id(&self) -> Option<String> {
        self.leaf_id.clone()
    }

    fn set_leaf_id(&mut self, leaf_id: Option<&str>) -> Result<(), String> {
        if let Some(id) = leaf_id
            && !self.by_id.contains_key(id)
        {
            return Err(format!("Entry {} not found", id));
        }
        let entry = SessionEntry::Leaf(LeafEntry {
            id: self.create_entry_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            target_id: leaf_id.map(|s| s.to_string()),
        });
        self.append_to_file(&entry)?;
        self.leaf_id = leaf_id.map(|s| s.to_string());
        self.entries.push(entry.clone());
        self.by_id.insert(entry.id().to_string(), entry);
        Ok(())
    }

    fn create_entry_id(&self) -> String {
        generate_entry_id(&self.by_id)
    }

    fn append_entry(&mut self, entry: SessionEntry) -> Result<(), String> {
        self.append_to_file(&entry)?;
        let id = entry.id().to_string();
        self.by_id.insert(id.clone(), entry);
        self.entries
            .push(self.by_id.get(&id).expect("just inserted").clone());
        self.leaf_id = leaf_id_after_entry(self.by_id.get(&id).expect("just inserted"));
        update_label_cache(
            &mut self.labels_by_id,
            self.by_id.get(&id).expect("just inserted"),
        );
        Ok(())
    }

    fn get_entry(&self, id: &str) -> Option<SessionEntry> {
        self.by_id.get(id).cloned()
    }

    fn find_entries(&self, type_name: &str) -> Vec<SessionEntry> {
        self.entries
            .iter()
            .filter(|e| entry_type_name(e) == type_name)
            .cloned()
            .collect()
    }

    fn get_label(&self, id: &str) -> Option<String> {
        self.labels_by_id.get(id).cloned()
    }

    fn get_path_to_root(&self, leaf_id: Option<&str>) -> Result<Vec<SessionEntry>, String> {
        let start_id = leaf_id.or(self.leaf_id.as_deref());
        if start_id.is_none() {
            return Ok(vec![]);
        }
        let sid = start_id.unwrap();
        let mut path: Vec<SessionEntry> = Vec::new();
        let mut current = self.by_id.get(sid);
        if current.is_none() {
            return Err(format!("Entry {} not found", sid));
        }
        while let Some(entry) = current {
            path.push(entry.clone());
            match entry.parent_id() {
                Some(pid) => {
                    current = self.by_id.get(pid);
                }
                None => break,
            }
        }
        path.reverse();
        Ok(path)
    }

    fn get_entries(&self) -> Vec<SessionEntry> {
        self.entries.clone()
    }

    fn path(&self) -> Option<&Path> {
        Some(&self.file_path)
    }
}

// ── Helper: entry type name ────────────────────────────────────────

/// Return the type string for a SessionEntry (pi-compatible).
fn entry_type_name(entry: &SessionEntry) -> &'static str {
    match entry {
        SessionEntry::Message(_) => "message",
        SessionEntry::ThinkingLevelChange(_) => "thinking_level_change",
        SessionEntry::ModelChange(_) => "model_change",
        SessionEntry::ActiveToolsChange(_) => "active_tools_change",
        SessionEntry::Compaction(_) => "compaction",
        SessionEntry::BranchSummary(_) => "branch_summary",
        SessionEntry::SessionInfo(_) => "session_info",
        SessionEntry::Label(_) => "label",
        SessionEntry::Custom(_) => "custom",
        SessionEntry::CustomMessage(_) => "custom_message",
        SessionEntry::Leaf(_) => "leaf",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::session::MessageEntry;
    use crate::agent::types::user_message;
    use tempfile::TempDir;

    fn make_session_meta(id: &str) -> SessionMetadata {
        SessionMetadata {
            id: id.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            cwd: "/tmp/test".to_string(),
            path: None,
            parent_session_path: None,
        }
    }

    fn make_msg_entry(id: &str, parent: Option<&str>, text: &str) -> SessionEntry {
        SessionEntry::Message(MessageEntry {
            id: id.to_string(),
            parent_id: parent.map(|s| s.to_string()),
            timestamp: chrono::Utc::now().to_rfc3339(),
            message: user_message(text),
            cost: 0.0,
        })
    }

    // ── InMemorySessionStorage tests ──────────────────────────────────

    #[test]
    fn test_in_memory_empty() {
        let meta = make_session_meta("test");
        let storage = InMemorySessionStorage::new(meta.clone());
        assert_eq!(storage.metadata().id, "test");
        assert!(storage.get_leaf_id().is_none());
        assert!(storage.get_entries().is_empty());
    }

    #[test]
    fn test_in_memory_append_and_get() {
        let mut storage = InMemorySessionStorage::new(make_session_meta("s1"));
        let e = make_msg_entry("m1", None, "hello");
        storage.append_entry(e).unwrap();
        assert_eq!(storage.get_leaf_id(), Some("m1".to_string()));
        assert_eq!(storage.get_entry("m1").unwrap().id(), "m1");
        assert_eq!(storage.get_entries().len(), 1);
    }

    #[test]
    fn test_in_memory_path_to_root() {
        let mut storage = InMemorySessionStorage::new(make_session_meta("s1"));
        storage
            .append_entry(make_msg_entry("m1", None, "first"))
            .unwrap();
        storage
            .append_entry(make_msg_entry("m2", Some("m1"), "second"))
            .unwrap();
        storage
            .append_entry(make_msg_entry("m3", Some("m2"), "third"))
            .unwrap();

        let path = storage.get_path_to_root(Some("m3")).unwrap();
        assert_eq!(path.len(), 3);
        assert_eq!(path[0].id(), "m1");
        assert_eq!(path[2].id(), "m3");
    }

    #[test]
    fn test_in_memory_labels() {
        let mut storage = InMemorySessionStorage::new(make_session_meta("s1"));
        storage
            .append_entry(make_msg_entry("m1", None, "first"))
            .unwrap();

        // Add label
        let label_entry = SessionEntry::Label(crate::agent::session::LabelEntry {
            id: "l1".to_string(),
            parent_id: Some("m1".to_string()),
            timestamp: chrono::Utc::now().to_rfc3339(),
            target_id: "m1".to_string(),
            label: Some("important".to_string()),
        });
        storage.append_entry(label_entry).unwrap();
        assert_eq!(storage.get_label("m1"), Some("important".to_string()));

        // Remove label
        let unlabel_entry = SessionEntry::Label(crate::agent::session::LabelEntry {
            id: "l2".to_string(),
            parent_id: Some("l1".to_string()),
            timestamp: chrono::Utc::now().to_rfc3339(),
            target_id: "m1".to_string(),
            label: None,
        });
        storage.append_entry(unlabel_entry).unwrap();
        assert_eq!(storage.get_label("m1"), None);
    }

    #[test]
    fn test_in_memory_set_leaf_id() {
        let mut storage = InMemorySessionStorage::new(make_session_meta("s1"));
        storage
            .append_entry(make_msg_entry("m1", None, "first"))
            .unwrap();
        storage
            .append_entry(make_msg_entry("m2", Some("m1"), "second"))
            .unwrap();

        // Set leaf to m1 (branching)
        storage.set_leaf_id(Some("m1")).unwrap();
        // The leaf entry points to m1
        assert_eq!(storage.get_leaf_id(), Some("m1".to_string()));

        // Verify leaf entry was appended
        let entries = storage.get_entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].id().len(), 8); // leaf entry has auto-generated id
        assert!(matches!(entries[2], SessionEntry::Leaf(_)));
    }

    #[test]
    fn test_in_memory_find_entries() {
        let mut storage = InMemorySessionStorage::new(make_session_meta("s1"));
        storage
            .append_entry(make_msg_entry("m1", None, "first"))
            .unwrap();
        let tl =
            SessionEntry::ThinkingLevelChange(crate::agent::session::ThinkingLevelChangeEntry {
                id: "tc1".to_string(),
                parent_id: Some("m1".to_string()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                thinking_level: "high".to_string(),
            });
        storage.append_entry(tl).unwrap();
        storage
            .append_entry(make_msg_entry("m2", Some("tc1"), "second"))
            .unwrap();

        let msgs = storage.find_entries("message");
        assert_eq!(msgs.len(), 2);
        let tls = storage.find_entries("thinking_level_change");
        assert_eq!(tls.len(), 1);
    }

    // ── JsonlSessionStorage tests ────────────────────────────────────

    #[test]
    fn test_jsonl_create_and_append() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session.jsonl");

        let mut storage =
            JsonlSessionStorage::create(path.clone(), "/tmp/test", "s1", None).unwrap();
        assert_eq!(storage.metadata().id, "s1");
        assert!(path.exists());

        storage
            .append_entry(make_msg_entry("m1", None, "hello"))
            .unwrap();
        assert_eq!(storage.get_entries().len(), 1);
        assert_eq!(storage.get_leaf_id(), Some("m1".to_string()));

        // Verify persistence by opening again
        let loaded = JsonlSessionStorage::open(path).unwrap();
        assert_eq!(loaded.get_entries().len(), 1);
        assert_eq!(loaded.get_entry("m1").unwrap().id(), "m1");
    }

    #[test]
    fn test_jsonl_open_and_traverse() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session.jsonl");

        let mut storage =
            JsonlSessionStorage::create(path.clone(), "/tmp/test", "s1", None).unwrap();
        storage
            .append_entry(make_msg_entry("m1", None, "first"))
            .unwrap();
        storage
            .append_entry(make_msg_entry("m2", Some("m1"), "second"))
            .unwrap();
        drop(storage);

        let loaded = JsonlSessionStorage::open(path).unwrap();
        let path_to = loaded.get_path_to_root(Some("m2")).unwrap();
        assert_eq!(path_to.len(), 2);
    }
}
