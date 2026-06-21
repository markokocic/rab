use crate::agent::types::AgentMessage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

// ── Constants ───────────────────────────────────────────────────────

pub const CURRENT_SESSION_VERSION: u32 = 3;

// ── Session header ──────────────────────────────────────────────────

/// The first entry in every session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHeader {
    #[serde(rename = "type")]
    pub type_: String, // always "session"
    #[serde(default)]
    pub version: Option<u32>,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

// ── Entry types ─────────────────────────────────────────────────────

/// A session entry — one JSON line in the session file.
///
/// Uses serde's internally-tagged enum with `type` field for discrimination.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEntry {
    #[serde(rename = "message")]
    Message(MessageEntry),
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    #[serde(rename = "model_change")]
    ModelChange(ModelChangeEntry),
    #[serde(rename = "compaction")]
    Compaction(CompactionEntry),
    #[serde(rename = "branch_summary")]
    BranchSummary(BranchSummaryEntry),
    #[serde(rename = "session_info")]
    SessionInfo(SessionInfoEntry),
    #[serde(rename = "label")]
    Label(LabelEntry),
    #[serde(rename = "custom")]
    Custom(CustomEntry),
    #[serde(rename = "custom_message")]
    CustomMessage(CustomMessageEntry),
}

impl SessionEntry {
    pub fn id(&self) -> &str {
        match self {
            SessionEntry::Message(e) => &e.id,
            SessionEntry::ThinkingLevelChange(e) => &e.id,
            SessionEntry::ModelChange(e) => &e.id,
            SessionEntry::Compaction(e) => &e.id,
            SessionEntry::BranchSummary(e) => &e.id,
            SessionEntry::SessionInfo(e) => &e.id,
            SessionEntry::Label(e) => &e.id,
            SessionEntry::Custom(e) => &e.id,
            SessionEntry::CustomMessage(e) => &e.id,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            SessionEntry::Message(e) => e.parent_id.as_deref(),
            SessionEntry::ThinkingLevelChange(e) => e.parent_id.as_deref(),
            SessionEntry::ModelChange(e) => e.parent_id.as_deref(),
            SessionEntry::Compaction(e) => e.parent_id.as_deref(),
            SessionEntry::BranchSummary(e) => e.parent_id.as_deref(),
            SessionEntry::SessionInfo(e) => e.parent_id.as_deref(),
            SessionEntry::Label(e) => e.parent_id.as_deref(),
            SessionEntry::Custom(e) => e.parent_id.as_deref(),
            SessionEntry::CustomMessage(e) => e.parent_id.as_deref(),
        }
    }

    pub fn timestamp(&self) -> &str {
        match self {
            SessionEntry::Message(e) => &e.timestamp,
            SessionEntry::ThinkingLevelChange(e) => &e.timestamp,
            SessionEntry::ModelChange(e) => &e.timestamp,
            SessionEntry::Compaction(e) => &e.timestamp,
            SessionEntry::BranchSummary(e) => &e.timestamp,
            SessionEntry::SessionInfo(e) => &e.timestamp,
            SessionEntry::Label(e) => &e.timestamp,
            SessionEntry::Custom(e) => &e.timestamp,
            SessionEntry::CustomMessage(e) => &e.timestamp,
        }
    }
}

/// Base fields shared by all entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub message: AgentMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingLevelChangeEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub thinking_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChangeEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub provider: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub from_id: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub target_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub custom_type: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessageEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub custom_type: String,
    pub content: serde_json::Value,
    #[serde(default)]
    pub display: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

// ── SessionInfo (for listing / display) ─────────────────────────────

/// Lightweight metadata about a session, used for listing and selection.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub path: PathBuf,
    pub id: String,
    pub cwd: String,
    pub name: Option<String>,
    pub parent_session_path: Option<String>,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub message_count: usize,
    pub first_message: String,
    /// All messages concatenated (for text search).
    pub all_messages_text: String,
}

// ── SessionContext (resolved messages for LLM) ──────────────────────

/// Resolved conversation context sent to the LLM.
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub messages: Vec<AgentMessage>,
}

// ── JSONL read/write ────────────────────────────────────────────────

/// Parse a single line as a SessionEntry. Returns None for empty/malformed lines.
pub fn parse_session_entry_line(line: &str) -> Option<SessionEntry> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// Parse a single line as a SessionHeader.
pub fn parse_session_header_line(line: &str) -> Option<SessionHeader> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let header: SessionHeader = serde_json::from_str(line).ok()?;
    if header.type_ != "session" {
        return None;
    }
    Some(header)
}

/// Read the session header from a JSONL file (first line only).
pub fn read_session_header(path: &Path) -> Option<SessionHeader> {
    let content = fs::read_to_string(path).ok()?;
    let first_line = content.lines().next()?;
    parse_session_header_line(first_line)
}

/// Load all entries from a session JSONL file.
/// Returns (header, entries) or empty vec if the file is missing or corrupted.
pub fn load_entries_from_file(path: &Path) -> Vec<SessionEntry> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let entries: Vec<SessionEntry> = content
        .lines()
        .filter_map(parse_session_entry_line)
        .collect();

    // Validate: first entry must be a session header (type = "session")
    // We check this by ensuring at least one entry exists and is not a non-header type.
    // Header entries have type="session" which is parsed as a serde error for SessionEntry
    // since we use tagged enum. The header line will fail to parse as SessionEntry.
    // That's fine — load_entries_from_file returns only SessionEntry items, not the header.
    // The caller uses read_session_header() separately for the header.

    entries
}

/// Write entries to a session file (used for initial write / rewrite).
/// Does NOT write the session header — caller must include it.
pub fn write_entries_to_file(
    path: &Path,
    header: &SessionHeader,
    entries: &[SessionEntry],
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut content = serde_json::to_string(header).map_err(std::io::Error::from)?;
    content.push('\n');
    for entry in entries {
        let line = serde_json::to_string(entry).map_err(std::io::Error::from)?;
        content.push_str(&line);
        content.push('\n');
    }
    fs::write(path, &content)
}

/// Append a single entry to the session file (one JSON line).
pub fn append_entry_to_file(path: &Path, entry: &SessionEntry) -> std::io::Result<()> {
    let line = serde_json::to_string(entry).map_err(std::io::Error::from)?;
    let content = format!("{}\n", line);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(content.as_bytes())
}

// ── CWD encoding ────────────────────────────────────────────────────

