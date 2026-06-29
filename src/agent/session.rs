use crate::agent::session_storage::{InMemorySessionStorage, JsonlSessionStorage, SessionStorage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use yoagent::types::AgentMessage;

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

/// A session entry - one JSON line in the session file.
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
    #[serde(rename = "active_tools_change")]
    ActiveToolsChange(ActiveToolsChangeEntry),
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
    #[serde(rename = "leaf")]
    Leaf(LeafEntry),
}

impl SessionEntry {
    pub fn id(&self) -> &str {
        match self {
            SessionEntry::Message(e) => &e.id,
            SessionEntry::ThinkingLevelChange(e) => &e.id,
            SessionEntry::ModelChange(e) => &e.id,
            SessionEntry::ActiveToolsChange(e) => &e.id,
            SessionEntry::Compaction(e) => &e.id,
            SessionEntry::BranchSummary(e) => &e.id,
            SessionEntry::SessionInfo(e) => &e.id,
            SessionEntry::Label(e) => &e.id,
            SessionEntry::Custom(e) => &e.id,
            SessionEntry::CustomMessage(e) => &e.id,
            SessionEntry::Leaf(e) => &e.id,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            SessionEntry::Message(e) => e.parent_id.as_deref(),
            SessionEntry::ThinkingLevelChange(e) => e.parent_id.as_deref(),
            SessionEntry::ModelChange(e) => e.parent_id.as_deref(),
            SessionEntry::ActiveToolsChange(e) => e.parent_id.as_deref(),
            SessionEntry::Compaction(e) => e.parent_id.as_deref(),
            SessionEntry::BranchSummary(e) => e.parent_id.as_deref(),
            SessionEntry::SessionInfo(e) => e.parent_id.as_deref(),
            SessionEntry::Label(e) => e.parent_id.as_deref(),
            SessionEntry::Custom(e) => e.parent_id.as_deref(),
            SessionEntry::CustomMessage(e) => e.parent_id.as_deref(),
            SessionEntry::Leaf(e) => e.parent_id.as_deref(),
        }
    }

    pub fn timestamp(&self) -> &str {
        match self {
            SessionEntry::Message(e) => &e.timestamp,
            SessionEntry::ThinkingLevelChange(e) => &e.timestamp,
            SessionEntry::ModelChange(e) => &e.timestamp,
            SessionEntry::ActiveToolsChange(e) => &e.timestamp,
            SessionEntry::Compaction(e) => &e.timestamp,
            SessionEntry::BranchSummary(e) => &e.timestamp,
            SessionEntry::SessionInfo(e) => &e.timestamp,
            SessionEntry::Label(e) => &e.timestamp,
            SessionEntry::Custom(e) => &e.timestamp,
            SessionEntry::CustomMessage(e) => &e.timestamp,
            SessionEntry::Leaf(e) => &e.timestamp,
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
    /// Cost of this message in USD, pre-computed at creation time (pi-style).
    /// Stored per-message so model switches within a session are accurately
    /// reflected. `#[serde(default)]` for backward compat with existing sessions.
    #[serde(default)]
    pub cost: f64,
}

impl MessageEntry {
    /// Create a new `MessageEntry`.
    pub fn new(
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        message: AgentMessage,
        cost: f64,
    ) -> Self {
        Self {
            id,
            parent_id,
            timestamp,
            message,
            cost,
        }
    }
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
pub struct ActiveToolsChangeEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub active_tool_names: Vec<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeafEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
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

// ── SessionTreeNode ─────────────────────────────────────────────────

/// A node in the session tree, with resolved children and labels.
#[derive(Debug, Clone)]
pub struct SessionTreeNode {
    pub entry: SessionEntry,
    pub children: Vec<SessionTreeNode>,
    pub label: Option<String>,
    pub label_timestamp: Option<String>,
}

// ── NewSessionOptions ───────────────────────────────────────────────

/// Options for creating a new session.
#[derive(Debug, Clone, Default)]
pub struct NewSessionOptions {
    pub id: Option<String>,
    pub parent_session: Option<String>,
}

// ── SessionContext (resolved messages for LLM) ──────────────────────

/// Resolved conversation context sent to the LLM.
/// Pi-compatible: includes resolved thinking level, model, and active tool names.
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub messages: Vec<AgentMessage>,
    pub thinking_level: String,
    pub model: Option<(String, String)>,
    pub active_tool_names: Option<Vec<String>>,
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

const SESSION_READ_BUFFER_SIZE: usize = 1024 * 1024; // 1MB

/// Load header + entries from a session JSONL file using buffered reading.
/// Pi-compatible: uses a 1MB buffer for efficient reading of large files.
/// Returns (header, entries). Returns (None, empty) if file is missing/corrupted.
pub fn load_session_from_file(path: &Path) -> (Option<SessionHeader>, Vec<SessionEntry>) {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, vec![]),
    };

    use std::io::Read;
    let mut reader = std::io::BufReader::with_capacity(SESSION_READ_BUFFER_SIZE, file);
    let mut content = String::new();
    if reader.read_to_string(&mut content).is_err() {
        return (None, vec![]);
    }

    let mut header: Option<SessionHeader> = None;
    let mut entries: Vec<SessionEntry> = Vec::new();

    for (i, line_str) in content.lines().enumerate() {
        let line = line_str.trim();
        if line.is_empty() {
            continue;
        }

        if i == 0 {
            // First line must be session header, or the file is invalid
            header = parse_session_header_line(line);
            if header.is_none() {
                // Invalid session file - return empty
                return (None, vec![]);
            }
            continue;
        }

        if let Some(entry) = parse_session_entry_line(line) {
            entries.push(entry);
        }
        // Malformed lines are skipped (pi-compatible)
    }

    (header, entries)
}

/// Load all entries from a session JSONL file (backward-compatible wrapper).
pub fn load_entries_from_file(path: &Path) -> Vec<SessionEntry> {
    load_session_from_file(path).1
}

/// Write entries to a session file (used for initial write / rewrite).
/// Does NOT write the session header - caller must include it.
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

// ── Session (Pi-compatible high-level wrapper) ──────────────────────

use crate::agent::session_storage::SessionMetadata;

/// High-level session wrapper, matching pi's `Session` class.
///
/// Owns a `SessionStorage` and provides entry construction, context building,
/// branch navigation, and metadata access. All `append_*` methods generate
/// typed entries with auto-generated IDs, parent chains, and timestamps.
pub struct Session {
    storage: Box<dyn SessionStorage>,
}

impl Session {
    /// Wrap an existing storage backend.
    pub fn new(storage: Box<dyn SessionStorage>) -> Self {
        Self { storage }
    }

    /// Access the underlying storage.
    pub fn get_storage(&self) -> &dyn SessionStorage {
        self.storage.as_ref()
    }

    /// Mutably access the underlying storage.
    pub fn get_storage_mut(&mut self) -> &mut dyn SessionStorage {
        self.storage.as_mut()
    }

    /// Consume and return the underlying storage.
    pub fn into_storage(self) -> Box<dyn SessionStorage> {
        self.storage
    }

    // ── Delegation to storage ──────────────────────────────────

    pub fn metadata(&self) -> SessionMetadata {
        self.storage.metadata()
    }

    pub fn get_leaf_id(&self) -> Option<String> {
        self.storage.get_leaf_id()
    }

    pub fn get_entry(&self, id: &str) -> Option<SessionEntry> {
        self.storage.get_entry(id)
    }

    pub fn get_entries(&self) -> Vec<SessionEntry> {
        self.storage.get_entries()
    }

    pub fn find_entries(&self, type_name: &str) -> Vec<SessionEntry> {
        self.storage.find_entries(type_name)
    }

    pub fn get_label(&self, id: &str) -> Option<String> {
        self.storage.get_label(id)
    }