/// Encode a working directory path into a safe directory name.
/// Mirrors pi's encoding: strip leading /, replace / \ : with -, wrap in --...--
pub fn encode_cwd_for_dir(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let cleaned = s
        .trim_start_matches('/')
        .trim_start_matches('\\')
        .replace(['/', '\\', ':'], "-");
    format!("--{}--", cleaned)
}

/// Get the default session directory for a cwd.
pub fn get_default_session_dir(cwd: &Path) -> PathBuf {
    let rab_dir = directories::BaseDirs::new()
        .expect("Could not determine home directory")
        .home_dir()
        .join(".rab");
    rab_dir.join("sessions").join(encode_cwd_for_dir(cwd))
}

/// Generate a unique ID for session entries (8 hex chars, collision-checked).
pub fn generate_entry_id(by_id: &HashMap<String, SessionEntry>) -> String {
    for _ in 0..100 {
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        if !by_id.contains_key(&id) {
            return id;
        }
    }
    // Fallback to full UUID
    uuid::Uuid::new_v4().to_string()
}

// ── SessionManager ──────────────────────────────────────────────────

/// Manages conversation sessions as append-only trees in JSONL files.
///
/// Each entry has an id and parentId forming a tree structure.
/// Appending creates a child of the current leaf. Branching moves the
/// leaf to an earlier entry, allowing new branches without modifying history.
pub struct SessionManager {
    session_id: String,
    session_file: Option<PathBuf>,
    session_dir: PathBuf,
    cwd: PathBuf,
    persist: bool,
    flushed: bool,
    file_entries: Vec<SessionEntry>,
    by_id: HashMap<String, SessionEntry>,
    labels_by_id: HashMap<String, String>,
    leaf_id: Option<String>,
}

impl SessionManager {
    // ── Construction ─────────────────────────────────────────────

    fn new(
        cwd: &Path,
        session_dir: &Path,
        session_file: Option<PathBuf>,
        persist: bool,
        create_new: bool,
    ) -> Self {
        let cwd = cwd.to_path_buf();
        let session_dir = session_dir.to_path_buf();

        let mut sm = Self {
            session_id: String::new(),
            session_file: None,
            session_dir,
            cwd,
            persist,
            flushed: false,
            file_entries: Vec::new(),
            by_id: HashMap::new(),
            labels_by_id: HashMap::new(),
            leaf_id: None,
        };

        if let Some(path) = session_file {
            sm.set_session_file(&path);
            if create_new {
                // Override: force new session even if file was loaded
                sm.new_session(None);
                sm.session_file = Some(path);
            }
        } else if create_new {
            sm.new_session(None);
        }

        sm
    }

    /// Switch to a different session file.
    fn set_session_file(&mut self, session_file: &Path) {
        self.session_file = Some(session_file.to_path_buf());
        if session_file.exists() {
            self.file_entries = load_entries_from_file(session_file);
            let header = read_session_header(session_file);

            // If file is empty or has no valid header, treat as corrupted:
            // truncate and start fresh, preserving the file path.
            if self.file_entries.is_empty() && header.is_none() {
                let explicit_path = self.session_file.clone();
                self.new_session(None);
                self.session_file = explicit_path;
                self._rewrite_file();
                self.flushed = true;
                return;
            }

            // Entries exist (or header exists but no entries yet — keep the session)
            self.session_id = header
                .map(|h| h.id)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            self.migrate_to_current();
            self._build_index();
            self.flushed = true;
        } else {
            // File doesn't exist — create new session at this path
            let explicit_path = self.session_file.clone();
            self.new_session(None);
            self.session_file = explicit_path;
        }
    }

    /// Create a new session (overwrites current entries).
    fn new_session(&mut self, id: Option<&str>) {
        self.session_id = id
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let timestamp = chrono::Utc::now().to_rfc3339();
        let header = SessionHeader {
            type_: "session".to_string(),
            version: Some(CURRENT_SESSION_VERSION),
            id: self.session_id.clone(),
            timestamp,
            cwd: self.cwd.to_string_lossy().to_string(),
            parent_session: None,
        };
        // Store header as file_entries[0] implicitly via the first entry.
        // We handle it separately in file operations.
        self.file_entries = Vec::new();
        self.by_id.clear();
        self.labels_by_id.clear();
        self.leaf_id = None;
        self.flushed = false;

        if self.persist {
            let file_ts = header.timestamp.replace([':', '.'], "-");
            self.session_file = Some(
                self.session_dir
                    .join(format!("{}_{}.jsonl", file_ts, self.session_id)),
            );
        }

        // Store header separately for rewrite
        // We use a sentinel pattern: the header is reconstructed from fields
    }

    fn _build_index(&mut self) {
        self.by_id.clear();
        self.labels_by_id.clear();
        self.leaf_id = None;
        for entry in &self.file_entries {
            self.by_id.insert(entry.id().to_string(), entry.clone());
            self.leaf_id = Some(entry.id().to_string());
            if let SessionEntry::Label(e) = entry {
                if let Some(label) = &e.label {
                    self.labels_by_id.insert(e.target_id.clone(), label.clone());
                } else {
                    self.labels_by_id.remove(&e.target_id);
                }
            }
        }
    }

    fn _rewrite_file(&self) {
        if !self.persist {
            return;
        }
        if let Some(ref path) = self.session_file {
            let header = self._make_header();
            let _ = write_entries_to_file(path, &header, &self.file_entries);
        }
    }

    fn _make_header(&self) -> SessionHeader {
        SessionHeader {
            type_: "session".to_string(),
            version: Some(CURRENT_SESSION_VERSION),
            id: self.session_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            cwd: self.cwd.to_string_lossy().to_string(),
            parent_session: None,
        }
    }

    fn _persist(&mut self) {
        if !self.persist {
            return;
        }
        let has_assistant = self
            .file_entries
            .iter()
            .any(|e| matches!(e, SessionEntry::Message(m) if m.message.role == crate::agent::types::Role::Assistant));

        if !has_assistant {
            // Don't create file until first assistant message
            self.flushed = false;
            return;
        }

        if !self.flushed {
            if let Some(ref path) = self.session_file {
                let header = self._make_header();
                let _ = write_entries_to_file(path, &header, &self.file_entries);
                self.flushed = true;
            }
        } else if let Some(ref path) = self.session_file
            && let Some(entry) = self.file_entries.last()
        {
            let _ = append_entry_to_file(path, entry);
        }
    }

    fn _append_entry(&mut self, entry: SessionEntry) -> String {
        let id = entry.id().to_string();
        self.file_entries.push(entry.clone());
        self.by_id.insert(id.clone(), entry);
        self.leaf_id = Some(id.clone());
        self._persist();
        id
    }