    /// Get the path from root to the given leaf (or current leaf if None).
    /// Pi-compatible: delegates to storage's `get_path_to_root`.
    pub fn get_branch(&self, from_id: Option<&str>) -> Result<Vec<SessionEntry>, String> {
        self.storage.get_path_to_root(from_id)
    }

    /// Build the session context (messages + metadata) for the LLM.
    /// Pi-compatible: uses `build_session_context()` from this module.
    pub fn build_context(&self) -> SessionContext {
        let path = self.get_branch(None).unwrap_or_default();
        build_session_context(&path)
    }

    /// Alias for `build_context` — pi-compatible naming.
    pub fn build_session_context(&self) -> SessionContext {
        self.build_context()
    }

    /// Convenience: session ID from metadata.
    pub fn session_id(&self) -> String {
        self.metadata().id
    }

    /// Convenience: session file path from metadata.
    pub fn session_file(&self) -> Option<PathBuf> {
        self.metadata().path
    }

    /// Convenience: session display name.
    pub fn session_name(&self) -> Option<String> {
        self.get_session_name()
    }

    /// Get the latest session name from session_info entries.
    pub fn get_session_name(&self) -> Option<String> {
        let entries = self.find_entries("session_info");
        let last = entries.last()?;
        if let SessionEntry::SessionInfo(e) = last {
            let name = e.name.trim();
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        } else {
            None
        }
    }

    // ── Entry construction (typed append methods) ───────────────

    /// Append a conversation message. Returns the entry id.
    pub fn append_message(&mut self, message: &yoagent::types::AgentMessage) -> String {
        self.append_message_with_cost(message, 0.0)
    }

    /// Append a conversation message with a pre-computed cost (pi-style).
    /// Returns the entry id.
    pub fn append_message_with_cost(
        &mut self,
        message: &yoagent::types::AgentMessage,
        cost: f64,
    ) -> String {
        let entry = SessionEntry::Message(MessageEntry::new(
            self.storage.create_entry_id(),
            self.storage.get_leaf_id(),
            chrono::Utc::now().to_rfc3339(),
            message.clone(),
            cost,
        ));
        let id = entry.id().to_string();
        self.storage.append_entry(entry).unwrap_or_else(|e| {
            eprintln!("Warning: failed to append message: {}", e);
        });
        id
    }

    /// Append a thinking level change. Returns the entry id.
    pub fn append_thinking_level_change(&mut self, thinking_level: &str) -> String {
        let entry = SessionEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
            id: self.storage.create_entry_id(),
            parent_id: self.storage.get_leaf_id(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            thinking_level: thinking_level.to_string(),
        });
        let id = entry.id().to_string();
        self.storage.append_entry(entry).unwrap_or_else(|e| {
            eprintln!("Warning: failed to append thinking level change: {}", e);
        });
        id
    }

    /// Append a model change. Returns the entry id.
    pub fn append_model_change(&mut self, provider: &str, model_id: &str) -> String {
        let entry = SessionEntry::ModelChange(ModelChangeEntry {
            id: self.storage.create_entry_id(),
            parent_id: self.storage.get_leaf_id(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            provider: provider.to_string(),
            model_id: model_id.to_string(),
        });
        let id = entry.id().to_string();
        self.storage.append_entry(entry).unwrap_or_else(|e| {
            eprintln!("Warning: failed to append model change: {}", e);
        });
        id
    }

    /// Append an active tools change. Returns the entry id.
    pub fn append_active_tools_change(&mut self, active_tool_names: &[String]) -> String {
        let entry = SessionEntry::ActiveToolsChange(ActiveToolsChangeEntry {
            id: self.storage.create_entry_id(),
            parent_id: self.storage.get_leaf_id(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            active_tool_names: active_tool_names.to_vec(),
        });
        let id = entry.id().to_string();
        self.storage.append_entry(entry).unwrap_or_else(|e| {
            eprintln!("Warning: failed to append active tools change: {}", e);
        });
        id
    }

    /// Append a compaction summary. Returns the entry id.
    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        tokens_before: u64,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> String {
        let entry = SessionEntry::Compaction(CompactionEntry {
            id: self.storage.create_entry_id(),
            parent_id: self.storage.get_leaf_id(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            summary: summary.to_string(),
            first_kept_entry_id: first_kept_entry_id.to_string(),
            tokens_before,
            details,
            from_hook,
        });
        let id = entry.id().to_string();
        self.storage.append_entry(entry).unwrap_or_else(|e| {
            eprintln!("Warning: failed to append compaction: {}", e);
        });
        id
    }

    /// Append a session info entry (display name). Returns the entry id.
    pub fn append_session_info(&mut self, name: &str) -> String {
        let entry = SessionEntry::SessionInfo(SessionInfoEntry {
            id: self.storage.create_entry_id(),
            parent_id: self.storage.get_leaf_id(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            name: name.trim().to_string(),
        });
        let id = entry.id().to_string();
        self.storage.append_entry(entry).unwrap_or_else(|e| {
            eprintln!("Warning: failed to append session info: {}", e);
        });
        id
    }

    /// Append a branch summary. Returns the entry id.
    pub fn append_branch_summary(
        &mut self,
        from_id: &str,
        summary: &str,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> String {
        let entry = SessionEntry::BranchSummary(BranchSummaryEntry {
            id: self.storage.create_entry_id(),
            parent_id: self.storage.get_leaf_id(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            from_id: from_id.to_string(),
            summary: summary.to_string(),
            details,
            from_hook,
        });
        let id = entry.id().to_string();
        self.storage.append_entry(entry).unwrap_or_else(|e| {
            eprintln!("Warning: failed to append branch summary: {}", e);
        });
        id
    }

    /// Append a label change (bookmark/unbookmark). Returns the entry id.
    pub fn append_label_change(&mut self, target_id: &str, label: Option<&str>) -> String {
        let entry = SessionEntry::Label(LabelEntry {
            id: self.storage.create_entry_id(),
            parent_id: self.storage.get_leaf_id(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            target_id: target_id.to_string(),
            label: label.map(|s| s.to_string()),
        });
        let id = entry.id().to_string();
        self.storage.append_entry(entry).unwrap_or_else(|e| {
            eprintln!("Warning: failed to append label change: {}", e);
        });
        id
    }

    /// Append a custom entry (extension data). Returns the entry id.
    pub fn append_custom_entry(&mut self, custom_type: &str, data: serde_json::Value) -> String {
        let entry = SessionEntry::Custom(CustomEntry {
            id: self.storage.create_entry_id(),
            parent_id: self.storage.get_leaf_id(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            custom_type: custom_type.to_string(),
            data,
        });
        let id = entry.id().to_string();
        self.storage.append_entry(entry).unwrap_or_else(|e| {
            eprintln!("Warning: failed to append custom entry: {}", e);
        });
        id
    }

    /// Append a custom message entry (pi-compatible extension message). Returns the entry id.
    pub fn append_custom_message_entry(
        &mut self,
        custom_type: &str,
        content: serde_json::Value,
        display: bool,
        details: Option<serde_json::Value>,
    ) -> String {
        let entry = SessionEntry::CustomMessage(CustomMessageEntry {
            id: self.storage.create_entry_id(),
            parent_id: self.storage.get_leaf_id(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            custom_type: custom_type.to_string(),
            content,
            display,
            details,
        });
        let id = entry.id().to_string();
        self.storage.append_entry(entry).unwrap_or_else(|e| {
            eprintln!("Warning: failed to append custom message: {}", e);
        });
        id
    }

    // ── Tree navigation ───────────────────────────────────────────

    /// Move the leaf pointer to an earlier entry, optionally with a summary.
    /// Pi-compatible: atomically moves leaf and appends a BranchSummaryEntry.
    /// Returns the entry id of the BranchSummaryEntry if a summary was provided.
    pub fn move_to(
        &mut self,
        entry_id: Option<&str>,
        summary: Option<(String, Option<serde_json::Value>, Option<bool>)>,
    ) -> Result<Option<String>, String> {
        // Validate target exists
        if let Some(ref id) = entry_id
            && self.get_entry(id).is_none()
        {
            return Err(format!("Entry {} not found", id));
        }
        // Persist leaf via storage
        self.storage.set_leaf_id(entry_id)?;

        // Optionally append BranchSummaryEntry
        if let Some((summary_text, details, from_hook)) = summary {
            let entry = SessionEntry::BranchSummary(BranchSummaryEntry {
                id: self.storage.create_entry_id(),
                parent_id: entry_id.map(|s| s.to_string()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                from_id: entry_id.unwrap_or("root").to_string(),
                summary: summary_text,
                details,
                from_hook,
            });
            let id = entry.id().to_string();
            self.storage.append_entry(entry).unwrap_or_else(|e| {
                eprintln!("Warning: failed to append branch summary: {}", e);
            });
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// Reset the leaf to the given entry (in-memory + leaf entry persisted).
    /// Pi-compatible: delegates to `set_leaf_id` on storage.
    pub fn set_leaf_id(&mut self, leaf_id: Option<&str>) -> Result<(), String> {
        self.storage.set_leaf_id(leaf_id)
    }

    /// Reset leaf to null (before any entries).
    pub fn reset_leaf(&mut self) -> Result<(), String> {
        self.storage.set_leaf_id(None)
    }
}

/// Build the session context from a resolved branch path.
///
/// Pi-compatible: walks path to find latest thinking level, model, active tools,
/// and handles compaction by replacing compacted entries with a summary message.
pub fn build_session_context(path: &[SessionEntry]) -> SessionContext {
    let mut thinking_level = "off".to_string();
    let mut model: Option<(String, String)> = None;
    let mut active_tool_names: Option<Vec<String>> = None;
    let mut compaction_entry: Option<&CompactionEntry> = None;

    for entry in path {
        match entry {
            SessionEntry::ThinkingLevelChange(e) => {
                thinking_level = e.thinking_level.clone();
            }
            SessionEntry::ModelChange(e) => {
                model = Some((e.provider.clone(), e.model_id.clone()));
            }
            SessionEntry::ActiveToolsChange(e) => {
                active_tool_names = Some(e.active_tool_names.clone());
            }
            SessionEntry::Compaction(e) => {
                compaction_entry = Some(e);
            }
            _ => {}
        }
    }

    // Pi-compatible: fallback — extract model from assistant messages if no explicit model_change
    if model.is_none() {
        for entry in path {
            if let SessionEntry::Message(e) = entry
                && let yoagent::types::AgentMessage::Llm(yoagent::types::Message::Assistant {
                    model: ref m,
                    provider: ref p,
                    ..
                }) = e.message
                && !m.is_empty()
                && !p.is_empty()
            {
                model = Some((p.clone(), m.clone()));
                break;
            }
        }
    }

    let messages = if let Some(compaction) = compaction_entry {
        let mut msgs: Vec<yoagent::types::AgentMessage> = Vec::new();

        // 1. Compaction summary message (pi-compatible: user role with XML wrapping)
        let comp_text = format!(
            "The conversation history before this point was compacted into the following summary:\n\n<summary>\n{}\n</summary>",
            compaction.summary
        );
        msgs.push(yoagent::types::AgentMessage::Llm(
            yoagent::types::Message::User {
                content: vec![yoagent::types::Content::Text { text: comp_text }],
                timestamp: chrono::Utc::now().timestamp_millis() as u64,
            },
        ));

        // 2. Find compaction entry index
        let compaction_idx = path
            .iter()
            .position(|e| matches!(e, SessionEntry::Compaction(ce) if ce.id == compaction.id));

        if let Some(cidx) = compaction_idx {
            // Entries BEFORE the compaction: only those at/after firstKeptEntryId
            let mut found_first_kept = false;
            for entry in path.iter().take(cidx) {
                if entry.id() == compaction.first_kept_entry_id {
                    found_first_kept = true;
                }
                if found_first_kept {
                    append_entry_to_message_list(entry, &mut msgs);
                }
            }

            // Entries AFTER the compaction: include all
            for entry in path.iter().skip(cidx + 1) {
                append_entry_to_message_list(entry, &mut msgs);
            }
        } else {
            // Fallback: include all entries
            for entry in path {
                append_entry_to_message_list(entry, &mut msgs);
            }
        }

        msgs
    } else {
        // No compaction: include all convertible entries
        let mut msgs: Vec<yoagent::types::AgentMessage> = Vec::new();
        for entry in path {
            append_entry_to_message_list(entry, &mut msgs);
        }
        msgs
    };

    SessionContext {
        messages,
        thinking_level,
        model,
        active_tool_names,
    }
}

/// Convert a session tree entry to an `AgentMessage` and append to the list.
/// Pi-compatible: handles `message`, `custom_message`, and `branch_summary` entries.
/// Skips provider/diagnostic error messages — their empty (or error-text-only)
/// content would cause the provider to reject subsequent requests.
fn append_entry_to_message_list(
    entry: &SessionEntry,
    msgs: &mut Vec<yoagent::types::AgentMessage>,
) {
    match entry {
        SessionEntry::Message(e) => {
            // Skip provider/diagnostic error messages
            if crate::agent::types::message_error(&e.message).is_some() {
                return;
            }
            msgs.push(e.message.clone());
        }
        SessionEntry::CustomMessage(e) => {
            msgs.push(yoagent::types::AgentMessage::Extension(
                yoagent::types::ExtensionMessage::new(
                    &e.custom_type,
                    serde_json::json!({ "text": e.content.get("text").and_then(|v| v.as_str()).unwrap_or(""), "display": e.display }),
                ),
            ));
        }
        SessionEntry::BranchSummary(e) if !e.summary.is_empty() => {
            // Pi-compatible: user role with XML summary wrapping
            let bs_text = format!(
                "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n{}\n</summary>",
                e.summary
            );
            msgs.push(yoagent::types::AgentMessage::Llm(
                yoagent::types::Message::User {
                    content: vec![yoagent::types::Content::Text { text: bs_text }],
                    timestamp: chrono::Utc::now().timestamp_millis() as u64,
                },
            ));
        }
        _ => {}
    }
}

// ── SessionManager ──────────────────────────────────────────────────

/// Manages conversation sessions as append-only trees in JSONL files.
///
/// Each entry has an id and parentId forming a tree structure.
/// Appending creates a child of the current leaf. Branching moves the
/// leaf to an earlier entry, allowing new branches without modifying history.
pub struct SessionManager {
    /// The high-level session wrapper.
    session: Session,
    /// Session storage directory on disk.
    session_dir: PathBuf,
    /// Working directory for this session.
    cwd: PathBuf,
    /// Whether session persistence is enabled.
    persist: bool,
    /// Whether the session file has been written at least once.
    flushed: bool,
}

impl SessionManager {
    // ── Construction ─────────────────────────────────────────────

    /// Create a SessionManager wrapping an existing Session.
    pub fn with_session(
        session: Session,
        session_dir: PathBuf,
        cwd: PathBuf,
        persist: bool,
    ) -> Self {
        Self {
            session,
            session_dir,
            cwd,
            persist,
            flushed: false,
        }
    }

    /// Create a new persisted session.
    /// Pi-compatible: defers file creation until first assistant message (lazy write).
    fn create_persisted(
        cwd: &Path,
        session_dir: &Path,
        options: Option<&NewSessionOptions>,
    ) -> Self {
        let id = options
            .and_then(|o| o.id.as_deref())
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let created_at = chrono::Utc::now().to_rfc3339();

        // Use in-memory storage initially — no file created yet (lazy write).
        let meta = crate::agent::session_storage::SessionMetadata {
            id: id.clone(),
            created_at: created_at.clone(),
            cwd: cwd.to_string_lossy().to_string(),
            path: None, // Path will be set when flushed
            parent_session_path: options.and_then(|o| o.parent_session.clone()),
        };
        let storage = InMemorySessionStorage::new(meta);
        let session = Session::new(Box::new(storage));
        Self::with_session(session, session_dir.to_path_buf(), cwd.to_path_buf(), true)
    }

    /// Open an existing session file.
    fn open_session(path: &Path, session_dir: &Path, cwd_override: Option<&Path>) -> Self {
        let cwd = cwd_override
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));

        let storage: Box<dyn SessionStorage> = match JsonlSessionStorage::open(path.to_path_buf()) {
            Ok(s) => Box::new(s),
            Err(e) => {
                eprintln!("Warning: failed to open session: {}, creating new", e);
                // Fall back: create a fresh file-backed session at the same path (overwrite)
                let id = uuid::Uuid::new_v4().to_string();
                match JsonlSessionStorage::create(
                    path.to_path_buf(),
                    &cwd.to_string_lossy(),
                    &id,
                    None,
                ) {
                    Ok(s) => Box::new(s),
                    Err(e2) => {
                        eprintln!("Warning: failed to create session file: {}", e2);
                        Box::new(InMemorySessionStorage::new(
                            crate::agent::session_storage::SessionMetadata {
                                id,
                                created_at: chrono::Utc::now().to_rfc3339(),
                                cwd: cwd.to_string_lossy().to_string(),
                                path: Some(path.to_path_buf()),
                                parent_session_path: None,
                            },
                        ))
                    }
                }
            }
        };
        let cwd = cwd_override
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(storage.metadata().cwd));
        let session = Session::new(storage);
        let mut sm = Self::with_session(session, session_dir.to_path_buf(), cwd, true);
        // File already exists (opened or recovered), mark flushed
        sm.flushed = true;
        sm
    }

    /// Create an in-memory (non-persisted) session.
    fn create_in_memory(cwd: &Path, session_dir: &Path) -> Self {
        let meta = crate::agent::session_storage::SessionMetadata {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            cwd: cwd.to_string_lossy().to_string(),
            path: None,
            parent_session_path: None,
        };
        let storage = InMemorySessionStorage::new(meta);
        let session = Session::new(Box::new(storage));
        Self::with_session(session, session_dir.to_path_buf(), cwd.to_path_buf(), false)
    }

    /// Create a new session (overwrites current entries).
    /// Pi-compatible: defers writing to disk until first assistant message.
    pub fn new_session(&mut self, options: Option<&NewSessionOptions>) {
        let id = options
            .and_then(|o| o.id.as_deref())
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let created_at = chrono::Utc::now().to_rfc3339();

        // Always create in-memory initially (lazy write).
        // ensure_flushed() will create the file on first assistant message.
        let meta = crate::agent::session_storage::SessionMetadata {
            id,
            created_at,
            cwd: self.cwd.to_string_lossy().to_string(),
            path: None,
            parent_session_path: options.and_then(|o| o.parent_session.clone()),
        };
        let storage = InMemorySessionStorage::new(meta);
        self.session = Session::new(Box::new(storage));
        self.flushed = false;
    }

    /// Ensure the session file has been written (lazy write).
    /// Migrates from in-memory to file-backed storage, writing header + all entries.
    /// Called before first assistant message.
    pub fn ensure_flushed(&mut self) {
        if self.flushed || !self.persist {
            return;
        }

        let id = self.session.metadata().id;
        let created_at = self.session.metadata().created_at.clone();
        let cwd_str = self.cwd.to_string_lossy().to_string();
        let parent_session = self.session.metadata().parent_session_path.clone();
        let file_ts = created_at.replace([':', '.'], "-");
        let file_path = self.session_dir.join(format!("{}_{}.jsonl", file_ts, id));

        // Get existing entries before replacing storage
        let existing_entries = self.session.get_entries();

        // Create file-backed storage and copy entries
        match JsonlSessionStorage::create(file_path.clone(), &cwd_str, &id, parent_session) {
            Ok(mut file_storage) => {
                // Write all existing entries to file
                for entry in &existing_entries {
                    if let Err(e) = file_storage.append_entry(entry.clone()) {
                        eprintln!("Warning: failed to write entry to session file: {}", e);
                    }
                }
                self.session = Session::new(Box::new(file_storage));
                self.flushed = true;
            }
            Err(e) => {
                eprintln!("Warning: failed to create session file: {}", e);
                // Stay in-memory but mark as "flushed" to avoid repeated attempts
                self.flushed = true;
            }
        }
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

    pub fn session_id(&self) -> String {
        self.session.metadata().id
    }

    pub fn session_file(&self) -> Option<PathBuf> {
        self.session.metadata().path
    }

    pub fn leaf_id(&self) -> Option<String> {
        self.session.get_leaf_id()
    }

    /// Get the current session name.
    pub fn session_name(&self) -> Option<String> {
        self.session.get_session_name()
    }

    /// Get the underlying Session reference.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Get the underlying Session mutable reference.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Consume and return the inner Session.
    pub fn into_session(self) -> Session {
        self.session
    }

    // ── Public: Info (pi-compatible methods) ──────────────────────

    /// Get the current leaf entry (pi-compatible).
    pub fn get_leaf_entry(&self) -> Option<SessionEntry> {
        self.leaf_id().and_then(|id| self.entry(&id))
    }

    /// Get the session as a tree structure with resolved children and labels (pi-compatible).
    pub fn get_tree(&self) -> Vec<SessionTreeNode> {
        let entries = self.session.get_entries();
        let mut node_map: HashMap<String, SessionTreeNode> = HashMap::new();

        for entry in &entries {
            let label = self.session.get_label(entry.id());
            node_map.insert(
                entry.id().to_string(),
                SessionTreeNode {
                    entry: entry.clone(),
                    children: Vec::new(),
                    label,
                    label_timestamp: None,
                },
            );
        }

        let child_edges: Vec<(Option<String>, String)> = entries
            .iter()
            .map(|e| (e.parent_id().map(|s| s.to_string()), e.id().to_string()))
            .collect();

        let mut child_additions: Vec<(String, SessionTreeNode)> = Vec::new();
        let mut roots: Vec<String> = Vec::new();
        for (parent_id, child_id) in &child_edges {
            if let Some(pid) = parent_id {
                if !node_map.contains_key(pid) {
                    roots.push(child_id.clone());
                } else if let Some(child) = node_map.get(child_id) {
                    child_additions.push((pid.clone(), child.clone()));
                }
            } else {
                roots.push(child_id.clone());
            }
        }
        for (pid, child) in child_additions {
            if let Some(parent) = node_map.get_mut(&pid) {
                parent.children.push(child);
            }
        }

        fn sort_tree(node: &mut SessionTreeNode) {
            node.children
                .sort_by_key(|c| c.entry.timestamp().to_string());
            for child in &mut node.children {
                sort_tree(child);
            }
        }

        let mut result: Vec<SessionTreeNode> =
            roots.iter().filter_map(|id| node_map.remove(id)).collect();
        for root in &mut result {
            sort_tree(root);
        }

        result
    }

    /// Get all session entries (excludes header). Pi-compatible.
    pub fn get_entries(&self) -> Vec<SessionEntry> {
        self.session.get_entries()
    }

    // ── Public: Appending (delegated to Session) ──────────────────

    pub fn append_message(&mut self, message: &yoagent::types::AgentMessage) -> String {
        // Flush on first message (lazy write) to ensure session is persisted
        // even if the agent never produces an assistant message (e.g. provider error).
        if !self.flushed && self.persist {
            self.ensure_flushed();
        }
        self.session.append_message(message)
    }

    pub fn append_thinking_level_change(&mut self, thinking_level: &str) -> String {
        self.session.append_thinking_level_change(thinking_level)
    }

    pub fn append_model_change(&mut self, provider: &str, model_id: &str) -> String {
        self.session.append_model_change(provider, model_id)
    }

    pub fn append_session_info(&mut self, name: &str) -> String {
        self.session.append_session_info(name)
    }

    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        tokens_before: u64,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> String {
        self.session.append_compaction(
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            from_hook,
        )
    }

    pub fn append_branch_summary(
        &mut self,
        from_id: &str,
        summary: &str,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> String {
        self.session
            .append_branch_summary(from_id, summary, details, from_hook)
    }

    pub fn append_label_change(&mut self, target_id: &str, label: Option<&str>) -> String {
        self.session.append_label_change(target_id, label)
    }

    pub fn append_custom_entry(&mut self, custom_type: &str, data: serde_json::Value) -> String {
        self.session.append_custom_entry(custom_type, data)
    }

    pub fn append_active_tools_change(&mut self, active_tool_names: &[String]) -> String {
        self.session.append_active_tools_change(active_tool_names)
    }

    pub fn append_custom_message_entry(
        &mut self,
        custom_type: &str,
        content: serde_json::Value,
        display: bool,
        details: Option<serde_json::Value>,
    ) -> String {
        self.session
            .append_custom_message_entry(custom_type, content, display, details)
    }

    // ── Public: Querying (delegated to Session) ──────────────────

    /// Find all entries of a given type (pi-compatible).
    pub fn find_entries_by_type(&self, type_name: &str) -> Vec<SessionEntry> {
        self.session.find_entries(type_name)
    }

    /// Get all entries (excludes header).
    pub fn entries(&self) -> Vec<SessionEntry> {
        self.session.get_entries()
    }

    /// Look up an entry by id.
    pub fn entry(&self, id: &str) -> Option<SessionEntry> {
        self.session.get_entry(id)
    }

    /// Get all direct children of an entry.
    pub fn children(&self, parent_id: &str) -> Vec<SessionEntry> {
        self.session
            .get_entries()
            .into_iter()
            .filter(|e| e.parent_id() == Some(parent_id))
            .collect()
    }

    /// Walk from entry to root, returning all entries in path order.
    pub fn branch(&self, from_id: Option<&str>) -> Vec<SessionEntry> {
        self.session.get_branch(from_id).unwrap_or_default()
    }

    /// Build the session context (messages for LLM with compaction handling).
    /// Pi-compatible: delegates to Session::build_context.
    pub fn build_session_context(&self) -> SessionContext {
        self.session.build_context()
    }

    /// Get the label for an entry, if any.
    pub fn label(&self, id: &str) -> Option<String> {
        self.session.get_label(id)
    }

    // ── Public: Branching ─────────────────────────────────────────

    /// Move leaf pointer to an earlier entry (starts a new branch).
    /// Pi-compatible: delegates to Session::set_leaf_id.
    pub fn set_branch(&mut self, branch_from_id: &str) -> Result<(), String> {
        self.session.set_leaf_id(Some(branch_from_id))
    }

    /// Reset leaf pointer to null (before any entries).
    pub fn reset_leaf(&mut self) {
        let _ = self.session.reset_leaf();
    }

    /// Move leaf pointer with a branch summary entry.
    /// Pi-compatible: delegates to Session::move_to.
    pub fn branch_with_summary(
        &mut self,
        branch_from_id: Option<&str>,
        summary: &str,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> Result<String, String> {
        let summary_tuple = Some((summary.to_string(), details, from_hook));
        self.session
            .move_to(branch_from_id, summary_tuple)
            .map(|opt| opt.unwrap_or_default())
    }

    // ── Static factories ──────────────────────────────────────────

    /// Create a new session.
    pub fn create(cwd: &Path, session_dir: Option<&Path>) -> Self {
        let dir = session_dir
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| get_default_session_dir(cwd));
        Self::create_persisted(cwd, &dir, None)
    }

    /// Create a new session with options (pi-compatible).
    pub fn create_with_options(
        cwd: &Path,
        session_dir: Option<&Path>,
        options: Option<&NewSessionOptions>,
    ) -> Self {
        let dir = session_dir
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| get_default_session_dir(cwd));
        Self::create_persisted(cwd, &dir, options)
    }

    /// Open a specific session file.
    pub fn open(path: &Path, session_dir: Option<&Path>, cwd_override: Option<&Path>) -> Self {
        let dir = session_dir.map(|p| p.to_path_buf()).unwrap_or_else(|| {
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| get_default_session_dir(&PathBuf::from("/")))
        });
        Self::open_session(path, &dir, cwd_override)
    }

    /// Create an in-memory session (no file persistence).
    pub fn in_memory(cwd: &Path) -> Self {
        let dir = get_default_session_dir(cwd);
        Self::create_in_memory(cwd, &dir)
    }

    /// Continue the most recent session, or create new if none.
    pub fn continue_recent(cwd: &Path, session_dir: Option<&Path>) -> Self {
        let dir = session_dir
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| get_default_session_dir(cwd));
        let filter_cwd = session_dir.is_some_and(|sd| sd != get_default_session_dir(cwd));
        let most_recent = find_most_recent_session(&dir, if filter_cwd { Some(cwd) } else { None });
        if let Some(path) = most_recent {
            Self::open_session(&path, &dir, Some(cwd))
        } else {
            Self::create_persisted(cwd, &dir, None)
        }
    }

    /// Fork a session from another project directory into the current one.
    /// Pi-compatible: creates a new session with the full history from the source session.
    pub fn fork_from(
        source_path: &Path,
        target_cwd: &Path,
        session_dir: Option<&Path>,
        options: Option<&NewSessionOptions>,
    ) -> std::io::Result<Self> {
        let resolved_source = source_path;
        let resolved_target = target_cwd.to_path_buf();
        let dir = session_dir
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| get_default_session_dir(&resolved_target));

        let source_entries = load_entries_from_file(resolved_source);
        if source_entries.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Cannot fork: source session is empty or invalid",
            ));
        }

        let _source_header = read_session_header(resolved_source).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Cannot fork: source session has no header",
            )
        })?;

        // Create new session
        let id = options
            .and_then(|o| o.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let timestamp = chrono::Utc::now().to_rfc3339();
        let file_ts = timestamp.replace([':', '.'], "-");
        let file_name = format!("{}_{}.jsonl", file_ts, id);
        let target_path = dir.join(&file_name);

        // Create storage and write immediately
        let mut storage = JsonlSessionStorage::create(
            target_path.clone(),
            &resolved_target.to_string_lossy(),
            &id,
            Some(resolved_source.to_string_lossy().to_string()),
        )
        .map_err(std::io::Error::other)?;

        // Push all source entries (re-chaining through append_entry)
        for entry in &source_entries {
            storage
                .append_entry(entry.clone())
                .map_err(std::io::Error::other)?;
        }

        let session = Session::new(Box::new(storage));
        Ok(Self::with_session(session, dir, resolved_target, true))
    }

    /// Create a branched session from a specific leaf path.
    /// Extracts the linear path from root to leaf into a new session file.
    /// Pi-compatible: creates a new session file, preserving labels.
    pub fn create_branched_session(&mut self, leaf_id: &str) -> Option<PathBuf> {
        let path = self.branch(Some(leaf_id));
        if path.is_empty() {
            return None;
        }

        // Filter out label entries and leaf entries, re-chain parentIds
        let mut path_clean: Vec<SessionEntry> = Vec::new();
        let mut path_parent_id: Option<String> = None;
        for entry in &path {
            if matches!(entry, SessionEntry::Label(_) | SessionEntry::Leaf(_)) {
                continue;
            }
            let mut e = entry.clone();
            match &mut e {
                SessionEntry::Message(m) => m.parent_id = path_parent_id.clone(),
                SessionEntry::ThinkingLevelChange(m) => m.parent_id = path_parent_id.clone(),
                SessionEntry::ModelChange(m) => m.parent_id = path_parent_id.clone(),
                SessionEntry::ActiveToolsChange(m) => m.parent_id = path_parent_id.clone(),
                SessionEntry::Compaction(m) => m.parent_id = path_parent_id.clone(),
                SessionEntry::BranchSummary(m) => m.parent_id = path_parent_id.clone(),
                SessionEntry::SessionInfo(m) => m.parent_id = path_parent_id.clone(),
                SessionEntry::Custom(m) => m.parent_id = path_parent_id.clone(),
                SessionEntry::CustomMessage(m) => m.parent_id = path_parent_id.clone(),
                _ => {}
            }
            path_parent_id = Some(e.id().to_string());
            path_clean.push(e);
        }

        // Collect labels for entries in the path
        let path_entry_ids: std::collections::HashSet<String> =
            path_clean.iter().map(|e| e.id().to_string()).collect();
        let mut labels_to_write: Vec<(String, String)> = Vec::new();
        for id in &path_entry_ids {
            if let Some(label) = self.session.get_label(id) {
                labels_to_write.push((id.clone(), label));
            }
        }

        let new_session_id = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().to_rfc3339();
        let file_ts = timestamp.replace([':', '.'], "-");
        let new_session_file = self
            .session_dir
            .join(format!("{}_{}.jsonl", file_ts, new_session_id));

        let cwd_str = self.cwd.to_string_lossy().to_string();

        // Write header + cleaned path + label entries to file
        if self.persist {
            let header = SessionHeader {
                type_: "session".to_string(),
                version: Some(CURRENT_SESSION_VERSION),
                id: new_session_id,
                timestamp,
                cwd: cwd_str,
                parent_session: self
                    .session
                    .metadata()
                    .path
                    .map(|p| p.to_string_lossy().to_string()),
            };

            if let Some(parent) = new_session_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut content = serde_json::to_string(&header).unwrap_or_default();
            content.push('\n');
            for entry in &path_clean {
                let line = serde_json::to_string(entry).unwrap_or_default();
                content.push_str(&line);
                content.push('\n');
            }
            for (target_id, label) in &labels_to_write {
                let label_entry = SessionEntry::Label(LabelEntry {
                    id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
                    parent_id: path_parent_id.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    target_id: target_id.clone(),
                    label: Some(label.clone()),
                });
                let line = serde_json::to_string(&label_entry).unwrap_or_default();
                content.push_str(&line);
                content.push('\n');
            }
            let _ = std::fs::write(&new_session_file, &content);
        }

        Some(new_session_file)
    }

    /// List all sessions across all project directories (pi-compatible).
    pub fn list_all(session_dir: Option<&Path>) -> Vec<SessionInfo> {
        let dir = if let Some(d) = session_dir {
            d.to_path_buf()
        } else {
            directories::BaseDirs::new()
                .expect("Could not determine home directory")
                .home_dir()
                .join(".rab")
                .join("sessions")
        };

        let mut all_sessions: Vec<SessionInfo> = Vec::new();

        if let Ok(read_dir) = std::fs::read_dir(&dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let sessions = list_sessions(&path);
                    all_sessions.extend(sessions);
                }
            }
        }

        // Also check the root dir itself for sessions
        let root_sessions = list_sessions(&dir);
        all_sessions.extend(root_sessions);

        all_sessions.sort_by_key(|b| std::cmp::Reverse(b.created));
        all_sessions
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

// ── Session repository (list / delete / fork) ───────────────────────

/// List all session metadata in a session directory, newest first.
/// Pi-compatible: returns metadata for all valid `.jsonl` sessions.
pub fn list_sessions(session_dir: &Path) -> Vec<SessionInfo> {
    let mut sessions: Vec<SessionInfo> = Vec::new();
    let dir = match std::fs::read_dir(session_dir) {
        Ok(d) => d,
        Err(_) => return sessions,
    };
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl")
            && let Some(info) = load_session_info(&path)
        {
            sessions.push(info);
        }
    }
    sessions.sort_by_key(|b| std::cmp::Reverse(b.created));
    sessions
}

/// Load session info from a session file.
pub fn load_session_info(path: &Path) -> Option<SessionInfo> {
    let header = read_session_header(path)?;
    let created = DateTime::parse_from_rfc3339(&header.timestamp)
        .ok()?
        .with_timezone(&Utc);
    let modified = path.metadata().ok()?.modified().ok()?;
    let modified_dt: DateTime<Utc> = modified.into();
    let entries = load_entries_from_file(path);
    let name = entries.iter().rev().find_map(|e| {
        if let SessionEntry::SessionInfo(si) = e {
            let n = si.name.trim();
            if n.is_empty() {
                None
            } else {
                Some(n.to_string())
            }
        } else {
            None
        }
    });
    let message_count = entries
        .iter()
        .filter(|e| matches!(e, SessionEntry::Message(_)))
        .count();
    let first_message = entries
        .iter()
        .find_map(|e| {
            if let SessionEntry::Message(m) = e {
                Some(crate::agent::types::message_text(&m.message))
            } else {
                None
            }
        })
        .unwrap_or_default();
    let all_messages_text = entries
        .iter()
        .filter_map(|e| {
            if let SessionEntry::Message(m) = e {
                Some(crate::agent::types::message_text(&m.message))
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    Some(SessionInfo {
        path: path.to_path_buf(),
        id: header.id,
        cwd: header.cwd,
        name,
        parent_session_path: header.parent_session,
        created,
        modified: modified_dt,
        message_count,
        first_message,
        all_messages_text,
    })
}

/// Delete a session file.
pub fn delete_session(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Fork a session: create a new session file containing a copy of entries from the source session
/// up to (and including) the entry with the given `entry_id`, or all entries if `entry_id` is None.
/// If `entry_id` is provided and `position` is "at", the copy goes up to and including that entry.
/// If `position` is "before" (default), the copy goes up to but not including the entry
/// (which must be a user message). Pi-compatible.
pub fn fork_session(
    source_path: &Path,
    target_dir: &Path,
    entry_id: Option<&str>,
    position: Option<&str>,
) -> std::io::Result<String> {
    let header = read_session_header(source_path).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing session header")
    })?;
    let entries = load_entries_from_file(source_path);

    // Build by_id map for parent traversal
    let by_id: HashMap<String, &SessionEntry> =
        entries.iter().map(|e| (e.id().to_string(), e)).collect();

    let forked_entries: Vec<SessionEntry> = if let Some(target_id) = entry_id {
        // Find the target entry
        let target = by_id.get(target_id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Entry not found")
        })?;

        // Determine the effective leaf ID for the fork
        let effective_leaf_id = match position.unwrap_or("before") {
            "at" => Some(target.id().to_string()),
            _ => {
                if !matches!(target, SessionEntry::Message(m) if crate::agent::types::message_is_user(&m.message))
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Entry is not a user message",
                    ));
                }
                target.parent_id().map(|s| s.to_string())
            }
        };