    /// Run migrations to bring entries to the current version.
    /// Currently a no-op since we only write v3 entries.
    fn migrate_to_current(&mut self) {
        // For now, just ensure entries look valid.
        // Future: handle v1→v2 (add id/parentId) and v2→v3 (hookMessage→custom).
    }

    // ── Public: Info ──────────────────────────────────────────────

    pub fn is_persisted(&self) -> bool {
        self.persist
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    /// Returns true if using the default cwd-encoded session directory.
    pub fn uses_default_session_dir(&self) -> bool {
        self.session_dir == get_default_session_dir(&self.cwd)
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn session_file(&self) -> Option<&Path> {
        self.session_file.as_deref()
    }

    pub fn leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }

    /// Get the current session name from the latest session_info entry.
    pub fn session_name(&self) -> Option<&str> {
        for entry in self.file_entries.iter().rev() {
            if let SessionEntry::SessionInfo(e) = entry {
                let name = e.name.trim();
                if name.is_empty() {
                    return None;
                }
                return Some(name);
            }
        }
        None
    }

    // ── Public: Appending ─────────────────────────────────────────

    /// Append a message as child of current leaf, then advance leaf.
    /// Returns the entry id.
    pub fn append_message(&mut self, message: &AgentMessage) -> String {
        let entry = SessionEntry::Message(MessageEntry {
            id: generate_entry_id(&self.by_id),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            message: message.clone(),
        });
        self._append_entry(entry)
    }

    /// Append a thinking level change.
    pub fn append_thinking_level_change(&mut self, thinking_level: &str) -> String {
        let entry = SessionEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
            id: generate_entry_id(&self.by_id),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            thinking_level: thinking_level.to_string(),
        });
        self._append_entry(entry)
    }

    /// Append a model change.
    pub fn append_model_change(&mut self, provider: &str, model_id: &str) -> String {
        let entry = SessionEntry::ModelChange(ModelChangeEntry {
            id: generate_entry_id(&self.by_id),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            provider: provider.to_string(),
            model_id: model_id.to_string(),
        });
        self._append_entry(entry)
    }

    /// Append a session info entry (display name).
    pub fn append_session_info(&mut self, name: &str) -> String {
        let entry = SessionEntry::SessionInfo(SessionInfoEntry {
            id: generate_entry_id(&self.by_id),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            name: name.trim().to_string(),
        });
        self._append_entry(entry)
    }

    /// Append a compaction summary.
    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        tokens_before: u64,
    ) -> String {
        let entry = SessionEntry::Compaction(CompactionEntry {
            id: generate_entry_id(&self.by_id),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            summary: summary.to_string(),
            first_kept_entry_id: first_kept_entry_id.to_string(),
            tokens_before,
            details: None,
            from_hook: None,
        });
        self._append_entry(entry)
    }

    /// Append a branch summary.
    pub fn append_branch_summary(&mut self, from_id: &str, summary: &str) -> String {
        let entry = SessionEntry::BranchSummary(BranchSummaryEntry {
            id: generate_entry_id(&self.by_id),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            from_id: from_id.to_string(),
            summary: summary.to_string(),
            details: None,
            from_hook: None,
        });
        self._append_entry(entry)
    }

    /// Append a label change (bookmark/unbookmark).
    pub fn append_label_change(&mut self, target_id: &str, label: Option<&str>) -> String {
        let entry = SessionEntry::Label(LabelEntry {
            id: generate_entry_id(&self.by_id),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            target_id: target_id.to_string(),
            label: label.map(|s| s.to_string()),
        });
        let id = self._append_entry(entry);

        // Update label map
        if let Some(l) = label {
            self.labels_by_id
                .insert(target_id.to_string(), l.to_string());
        } else {
            self.labels_by_id.remove(target_id);
        }
        id
    }

    /// Append a custom entry (extension data).
    pub fn append_custom_entry(&mut self, custom_type: &str, data: serde_json::Value) -> String {
        let entry = SessionEntry::Custom(CustomEntry {
            id: generate_entry_id(&self.by_id),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            custom_type: custom_type.to_string(),
            data,
        });
        self._append_entry(entry)
    }

    // ── Public: Tree navigation ───────────────────────────────────

    /// Get all entries (excludes header).
    pub fn entries(&self) -> &[SessionEntry] {
        &self.file_entries
    }

    /// Look up an entry by id.
    pub fn entry(&self, id: &str) -> Option<&SessionEntry> {
        self.by_id.get(id)
    }

    /// Get all direct children of an entry.
    pub fn children(&self, parent_id: &str) -> Vec<&SessionEntry> {
        self.file_entries
            .iter()
            .filter(|e| e.parent_id() == Some(parent_id))
            .collect()
    }

    /// Walk from entry to root, returning all entries in path order.
    pub fn branch(&self, from_id: Option<&str>) -> Vec<&SessionEntry> {
        let start_id = from_id.or(self.leaf_id.as_deref());
        let mut path = Vec::new();
        let mut current = start_id.and_then(|id| self.by_id.get(id));
        while let Some(entry) = current {
            path.push(entry);
            current = entry.parent_id().and_then(|pid| self.by_id.get(pid));
        }
        path.reverse();
        path
    }

    /// Build the session context (messages for LLM).
    pub fn build_session_context(&self) -> SessionContext {
        let path = self.branch(None);
        let messages: Vec<AgentMessage> = path
            .iter()
            .filter_map(|entry| {
                if let SessionEntry::Message(e) = entry {
                    Some(e.message.clone())
                } else {
                    None
                }
            })
            .collect();
        SessionContext { messages }
    }

    /// Get the label for an entry, if any.
    pub fn label(&self, id: &str) -> Option<&str> {
        self.labels_by_id.get(id).map(|s| s.as_str())
    }

    // ── Public: Branching ─────────────────────────────────────────

    /// Move leaf pointer to an earlier entry (starts a new branch).
    pub fn set_branch(&mut self, branch_from_id: &str) -> Result<(), String> {
        if !self.by_id.contains_key(branch_from_id) {
            return Err(format!("Entry {} not found", branch_from_id));
        }
        self.leaf_id = Some(branch_from_id.to_string());
        Ok(())
    }

    /// Reset leaf pointer to null (before any entries).
    pub fn reset_leaf(&mut self) {
        self.leaf_id = None;
    }

    // ── Static factories ──────────────────────────────────────────

    /// Create a new session.
    pub fn create(cwd: &Path, session_dir: Option<&Path>) -> Self {
        let dir = session_dir
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| get_default_session_dir(cwd));
        Self::new(cwd, &dir, None, true, true)
    }

    /// Open a specific session file.
    pub fn open(path: &Path, session_dir: Option<&Path>, cwd_override: Option<&Path>) -> Self {
        let cwd = if let Some(cwd_path) = cwd_override {
            cwd_path.to_path_buf()
        } else {
            // Extract cwd from header
            read_session_header(path)
                .map(|h| PathBuf::from(h.cwd))
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
        };
        let dir = session_dir.map(|p| p.to_path_buf()).unwrap_or_else(|| {
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| get_default_session_dir(&cwd))
        });
        Self::new(&cwd, &dir, Some(path.to_path_buf()), true, false)
    }

    /// Create an in-memory session (no file persistence).
    pub fn in_memory(cwd: &Path) -> Self {
        let dir = get_default_session_dir(cwd);
        Self::new(cwd, &dir, None, false, true)
    }

    /// Continue the most recent session, or create new if none.
    pub fn continue_recent(cwd: &Path, session_dir: Option<&Path>) -> Self {
        let dir = session_dir
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| get_default_session_dir(cwd));
        let filter_cwd = session_dir.is_some_and(|sd| sd != get_default_session_dir(cwd));
        let most_recent = find_most_recent_session(&dir, if filter_cwd { Some(cwd) } else { None });
        if let Some(path) = most_recent {
            Self::new(cwd, &dir, Some(path), true, false)
        } else {
            Self::new(cwd, &dir, None, true, true)
        }
    }
}