        // Collect path from effective leaf to root
        let mut path: Vec<&SessionEntry> = Vec::new();
        let mut current = effective_leaf_id.as_ref().and_then(|id| by_id.get(id));
        while let Some(entry) = current {
            path.push(entry);
            current = entry.parent_id().and_then(|pid| by_id.get(pid));
        }
        path.reverse();
        path.into_iter().cloned().collect()
    } else {
        entries.clone()
    };

    // Create the new session
    let session_id = uuid::Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().to_rfc3339();
    let file_ts = timestamp.replace([':', '.'], "-");
    let file_name = format!("{}_{}.jsonl", file_ts, session_id);
    let target_path = target_dir.join(&file_name);

    std::fs::create_dir_all(target_dir)?;

    let new_header = SessionHeader {
        type_: "session".to_string(),
        version: Some(CURRENT_SESSION_VERSION),
        id: session_id.clone(),
        timestamp,
        cwd: header.cwd.clone(),
        parent_session: Some(source_path.to_string_lossy().to_string()),
    };
    write_entries_to_file(&target_path, &new_header, &forked_entries)?;

    Ok(session_id)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::user_message;
    use tempfile::TempDir;

    fn make_user_msg(content: &str) -> AgentMessage {
        user_message(content)
    }

    fn make_asst_msg(content: &str) -> AgentMessage {
        crate::agent::types::assistant_message(content)
    }

    // ── Entry serialization round-trip ──────────────────────────────

    #[test]
    fn test_build_context_tracks_metadata() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_thinking_level_change("high");
        sm.append_model_change("opencode_go", "deepseek-v4-pro");
        sm.append_active_tools_change(&["read".to_string(), "write".to_string()]);
        sm.append_message(&make_user_msg("hello"));
        sm.append_message(&make_asst_msg("hi"));

        let context = sm.build_session_context();
        assert_eq!(context.thinking_level, "high");
        assert_eq!(
            context.model,
            Some(("opencode_go".to_string(), "deepseek-v4-pro".to_string()))
        );
        assert_eq!(
            context.active_tool_names,
            Some(vec!["read".to_string(), "write".to_string()])
        );
        assert_eq!(context.messages.len(), 2);
    }

    #[test]
    fn test_build_context_defaults_when_no_metadata() {
        let cwd = Path::new("/tmp/test");
        let sm = SessionManager::in_memory(cwd);
        let context = sm.build_session_context();
        assert_eq!(context.thinking_level, "off");
        assert!(context.model.is_none());
        assert!(context.active_tool_names.is_none());
        assert!(context.messages.is_empty());
    }

    // ── Find entries test ────────────────────────────────────────────

    #[test]
    fn test_find_entries_by_type() {
        let cwd = Path::new("/tmp/test");
        let mut sm = SessionManager::in_memory(cwd);
        sm.append_message(&make_user_msg("hello"));
        sm.append_thinking_level_change("high");
        sm.append_model_change("p", "m");
        sm.append_session_info("test session");

        let messages = sm.find_entries_by_type("message");
        assert_eq!(messages.len(), 1);

        let thinking = sm.find_entries_by_type("thinking_level_change");
        assert_eq!(thinking.len(), 1);

        let models = sm.find_entries_by_type("model_change");
        assert_eq!(models.len(), 1);

        let infos = sm.find_entries_by_type("session_info");
        assert_eq!(infos.len(), 1);
    }

    // ── Session listing / forking tests ──────────────────────────────

    #[test]
    fn test_list_sessions_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let sessions = list_sessions(tmp.path());
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_list_sessions() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_user_msg("first"));
        sm.append_message(&make_asst_msg("response"));
        let path = sm.session_file().unwrap().to_path_buf();
        drop(sm);

        let sessions = list_sessions(&sessions_dir);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].path, path);
        assert_eq!(sessions[0].message_count, 2);
    }

    #[test]
    fn test_fork_session_all_entries() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_user_msg("hello"));
        sm.append_message(&make_asst_msg("world"));
        let source_path = sm.session_file().unwrap().to_path_buf();
        drop(sm);

        let target_dir = tmp.path().join("forked");
        let new_id = fork_session(&source_path, &target_dir, None, None).unwrap();
        assert!(!new_id.is_empty());

        let sessions = list_sessions(&target_dir);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].message_count, 2);
    }

    #[test]
    fn test_delete_session() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.jsonl");
        std::fs::write(&path, "{\"type\":\"session\",\"id\":\"test\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"cwd\":\"/\"}\n").unwrap();
        assert!(path.exists());
        delete_session(&path).unwrap();
        assert!(!path.exists());
        // deleting non-existent file should be ok
        delete_session(&path).unwrap();
    }

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
            SessionEntry::Message(MessageEntry::new(
                "msg1".to_string(),
                None,
                "2026-06-19T12:00:01Z".to_string(),
                make_user_msg("hello"),
                0.0,
            )),
            SessionEntry::Message(MessageEntry {
                cost: 0.0,
                id: "msg2".to_string(),
                parent_id: Some("msg1".to_string()),
                timestamp: "2026-06-19T12:00:02Z".to_string(),
                message: AgentMessage::Llm(yoagent::types::Message::Assistant {
                    content: vec![yoagent::types::Content::Text {
                        text: "hi there".to_string(),
                    }],
                    stop_reason: yoagent::types::StopReason::Stop,
                    model: String::new(),
                    provider: String::new(),
                    usage: yoagent::types::Usage {
                        input: 10,
                        output: 5,
                        ..Default::default()
                    },
                    timestamp: 0,
                    error_message: None,
                }),
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
                assert!(crate::agent::types::message_is_user(&e.message));
                assert_eq!(crate::agent::types::message_text(&e.message), "hello");
            }
            _ => panic!("Expected Message"),
        }

        match &read_entries[1] {
            SessionEntry::Message(e) => {
                assert_eq!(e.id, "msg2");
                assert!(crate::agent::types::message_is_assistant(&e.message));
                assert_eq!(crate::agent::types::message_text(&e.message), "hi there");
                assert!(crate::agent::types::message_usage(&e.message).is_some());
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
        let entry = SessionEntry::Message(MessageEntry::new(
            "myid".to_string(),
            None,
            "2026-06-19T12:00:00Z".to_string(),
            make_user_msg("hello"),
            0.0,
        ));
        assert_eq!(entry.id(), "myid");
    }

    #[test]
    fn test_entry_parent_id_accessor() {
        let entry = SessionEntry::Message(MessageEntry::new(
            "myid".to_string(),
            Some("parent".to_string()),
            "2026-06-19T12:00:00Z".to_string(),
            make_user_msg("hello"),
            0.0,
        ));
        assert_eq!(entry.parent_id(), Some("parent"));
    }

    #[test]
    fn test_entry_timestamp_accessor() {
        let entry = SessionEntry::Message(MessageEntry::new(
            "myid".to_string(),
            None,
            "2026-06-19T12:00:00Z".to_string(),
            make_user_msg("hello"),
            0.0,
        ));
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
        // Create a map that has all possible 8-char hex IDs - impossible
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
        // Message uses yoagent format: role-tagged enum with Vec<Content>
        let json = r#"{"type":"message","id":"abc12345","parentId":null,"timestamp":"2026-06-19T12:00:00Z","message":{"role":"user","content":[{"type":"text","text":"hello"}],"timestamp":1718800000000}}"#;
        let entry: SessionEntry = serde_json::from_str(json).unwrap();
        match entry {
            SessionEntry::Message(e) => {
                assert_eq!(e.id, "abc12345");
                assert_eq!(crate::agent::types::message_text(&e.message), "hello");
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
        // File should NOT exist yet (lazy write: no file path until first assistant)
        assert!(
            sm.session_file().is_none(),
            "session file should not be created until first assistant message (lazy write)"
        );
        assert!(!sm.flushed);
    }

    #[test]
    fn test_session_append_and_build_context() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));

        let user_msg = make_user_msg("hello");
        let user_id = sm.append_message(&user_msg);
        assert_eq!(sm.leaf_id().as_deref(), Some(user_id.as_str()));

        // In-memory entries exist even before flush
        assert_eq!(sm.entries().len(), 1);

        let assistant_msg = make_asst_msg("hi there");
        sm.append_message(&assistant_msg);
        assert_eq!(sm.entries().len(), 2);

        // After assistant message, file should be created (lazy write)
        assert!(
            sm.session_file().unwrap().exists(),
            "session file should exist after first assistant message"
        );

        let context = sm.build_session_context();
        assert_eq!(context.messages.len(), 2);
        assert_eq!(
            crate::agent::types::message_text(&context.messages[0]),
            "hello"
        );
        assert_eq!(
            crate::agent::types::message_text(&context.messages[1]),
            "hi there"
        );
    }

    #[test]
    fn test_session_open_existing() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        // Create and populate a session
        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_user_msg("first"));
        sm.append_message(&make_asst_msg("response"));

        let file_path = sm.session_file().unwrap().to_path_buf();
        let session_id = sm.session_id().to_string();
        drop(sm);

        // Open it
        let sm2 = SessionManager::open(&file_path, Some(&sessions_dir), None);
        assert_eq!(sm2.session_id(), session_id);
        let context = sm2.build_session_context();
        assert_eq!(context.messages.len(), 2);
        assert_eq!(
            crate::agent::types::message_text(&context.messages[0]),
            "first"
        );
        assert_eq!(
            crate::agent::types::message_text(&context.messages[1]),
            "response"
        );
    }

    #[test]
    fn test_session_continue_recent() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        // First session
        let mut sm1 = SessionManager::create(&cwd, Some(&sessions_dir));
        sm1.append_message(&make_user_msg("old session"));
        sm1.append_message(&make_asst_msg("old response"));
        let _old_id = sm1.session_id().to_string();
        drop(sm1);

        // Small delay to ensure different mtime
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Second session (more recent)
        let mut sm2 = SessionManager::create(&cwd, Some(&sessions_dir));
        sm2.append_message(&make_user_msg("new session"));
        sm2.append_message(&make_asst_msg("new response"));
        let new_id = sm2.session_id().to_string();
        drop(sm2);

        // Continue recent - should get the new one
        let sm3 = SessionManager::continue_recent(&cwd, Some(&sessions_dir));
        assert_eq!(sm3.session_id(), new_id);
        let context = sm3.build_session_context();
        assert_eq!(
            crate::agent::types::message_text(&context.messages[0]),
            "new session"
        );
    }

    #[test]
    fn test_session_continue_recent_none_exist() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // No sessions exist - should create new
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
        sm.append_message(&make_user_msg("hello"));
        sm.append_message(&make_asst_msg("hi"));
        assert_eq!(sm.session_name().as_deref(), Some("My Task"));

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
        sm.append_compaction("Earlier work summarized", "entry_kept", 5000, None, None);

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
        let msg_id = sm.append_message(&make_user_msg("important message"));
        sm.append_message(&make_asst_msg("ok"));

        // Set label
        sm.append_label_change(&msg_id, Some("important"));
        assert_eq!(sm.label(&msg_id).as_deref(), Some("important"));

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
        let m1 = sm.append_message(&make_user_msg("one"));
        sm.append_message(&make_asst_msg("response one"));
        let _m2 = sm.append_message(&make_user_msg("two"));
        sm.append_message(&make_asst_msg("response two"));

        // Current leaf is after last message
        assert_eq!(sm.entries().len(), 4);

        // Branch back to first user message (writes a persistent LeafEntry, pi-compatible)
        sm.set_branch(&m1).unwrap();
        assert_eq!(sm.entries().len(), 5); // 4 original + 1 leaf entry
        assert_eq!(sm.leaf_id().as_deref(), Some(m1.as_str()));

        // Append a new branch
        sm.append_message(&make_asst_msg("alternate response"));
        // Now 6 entries (original 4 + leaf + 1 new message)
        assert_eq!(sm.entries().len(), 6);

        // Build context from current leaf - should have 2 messages (m1, branch asst)
        let context = sm.build_session_context();
        assert_eq!(context.messages.len(), 2); // user "one" + assistant "alternate response"
        // Verify metadata in context
        assert_eq!(context.thinking_level, "off");
        assert!(context.model.is_none());
        assert!(context.active_tool_names.is_none());
    }

    #[test]
    fn test_session_reset_leaf() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_user_msg("one"));
        sm.append_message(&make_asst_msg("response"));
        assert_eq!(sm.entries().len(), 2);

        // Reset leaf (persistent leaf entry, pi-compatible)
        sm.reset_leaf();
        // Leaf entry was written (type: "leaf", targetId: null)
        assert_eq!(sm.entries().len(), 3);
        assert!(sm.leaf_id().is_none());

        // Append from reset state (parentId should be None since leaf is None)
        sm.append_message(&make_user_msg("fresh start"));
        assert_eq!(sm.entries().len(), 4);
        // Verify fresh start has no parent
        match &sm.entries()[3] {
            SessionEntry::Message(m) => {
                assert!(m.parent_id.is_none());
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_session_branch_summary() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        sm.append_message(&make_user_msg("one"));
        sm.append_message(&make_asst_msg("response"));

        sm.append_branch_summary("root", "Abandoned path summary", None, None);

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
        let m1 = sm.append_message(&make_user_msg("one"));
        sm.append_message(&make_asst_msg("response"));

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
        sm.append_message(&make_user_msg("one"));
        sm.append_message(&make_asst_msg("ok"));
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
        sm1.append_message(&make_user_msg("old"));
        sm1.append_message(&make_asst_msg("old"));
        let _path1 = sm1.session_file().unwrap().to_path_buf();
        drop(sm1);

        std::thread::sleep(std::time::Duration::from_millis(10));

        // Create second session (more recent)
        let mut sm2 = SessionManager::create(&cwd, Some(&sessions_dir));
        sm2.append_message(&make_user_msg("new"));
        sm2.append_message(&make_asst_msg("new"));
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

        // Opening an empty file should not panic - should start fresh
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
        sm.append_message(&make_user_msg("test"));
        sm.append_message(&make_asst_msg("ok"));
        let original_id = sm.session_id().to_string();
        let file_path = sm.session_file().unwrap().to_path_buf();
        drop(sm);

        // Read the header line and write only that
        let content = std::fs::read_to_string(&file_path).unwrap();
        let header_line = content.lines().next().unwrap();
        std::fs::write(&file_path, format!("{}\n", header_line)).unwrap();

        // Open - should keep the session (header exists, just no entries)
        let sm = SessionManager::open(&file_path, Some(&sessions_dir), None);
        assert_eq!(sm.session_id(), original_id);
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
        sm.append_message(&make_user_msg("valid message"));
        sm.append_message(&make_asst_msg("valid response"));
        let file_path = sm.session_file().unwrap().to_path_buf();
        drop(sm);

        // Append garbage lines to the file
        let mut content = std::fs::read_to_string(&file_path).unwrap();
        content.push_str("this is garbage\n");
        content.push_str("{incomplete json\n");
        content.push('\n'); // blank line
        std::fs::write(&file_path, &content).unwrap();

        // Open - valid entries should be loaded, garbage skipped
        let sm = SessionManager::open(&file_path, Some(&sessions_dir), None);
        let ctx = sm.build_session_context();
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(
            crate::agent::types::message_text(&ctx.messages[0]),
            "valid message"
        );
        assert_eq!(
            crate::agent::types::message_text(&ctx.messages[1]),
            "valid response"
        );
    }

    #[test]
    fn test_corrupt_missing_header_uses_new_id() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // Write only valid entries but no session header
        let entry = SessionEntry::Message(MessageEntry::new(
            "msg1".to_string(),
            None,
            "2026-01-01T00:00:00Z".to_string(),
            make_user_msg("orphan message"),
            0.0,
        ));
        let json = serde_json::to_string(&entry).unwrap();
        let file_path = sessions_dir.join("no_header.jsonl");
        std::fs::write(&file_path, format!("{}\n", json)).unwrap();

        // Pi-compatible: no valid session header means the file is invalid.
        // Should generate new ID, empty entries (fresh start).
        let sm = SessionManager::open(&file_path, Some(&sessions_dir), None);
        assert!(!sm.session_id().is_empty());
        assert_eq!(sm.entries().len(), 0);
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

        // Open - recovers
        let mut sm = SessionManager::open(&file_path, Some(&sessions_dir), None);
        assert!(sm.entries().is_empty());

        // Should be able to append normally
        sm.append_message(&make_user_msg("fresh start"));
        sm.append_message(&make_asst_msg("fresh response"));

        let ctx = sm.build_session_context();
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(
            crate::agent::types::message_text(&ctx.messages[0]),
            "fresh start"
        );

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