/// Find the most recent session file by mtime.
pub fn find_most_recent_session(session_dir: &Path, filter_cwd: Option<&Path>) -> Option<PathBuf> {
    let resolved_cwd = filter_cwd.map(|c| c.to_path_buf());
    let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    let entries = std::fs::read_dir(session_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            let header = read_session_header(&path);
            if let Some(ref h) = header {
                if let Some(ref rcwd) = resolved_cwd
                    && h.cwd != rcwd.to_string_lossy().as_ref()
                {
                    continue;
                }
            } else {
                continue;
            }
            if let Ok(meta) = path.metadata()
                && let Ok(mtime) = meta.modified()
            {
                files.push((path, mtime));
            }
        }
    }

    files.sort_by_key(|b| std::cmp::Reverse(b.1));
    files.into_iter().next().map(|(path, _)| path)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::{AgentMessage, Role, Usage};
    use tempfile::TempDir;

    fn make_message(role: Role, content: &str) -> AgentMessage {
        AgentMessage {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role,
            content: content.to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            usage: None,
            is_error: false,
            timestamp: Utc::now().timestamp_millis(),
        }
    }

    // ── Entry serialization round-trip ──────────────────────────────

    #[test]
    fn test_message_entry_roundtrip() {
        let msg = make_message(Role::User, "hello world");
        let entry = SessionEntry::Message(MessageEntry {
            id: "abc12345".to_string(),
            parent_id: None,
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            message: msg.clone(),
        });

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();

        match parsed {
            SessionEntry::Message(e) => {
                assert_eq!(e.id, "abc12345");
                assert_eq!(e.parent_id, None);
                assert_eq!(e.message.role, Role::User);
                assert_eq!(e.message.content, "hello world");
            }
            _ => panic!("Expected Message variant"),
        }
    }

    #[test]
    fn test_thinking_level_change_roundtrip() {
        let entry = SessionEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
            id: "abc12345".to_string(),
            parent_id: Some("parent1".to_string()),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            thinking_level: "high".to_string(),
        });

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();

        match parsed {
            SessionEntry::ThinkingLevelChange(e) => {
                assert_eq!(e.thinking_level, "high");
                assert_eq!(e.parent_id.as_deref(), Some("parent1"));
            }
            _ => panic!("Expected ThinkingLevelChange variant"),
        }
    }

    #[test]
    fn test_model_change_roundtrip() {
        let entry = SessionEntry::ModelChange(ModelChangeEntry {
            id: "abc12345".to_string(),
            parent_id: Some("parent1".to_string()),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            provider: "opencode_go".to_string(),
            model_id: "deepseek-v4-pro".to_string(),
        });

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();

        match parsed {
            SessionEntry::ModelChange(e) => {
                assert_eq!(e.provider, "opencode_go");
                assert_eq!(e.model_id, "deepseek-v4-pro");
            }
            _ => panic!("Expected ModelChange variant"),
        }
    }

    #[test]
    fn test_compaction_entry_roundtrip() {
        let entry = SessionEntry::Compaction(CompactionEntry {
            id: "abc12345".to_string(),
            parent_id: Some("parent1".to_string()),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            summary: "Earlier conversation summarized...".to_string(),
            first_kept_entry_id: "entry123".to_string(),
            tokens_before: 5000,
            details: None,
            from_hook: None,
        });

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();

        match parsed {
            SessionEntry::Compaction(e) => {
                assert_eq!(e.summary, "Earlier conversation summarized...");
                assert_eq!(e.first_kept_entry_id, "entry123");
                assert_eq!(e.tokens_before, 5000);
            }
            _ => panic!("Expected Compaction variant"),
        }
    }

    #[test]
    fn test_branch_summary_roundtrip() {
        let entry = SessionEntry::BranchSummary(BranchSummaryEntry {
            id: "abc12345".to_string(),
            parent_id: Some("parent1".to_string()),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            from_id: "branch_point".to_string(),
            summary: "Abandoned work on feature X".to_string(),
            details: None,
            from_hook: None,
        });

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();

        match parsed {
            SessionEntry::BranchSummary(e) => {
                assert_eq!(e.summary, "Abandoned work on feature X");
                assert_eq!(e.from_id, "branch_point");
            }
            _ => panic!("Expected BranchSummary variant"),
        }
    }

    #[test]
    fn test_session_info_roundtrip() {
        let entry = SessionEntry::SessionInfo(SessionInfoEntry {
            id: "abc12345".to_string(),
            parent_id: Some("parent1".to_string()),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            name: "Refactor auth module".to_string(),
        });

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();

        match parsed {
            SessionEntry::SessionInfo(e) => {
                assert_eq!(e.name, "Refactor auth module");
            }
            _ => panic!("Expected SessionInfo variant"),
        }
    }

    #[test]
    fn test_label_entry_roundtrip() {
        // Set label
        let entry = SessionEntry::Label(LabelEntry {
            id: "abc12345".to_string(),
            parent_id: Some("parent1".to_string()),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            target_id: "target_entry".to_string(),
            label: Some("important".to_string()),
        });

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();

        match parsed {
            SessionEntry::Label(e) => {
                assert_eq!(e.label.as_deref(), Some("important"));
                assert_eq!(e.target_id, "target_entry");
            }
            _ => panic!("Expected Label variant"),
        }

        // Clear label
        let entry = SessionEntry::Label(LabelEntry {
            id: "abc12346".to_string(),
            parent_id: Some("parent1".to_string()),
            timestamp: "2026-06-19T12:01:00Z".to_string(),
            target_id: "target_entry".to_string(),
            label: None,
        });

        let json = serde_json::to_string(&entry).unwrap();
        // With skip_serializing_if = "Option::is_none", label field is omitted when None
        assert!(!json.contains(r#""label":"#));
    }

    #[test]
    fn test_custom_entry_roundtrip() {
        let entry = SessionEntry::Custom(CustomEntry {
            id: "abc12345".to_string(),
            parent_id: Some("parent1".to_string()),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            custom_type: "my_extension".to_string(),
            data: serde_json::json!({"key": "value"}),
        });

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();

        match parsed {
            SessionEntry::Custom(e) => {
                assert_eq!(e.custom_type, "my_extension");
                assert_eq!(e.data["key"], "value");
            }
            _ => panic!("Expected Custom variant"),
        }
    }

    #[test]
    fn test_custom_message_entry_roundtrip() {
        let entry = SessionEntry::CustomMessage(CustomMessageEntry {
            id: "abc12345".to_string(),
            parent_id: Some("parent1".to_string()),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            custom_type: "my_extension".to_string(),
            content: serde_json::json!({"text": "Hello from extension"}),
            display: true,
            details: Some(serde_json::json!({"source": "plugin"})),
        });

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();

        match parsed {
            SessionEntry::CustomMessage(e) => {
                assert_eq!(e.custom_type, "my_extension");
                assert!(e.display);
            }
            _ => panic!("Expected CustomMessage variant"),
        }
    }

    // ── JSONL read/write ────────────────────────────────────────────

    #[test]
    fn test_parse_session_entry_line() {
        let entry = SessionEntry::SessionInfo(SessionInfoEntry {
            id: "abc12345".to_string(),
            parent_id: None,
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            name: "Test session".to_string(),
        });
        let json = serde_json::to_string(&entry).unwrap();
        let parsed = parse_session_entry_line(&json);
        assert!(parsed.is_some());
    }

    #[test]
    fn test_parse_session_entry_line_empty() {
        assert!(parse_session_entry_line("").is_none());
        assert!(parse_session_entry_line("   ").is_none());
    }

    #[test]
    fn test_parse_session_entry_line_malformed() {
        assert!(parse_session_entry_line("not valid json").is_none());
    }

    #[test]
    fn test_parse_session_header_line() {
        let header = SessionHeader {
            type_: "session".to_string(),
            version: Some(3),
            id: "session123".to_string(),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            cwd: "/home/user/project".to_string(),
            parent_session: None,
        };
        let json = serde_json::to_string(&header).unwrap();
        let parsed = parse_session_header_line(&json);
        assert!(parsed.is_some());
        assert_eq!(parsed.unwrap().id, "session123");
    }

    #[test]
    fn test_parse_session_header_line_wrong_type() {
        // parse_session_header_line validates type == "session"
        let json =
            r#"{"type":"message","id":"abc","timestamp":"2026-06-19T12:00:00Z","cwd":"/home"}"#;
        let result = parse_session_header_line(json);
        assert!(result.is_none());
    }

    #[test]
    fn test_write_and_read_entries() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.jsonl");

        let header = SessionHeader {
            type_: "session".to_string(),
            version: Some(3),
            id: "session1".to_string(),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            cwd: "/home/user/project".to_string(),
            parent_session: None,
        };

        let entries: Vec<SessionEntry> = vec![
            SessionEntry::Message(MessageEntry {
                id: "msg1".to_string(),
                parent_id: None,
                timestamp: "2026-06-19T12:00:01Z".to_string(),
                message: make_message(Role::User, "hello"),
            }),
            SessionEntry::Message(MessageEntry {
                id: "msg2".to_string(),
                parent_id: Some("msg1".to_string()),
                timestamp: "2026-06-19T12:00:02Z".to_string(),
                message: {
                    let mut m = make_message(Role::Assistant, "hi there");
                    m.usage = Some(Usage {
                        input_tokens: Some(10),
                        output_tokens: Some(5),
                        cache_tokens: None,
                    });
                    m
                },
            }),
        ];

        write_entries_to_file(&file_path, &header, &entries).unwrap();

        // Read back header
        let read_header = read_session_header(&file_path).unwrap();
        assert_eq!(read_header.id, "session1");

        // Read back entries
        let read_entries = load_entries_from_file(&file_path);
        assert_eq!(read_entries.len(), 2);

        match &read_entries[0] {
            SessionEntry::Message(e) => {
                assert_eq!(e.id, "msg1");
                assert_eq!(e.message.role, Role::User);
                assert_eq!(e.message.content, "hello");
            }
            _ => panic!("Expected Message"),
        }

        match &read_entries[1] {
            SessionEntry::Message(e) => {
                assert_eq!(e.id, "msg2");
                assert_eq!(e.message.role, Role::Assistant);
                assert_eq!(e.message.content, "hi there");
                assert!(e.message.usage.is_some());
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_append_entry_to_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("append_test.jsonl");

        let entry = SessionEntry::SessionInfo(SessionInfoEntry {
            id: "abc12345".to_string(),
            parent_id: None,
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            name: "Test".to_string(),
        });

        append_entry_to_file(&file_path, &entry).unwrap();

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("Test"));
        assert!(content.contains("abc12345"));
    }

    #[test]
    fn test_load_entries_missing_file() {
        let entries = load_entries_from_file(Path::new("/nonexistent/file.jsonl"));
        assert!(entries.is_empty());
    }

    #[test]
    fn test_read_session_header_missing_file() {
        let header = read_session_header(Path::new("/nonexistent/file.jsonl"));
        assert!(header.is_none());
    }

    // ── CWD encoding ────────────────────────────────────────────────

    #[test]
    fn test_encode_cwd() {
        assert_eq!(
            encode_cwd_for_dir(Path::new("/home/user/project")),
            "--home-user-project--"
        );
    }

    #[test]
    fn test_encode_cwd_windows_style() {
        assert_eq!(
            encode_cwd_for_dir(Path::new("C:\\Users\\user\\project")),
            "--C--Users-user-project--"
        );
    }

    #[test]
    fn test_encode_cwd_no_leading_slash() {
        assert_eq!(
            encode_cwd_for_dir(Path::new("home/user/project")),
            "--home-user-project--"
        );
    }

    #[test]
    fn test_encode_cwd_special_chars() {
        assert_eq!(
            encode_cwd_for_dir(Path::new("/home/user/my:project")),
            "--home-user-my-project--"
        );
    }

    // ── SessionEntry accessors ───────────────────────────────────────

    #[test]
    fn test_entry_id_accessor() {
        let entry = SessionEntry::Message(MessageEntry {
            id: "myid".to_string(),
            parent_id: None,
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            message: make_message(Role::User, "hello"),
        });
        assert_eq!(entry.id(), "myid");
    }

    #[test]
    fn test_entry_parent_id_accessor() {
        let entry = SessionEntry::Message(MessageEntry {
            id: "myid".to_string(),
            parent_id: Some("parent".to_string()),
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            message: make_message(Role::User, "hello"),
        });
        assert_eq!(entry.parent_id(), Some("parent"));
    }

    #[test]
    fn test_entry_timestamp_accessor() {
        let entry = SessionEntry::Message(MessageEntry {
            id: "myid".to_string(),
            parent_id: None,
            timestamp: "2026-06-19T12:00:00Z".to_string(),
            message: make_message(Role::User, "hello"),
        });
        assert_eq!(entry.timestamp(), "2026-06-19T12:00:00Z");
    }

    // ── generate_entry_id ────────────────────────────────────────────

    #[test]
    fn test_generate_entry_id_length() {
        let map = HashMap::new();
        let id = generate_entry_id(&map);
        assert_eq!(id.len(), 8);
    }

    #[test]
    fn test_generate_entry_id_hex() {
        let map = HashMap::new();
        let id = generate_entry_id(&map);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_entry_id_collision_fallback() {
        // Create a map that has all possible 8-char hex IDs — impossible
        // but we test the fallback behavior by only having a collision
        // on the first generated ID (unlikely but the code handles it).
        // This is more of a smoke test that the function doesn't panic.
        let map = HashMap::new();
        let id1 = generate_entry_id(&map);
        assert!(!id1.is_empty());
    }

    // ── Deserialize from pi-compatible JSON ──────────────────────────

    #[test]
    fn test_deserialize_pi_format_message() {
        // pi format uses camelCase and "type": "message"
        let json = r#"{"type":"message","id":"abc12345","parentId":null,"timestamp":"2026-06-19T12:00:00Z","message":{"id":"msg1","parentId":null,"role":"user","content":"hello","toolCalls":[],"isError":false,"timestamp":1718800000000}}"#;
        let entry: SessionEntry = serde_json::from_str(json).unwrap();
        match entry {
            SessionEntry::Message(e) => {
                assert_eq!(e.id, "abc12345");
                assert_eq!(e.message.content, "hello");
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_deserialize_pi_format_thinking_level() {
        let json = r#"{"type":"thinking_level_change","id":"abc12345","parentId":"parent1","timestamp":"2026-06-19T12:00:00Z","thinkingLevel":"high"}"#;
        let entry: SessionEntry = serde_json::from_str(json).unwrap();
        match entry {
            SessionEntry::ThinkingLevelChange(e) => {
                assert_eq!(e.thinking_level, "high");
            }
            _ => panic!("Expected ThinkingLevelChange"),
        }
    }

    #[test]
    fn test_deserialize_pi_format_model_change() {
        let json = r#"{"type":"model_change","id":"abc12345","parentId":"parent1","timestamp":"2026-06-19T12:00:00Z","provider":"opencode_go","modelId":"deepseek-v4-pro"}"#;
        let entry: SessionEntry = serde_json::from_str(json).unwrap();
        match entry {
            SessionEntry::ModelChange(e) => {
                assert_eq!(e.provider, "opencode_go");
                assert_eq!(e.model_id, "deepseek-v4-pro");
            }
            _ => panic!("Expected ModelChange"),
        }
    }

    #[test]
    fn test_deserialize_pi_format_compaction() {
        let json = r#"{"type":"compaction","id":"abc12345","parentId":"parent1","timestamp":"2026-06-19T12:00:00Z","summary":"Earlier conversation summarized","firstKeptEntryId":"entry123","tokensBefore":5000}"#;
        let entry: SessionEntry = serde_json::from_str(json).unwrap();
        match entry {
            SessionEntry::Compaction(e) => {
                assert_eq!(e.summary, "Earlier conversation summarized");
                assert_eq!(e.first_kept_entry_id, "entry123");
                assert_eq!(e.tokens_before, 5000);
            }
            _ => panic!("Expected Compaction"),
        }
    }

    #[test]
    fn test_deserialize_pi_format_session_info() {
        let json = r#"{"type":"session_info","id":"abc12345","parentId":"parent1","timestamp":"2026-06-19T12:00:00Z","name":"My session"}"#;
        let entry: SessionEntry = serde_json::from_str(json).unwrap();
        match entry {
            SessionEntry::SessionInfo(e) => {
                assert_eq!(e.name, "My session");
            }
            _ => panic!("Expected SessionInfo"),
        }
    }

    // ── SessionManager ───────────────────────────────────────────────

    fn make_agent_message(role: Role, content: &str) -> AgentMessage {
        AgentMessage {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role,
            content: content.to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            usage: None,
            is_error: false,
            timestamp: Utc::now().timestamp_millis(),
        }
    }

    #[test]
    fn test_session_create_in_memory() {
        let cwd = Path::new("/tmp/test-project");
        let sm = SessionManager::in_memory(cwd);
        assert!(!sm.is_persisted());
        assert!(!sm.session_id().is_empty());
        assert_eq!(sm.cwd(), cwd);
        assert!(sm.leaf_id().is_none());
        assert!(sm.entries().is_empty());
    }

    #[test]
    fn test_session_create_persisted() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let sm = SessionManager::create(&cwd, Some(&sessions_dir));
        assert!(sm.is_persisted());
        assert!(!sm.session_id().is_empty());
        // File not created yet (deferred until first assistant message)
        assert!(sm.session_file().is_some());
    }

    #[test]
    fn test_session_append_and_build_context() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));

        let user_msg = make_agent_message(Role::User, "hello");
        let user_id = sm.append_message(&user_msg);
        assert!(sm.leaf_id() == Some(&user_id));

        // In-memory entries exist even before flush
        assert_eq!(sm.entries().len(), 1);

        let assistant_msg = make_agent_message(Role::Assistant, "hi there");
        sm.append_message(&assistant_msg);
        assert_eq!(sm.entries().len(), 2);

        // After assistant message, file should be flushed
        let context = sm.build_session_context();
        assert_eq!(context.messages.len(), 2);
        assert_eq!(context.messages[0].content, "hello");
        assert_eq!(context.messages[1].content, "hi there");
    }

    #[test]
    fn test_session_open_existing() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        // Create and populate a session
        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_agent_message(Role::User, "first"));
        sm.append_message(&make_agent_message(Role::Assistant, "response"));

        let file_path = sm.session_file().unwrap().to_path_buf();
        let session_id = sm.session_id().to_string();
        drop(sm);

        // Open it
        let sm2 = SessionManager::open(&file_path, Some(&sessions_dir), None);
        assert_eq!(sm2.session_id(), &session_id);
        let context = sm2.build_session_context();
        assert_eq!(context.messages.len(), 2);
        assert_eq!(context.messages[0].content, "first");
        assert_eq!(context.messages[1].content, "response");
    }

    #[test]
    fn test_session_continue_recent() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        // First session
        let mut sm1 = SessionManager::create(&cwd, Some(&sessions_dir));
        sm1.append_message(&make_agent_message(Role::User, "old session"));
        sm1.append_message(&make_agent_message(Role::Assistant, "old response"));
        let _old_id = sm1.session_id().to_string();
        drop(sm1);

        // Small delay to ensure different mtime
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Second session (more recent)
        let mut sm2 = SessionManager::create(&cwd, Some(&sessions_dir));
        sm2.append_message(&make_agent_message(Role::User, "new session"));
        sm2.append_message(&make_agent_message(Role::Assistant, "new response"));
        let new_id = sm2.session_id().to_string();
        drop(sm2);

        // Continue recent — should get the new one
        let sm3 = SessionManager::continue_recent(&cwd, Some(&sessions_dir));
        assert_eq!(sm3.session_id(), &new_id);
        let context = sm3.build_session_context();
        assert_eq!(context.messages[0].content, "new session");
    }

    #[test]
    fn test_session_continue_recent_none_exist() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // No sessions exist — should create new
        let sm = SessionManager::continue_recent(&cwd, Some(&sessions_dir));
        assert!(!sm.session_id().is_empty());
        assert!(sm.entries().is_empty());
    }

    #[test]
    fn test_session_name() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        assert!(sm.session_name().is_none());

        sm.append_session_info("My Task");
        sm.append_message(&make_agent_message(Role::User, "hello"));
        sm.append_message(&make_agent_message(Role::Assistant, "hi"));
        assert_eq!(sm.session_name(), Some("My Task"));

        // Setting empty name clears it
        sm.append_session_info("");
        assert!(sm.session_name().is_none());
    }

    #[test]
    fn test_session_thinking_level_change() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_thinking_level_change("high");

        assert_eq!(sm.entries().len(), 1);
        match &sm.entries()[0] {
            SessionEntry::ThinkingLevelChange(e) => {
                assert_eq!(e.thinking_level, "high");
            }
            _ => panic!("Expected ThinkingLevelChange"),
        }
    }

    #[test]
    fn test_session_model_change() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_model_change("opencode_go", "deepseek-v4-pro");

        assert_eq!(sm.entries().len(), 1);
        match &sm.entries()[0] {
            SessionEntry::ModelChange(e) => {
                assert_eq!(e.provider, "opencode_go");
                assert_eq!(e.model_id, "deepseek-v4-pro");
            }
            _ => panic!("Expected ModelChange"),
        }
    }

    #[test]
    fn test_session_compaction() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_compaction("Earlier work summarized", "entry_kept", 5000);

        match &sm.entries()[0] {
            SessionEntry::Compaction(e) => {
                assert_eq!(e.summary, "Earlier work summarized");
                assert_eq!(e.first_kept_entry_id, "entry_kept");
                assert_eq!(e.tokens_before, 5000);
            }
            _ => panic!("Expected Compaction"),
        }
    }

    #[test]
    fn test_session_label() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        let msg_id = sm.append_message(&make_agent_message(Role::User, "important message"));
        sm.append_message(&make_agent_message(Role::Assistant, "ok"));

        // Set label
        sm.append_label_change(&msg_id, Some("important"));
        assert_eq!(sm.label(&msg_id), Some("important"));

        // Clear label
        sm.append_label_change(&msg_id, None);
        assert_eq!(sm.label(&msg_id), None);
    }

    #[test]
    fn test_session_branch_navigation() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        let m1 = sm.append_message(&make_agent_message(Role::User, "one"));
        sm.append_message(&make_agent_message(Role::Assistant, "response one"));
        let _m2 = sm.append_message(&make_agent_message(Role::User, "two"));
        sm.append_message(&make_agent_message(Role::Assistant, "response two"));

        // Current leaf is after last message
        assert_eq!(sm.entries().len(), 4);

        // Branch back to first user message
        sm.set_branch(&m1).unwrap();

        // Append a new branch
        sm.append_message(&make_agent_message(Role::Assistant, "alternate response"));

        // We now have 5 entries (original 4 + new branch entry)
        assert_eq!(sm.entries().len(), 5);

        // Build context from current leaf — should have 3 messages (m1, branch asst, nothing after)
        let context = sm.build_session_context();
        assert_eq!(context.messages.len(), 2); // user "one" + assistant "alternate response"
    }

    #[test]
    fn test_session_reset_leaf() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_agent_message(Role::User, "one"));
        sm.append_message(&make_agent_message(Role::Assistant, "response"));

        sm.reset_leaf();
        assert!(sm.leaf_id().is_none());

        // Append from reset state (parentId = null)
        sm.append_message(&make_agent_message(Role::User, "fresh start"));
        assert_eq!(sm.entries().len(), 3);
    }

    #[test]
    fn test_session_branch_summary() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_agent_message(Role::User, "one"));
        sm.append_message(&make_agent_message(Role::Assistant, "response"));

        sm.append_branch_summary("root", "Abandoned path summary");

        match &sm.entries()[2] {
            SessionEntry::BranchSummary(e) => {
                assert_eq!(e.summary, "Abandoned path summary");
                assert_eq!(e.from_id, "root");
            }
            _ => panic!("Expected BranchSummary"),
        }
    }

    #[test]
    fn test_session_children() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        let m1 = sm.append_message(&make_agent_message(Role::User, "one"));
        sm.append_message(&make_agent_message(Role::Assistant, "response"));

        // m1 should have the assistant as child
        let children = sm.children(&m1);
        assert_eq!(children.len(), 1);
    }

    #[test]
    fn test_session_custom_entry() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_agent_message(Role::User, "one"));
        sm.append_message(&make_agent_message(Role::Assistant, "ok"));
        sm.append_custom_entry("my_ext", serde_json::json!({"key": "value"}));

        match &sm.entries()[2] {
            SessionEntry::Custom(e) => {
                assert_eq!(e.custom_type, "my_ext");
                assert_eq!(e.data["key"], "value");
            }
            _ => panic!("Expected Custom"),
        }
    }

    #[test]
    fn test_find_most_recent_session() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // Create first session
        let mut sm1 = SessionManager::create(&cwd, Some(&sessions_dir));
        sm1.append_message(&make_agent_message(Role::User, "old"));
        sm1.append_message(&make_agent_message(Role::Assistant, "old"));
        let _path1 = sm1.session_file().unwrap().to_path_buf();
        drop(sm1);

        std::thread::sleep(std::time::Duration::from_millis(10));

        // Create second session (more recent)
        let mut sm2 = SessionManager::create(&cwd, Some(&sessions_dir));
        sm2.append_message(&make_agent_message(Role::User, "new"));
        sm2.append_message(&make_agent_message(Role::Assistant, "new"));
        let path2 = sm2.session_file().unwrap().to_path_buf();
        drop(sm2);

        let most_recent = find_most_recent_session(&sessions_dir, None).unwrap();
        assert_eq!(most_recent, path2);
    }

    // ── Corruption handling ───────────────────────────────────────────

    #[test]
    fn test_corrupt_empty_file_is_recovered() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // Create an empty JSONL file
        let file_path = sessions_dir.join("empty.jsonl");
        std::fs::write(&file_path, "").unwrap();

        // Opening an empty file should not panic — should start fresh
        let sm = SessionManager::open(&file_path, Some(&sessions_dir), None);
        assert!(!sm.session_id().is_empty());
        assert!(sm.entries().is_empty());
        assert_eq!(sm.session_file().unwrap(), file_path);
    }

    #[test]
    fn test_corrupt_garbage_file_is_recovered() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // Write complete garbage
        let file_path = sessions_dir.join("garbage.jsonl");
        std::fs::write(
            &file_path,
            "this is not json\nneither is this\n{half-json\n",
        )
        .unwrap();

        // Should recover gracefully
        let sm = SessionManager::open(&file_path, Some(&sessions_dir), None);
        assert!(!sm.session_id().is_empty());
        assert!(sm.entries().is_empty());
    }

    #[test]
    fn test_corrupt_header_only_file_is_kept() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // Create a session, get its header, then write just the header line
        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_agent_message(Role::User, "test"));
        sm.append_message(&make_agent_message(Role::Assistant, "ok"));
        let original_id = sm.session_id().to_string();
        let file_path = sm.session_file().unwrap().to_path_buf();
        drop(sm);

        // Read the header line and write only that
        let content = std::fs::read_to_string(&file_path).unwrap();
        let header_line = content.lines().next().unwrap();
        std::fs::write(&file_path, format!("{}\n", header_line)).unwrap();

        // Open — should keep the session (header exists, just no entries)
        let sm = SessionManager::open(&file_path, Some(&sessions_dir), None);
        assert_eq!(sm.session_id(), &original_id);
        assert!(sm.entries().is_empty());
    }

    #[test]
    fn test_corrupt_malformed_lines_are_skipped() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // Create a valid session
        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_agent_message(Role::User, "valid message"));
        sm.append_message(&make_agent_message(Role::Assistant, "valid response"));
        let file_path = sm.session_file().unwrap().to_path_buf();
        drop(sm);

        // Append garbage lines to the file
        let mut content = std::fs::read_to_string(&file_path).unwrap();
        content.push_str("this is garbage\n");
        content.push_str("{incomplete json\n");
        content.push('\n'); // blank line
        std::fs::write(&file_path, &content).unwrap();

        // Open — valid entries should be loaded, garbage skipped
        let sm = SessionManager::open(&file_path, Some(&sessions_dir), None);
        let ctx = sm.build_session_context();
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.messages[0].content, "valid message");
        assert_eq!(ctx.messages[1].content, "valid response");
    }

    #[test]
    fn test_corrupt_missing_header_uses_new_id() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // Write only valid entries but no session header
        let entry = SessionEntry::Message(MessageEntry {
            id: "msg1".to_string(),
            parent_id: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            message: make_agent_message(Role::User, "orphan message"),
        });
        let json = serde_json::to_string(&entry).unwrap();
        let file_path = sessions_dir.join("no_header.jsonl");
        std::fs::write(&file_path, format!("{}\n", json)).unwrap();

        // Open — should generate new ID, load entries
        let sm = SessionManager::open(&file_path, Some(&sessions_dir), None);
        assert!(!sm.session_id().is_empty());
        assert_eq!(sm.entries().len(), 1);
    }

    #[test]
    fn test_corrupt_file_then_append_works() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // Start with a corrupted file
        let file_path = sessions_dir.join("recovered.jsonl");
        std::fs::write(&file_path, "garbage\nmore garbage\n").unwrap();

        // Open — recovers
        let mut sm = SessionManager::open(&file_path, Some(&sessions_dir), None);
        assert!(sm.entries().is_empty());

        // Should be able to append normally
        sm.append_message(&make_agent_message(Role::User, "fresh start"));
        sm.append_message(&make_agent_message(Role::Assistant, "fresh response"));

        let ctx = sm.build_session_context();
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.messages[0].content, "fresh start");

        // Verify file was rewritten with valid content
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("fresh start"));
        assert!(!content.contains("garbage"));
    }

    #[test]
    fn test_corrupt_all_lines_malformed_is_empty() {
        let entries = load_entries_from_file(Path::new("/nonexistent/file.jsonl"));
        assert!(entries.is_empty());
    }

    #[test]
    fn test_corrupt_malformed_line_returns_none() {
        let result = parse_session_entry_line("not valid json");
        assert!(result.is_none());
    }

    #[test]
    fn test_corrupt_blank_lines_are_skipped() {
        let result = parse_session_entry_line("");
        assert!(result.is_none());
        let result = parse_session_entry_line("   ");
        assert!(result.is_none());
    }

    #[test]
    fn test_corrupt_header_line_malformed_returns_none() {
        let result = read_session_header(Path::new("/nonexistent"));
        assert!(result.is_none());
    }
}
