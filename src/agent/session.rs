//! Simplified session module — wraps `yoagent::Session` with cost tracking,
//! metadata, and file management. No traits, no type-of-entry enum, no lazy write.
//!
//! File format:
//!   Line 1: metadata JSON (id, cwd, createdAt, name, parentSession)
//!   Lines 2+: yoagent JSONL entries (one per line, append-friendly)
//!
//! Costs are stored as `AgentMessage::Extension` entries (kind = `session/cost`)
//! inside the JSONL stream, not in the header metadata.
//!
//! Metadata entries (model changes, compaction, etc.) are stored as
//! `AgentMessage::Extension` entries with well-known `kind` values.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use yoagent::Session as YoagentSession;
use yoagent::types::{AgentMessage, Content, ExtensionMessage, Message, StopReason};

// ── Extension kinds for metadata entries ──────────────────────────

pub const KIND_MODEL_CHANGE: &str = "session/model_change";
pub const KIND_THINKING_LEVEL_CHANGE: &str = "session/thinking_level_change";
pub const KIND_ACTIVE_TOOLS_CHANGE: &str = "session/active_tools_change";
pub const KIND_COMPACTION: &str = "session/compaction";
pub const KIND_BRANCH_SUMMARY: &str = "session/branch_summary";
pub const KIND_LABEL: &str = "session/label";
const KIND_CUSTOM_MESSAGE: &str = "session/custom_message";

/// Extension kind for storing per-message cost (pre-computed at creation time).
pub const KIND_SESSION_COST: &str = "session/cost";

// ── MessageCost ───────────────────────────────────────────────────

/// Cost of a single message (pre-computed at creation time).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct MessageCost {
    pub input: f64,
    pub output: f64,
    #[serde(rename = "cacheRead")]
    pub cache_read: f64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: f64,
    pub total: f64,
}

impl MessageCost {
    pub const ZERO: Self = Self {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
        total: 0.0,
    };

    pub fn new(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Self {
        let total = input + output + cache_read + cache_write;
        Self {
            input,
            output,
            cache_read,
            cache_write,
            total,
        }
    }
}

// ── Session metadata (serialised as file line 1) ─────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionMeta {
    id: String,
    cwd: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(rename = "parentSession", skip_serializing_if = "Option::is_none")]
    parent_session: Option<String>,
}

// ── Session ───────────────────────────────────────────────────────

/// High-level session: wraps `yoagent::Session`, adds cost tracking and metadata.
pub struct Session {
    /// The underlying yoagent session tree.
    inner: YoagentSession,
    /// Session-level metadata.
    meta: SessionMeta,
    /// File path on disk, if persisted.
    file_path: Option<PathBuf>,
}

impl Session {
    fn from_parts(inner: YoagentSession, meta: SessionMeta, file_path: Option<PathBuf>) -> Self {
        Self {
            inner,
            meta,
            file_path,
        }
    }

    fn append_ext(&mut self, kind: &str, data: serde_json::Value) -> String {
        self.inner
            .append(AgentMessage::Extension(ExtensionMessage::new(kind, data)))
    }

    // ── Constructors ─────────────────────────────────────────

    /// Create a new session (in-memory). Use `flush()` to persist.
    pub fn new(cwd: &Path) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();
        Self {
            inner: YoagentSession::new(),
            meta: SessionMeta {
                id,
                cwd: cwd.to_string_lossy().to_string(),
                created_at,
                name: None,
                parent_session: None,
            },
            file_path: None,
        }
    }

    /// Create a new session (in-memory). The session is persisted on first mutation.
    pub fn create(cwd: &Path, _session_dir: &Path) -> std::io::Result<Self> {
        Ok(Self::new(cwd))
    }

    /// Open an existing session file. Falls back to a new session on error.
    pub fn open(path: &Path, cwd_override: Option<&Path>) -> Self {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                let cwd = cwd_override.unwrap_or(Path::new("/"));
                return Self::new(cwd);
            }
        };

        let (meta, jsonl) = match content.split_once('\n') {
            Some((first, rest)) => {
                let meta: SessionMeta = match serde_json::from_str(first.trim()) {
                    Ok(m) => m,
                    Err(_) => {
                        let cwd = cwd_override.unwrap_or(Path::new("/"));
                        return Self::new(cwd);
                    }
                };
                (meta, rest)
            }
            None => {
                let cwd = cwd_override.unwrap_or(Path::new("/"));
                return Self::new(cwd);
            }
        };

        let inner = YoagentSession::from_jsonl(jsonl).unwrap_or_default();

        Self {
            inner,
            meta,
            file_path: Some(path.to_path_buf()),
        }
    }

    /// In-memory session (alias for `new`).
    pub fn in_memory(cwd: &Path) -> Self {
        Self::new(cwd)
    }

    /// Continue the most recent session in `session_dir`, or create new.
    pub fn continue_recent(cwd: &Path, session_dir: &Path) -> std::io::Result<Self> {
        let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        if let Ok(entries) = fs::read_dir(session_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().is_some_and(|e| e == "jsonl")
                    && let Ok(meta) = p.metadata()
                    && let Ok(mtime) = meta.modified()
                {
                    files.push((p, mtime));
                }
            }
        }

        files.sort_by_key(|b| std::cmp::Reverse(b.1));
        match files.into_iter().next() {
            Some((path, _)) => Ok(Self::open(&path, Some(cwd))),
            None => Self::create(cwd, session_dir),
        }
    }

    /// Fork from an existing session file into a new one.
    pub fn fork_from(
        source: &Path,
        target_cwd: &Path,
        session_dir: &Path,
    ) -> std::io::Result<Self> {
        let source_content = fs::read_to_string(source)?;
        let jsonl = source_content
            .split_once('\n')
            .map(|(_, rest)| rest)
            .unwrap_or(&source_content);

        let inner = YoagentSession::from_jsonl(jsonl)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

        let id = uuid::Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();
        let cwd = target_cwd.to_string_lossy().to_string();
        let meta = SessionMeta {
            id,
            cwd,
            created_at,
            name: None,
            parent_session: Some(source.to_string_lossy().to_string()),
        };

        let mut s = Self::from_parts(inner, meta, None);
        s.flush(Some(session_dir))?;
        Ok(s)
    }

    // ── Persistence ──────────────────────────────────────────

    /// Write the session file. Creates the directory if needed.
    pub fn flush(&mut self, session_dir: Option<&Path>) -> std::io::Result<()> {
        let dir = match session_dir {
            Some(d) => d.to_path_buf(),
            None => match self.file_path {
                Some(ref p) => p.parent().unwrap().to_path_buf(),
                None => return Ok(()),
            },
        };
        let file_ts = self.meta.created_at.replace([':', '.'], "-");
        let file_name = format!("{}_{}.jsonl", file_ts, self.meta.id);
        let file_path = dir.join(&file_name);

        let meta_json = serde_json::to_string(&self.meta).map_err(std::io::Error::other)?;
        let entries_json = self.inner.to_jsonl();

        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&file_path, format!("{}\n{}\n", meta_json, entries_json))?;

        self.file_path = Some(file_path);
        Ok(())
    }

    /// Ensure the session file has been flushed to disk.
    /// Creates the file on first call, updates it on subsequent calls.
    pub fn ensure_flushed(&mut self, session_dir: Option<&Path>) {
        let _ = self.flush(session_dir);
    }

    /// Whether this session has been persisted to disk.
    pub fn is_persisted(&self) -> bool {
        self.file_path.is_some()
    }

    // ── Message appending ────────────────────────────────────

    /// Append a conversation message. Returns the entry id.
    pub fn append_message(&mut self, msg: AgentMessage) -> String {
        self.inner.append(msg)
    }

    /// Append a message with a pre-computed cost.
    /// The cost is stored as an extension entry in the JSONL stream.
    pub fn append_message_with_cost(&mut self, msg: AgentMessage, cost: MessageCost) -> String {
        let id = self.inner.append(msg);
        self.inner
            .append(AgentMessage::Extension(ExtensionMessage::new(
                KIND_SESSION_COST,
                serde_json::json!({
                    "targetId": id,
                    "input": cost.input,
                    "output": cost.output,
                    "cacheRead": cost.cache_read,
                    "cacheWrite": cost.cache_write,
                    "total": cost.total,
                }),
            )));
        id
    }

    /// Get the cost for an entry, if any. Scans cost extension entries in
    /// reverse to find the most recent one targeting the given entry id.
    pub fn entry_cost(&self, id: &str) -> Option<MessageCost> {
        self.inner.entries().iter().rev().find_map(|e| {
            if let AgentMessage::Extension(ext) = &e.message {
                if ext.kind == KIND_SESSION_COST && ext.data["targetId"].as_str() == Some(id) {
                    Some(MessageCost {
                        input: ext.data["input"].as_f64().unwrap_or(0.0),
                        output: ext.data["output"].as_f64().unwrap_or(0.0),
                        cache_read: ext.data["cacheRead"].as_f64().unwrap_or(0.0),
                        cache_write: ext.data["cacheWrite"].as_f64().unwrap_or(0.0),
                        total: ext.data["total"].as_f64().unwrap_or(0.0),
                    })
                } else {
                    None
                }
            } else {
                None
            }
        })
    }

    // ── Metadata entries ─────────────────────────────────────

    pub fn append_model_change(&mut self, provider: &str, model_id: &str) -> String {
        self.append_ext(
            KIND_MODEL_CHANGE,
            serde_json::json!({"provider": provider, "modelId": model_id}),
        )
    }

    pub fn append_thinking_level_change(&mut self, level: &str) -> String {
        self.append_ext(
            KIND_THINKING_LEVEL_CHANGE,
            serde_json::json!({"level": level}),
        )
    }

    pub fn append_active_tools_change(&mut self, tools: &[String]) -> String {
        self.append_ext(
            KIND_ACTIVE_TOOLS_CHANGE,
            serde_json::json!({"tools": tools}),
        )
    }

    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        tokens_before: u64,
        details: Option<serde_json::Value>,
    ) -> String {
        let mut data = serde_json::json!({
            "summary": summary,
            "firstKeptEntryId": first_kept_entry_id,
            "tokensBefore": tokens_before,
        });
        if let Some(d) = details {
            data["details"] = d;
        }
        self.append_ext(KIND_COMPACTION, data)
    }

    pub fn append_branch_summary(
        &mut self,
        from_id: &str,
        summary: &str,
        details: Option<serde_json::Value>,
    ) -> String {
        let mut data = serde_json::json!({
            "fromId": from_id,
            "summary": summary,
        });
        if let Some(d) = details {
            data["details"] = d;
        }
        self.append_ext(KIND_BRANCH_SUMMARY, data)
    }

    pub fn append_session_info(&mut self, name: &str) -> String {
        let sanitized = name.replace(['\r', '\n'], " ").trim().to_string();
        self.meta.name = Some(sanitized);
        self.append_ext(
            KIND_CUSTOM_MESSAGE,
            serde_json::json!({"text": name, "display": true}),
        )
    }

    pub fn append_label_change(
        &mut self,
        target_id: &str,
        label: Option<&str>,
    ) -> Result<String, String> {
        if self.inner.entry(target_id).is_none() {
            return Err(format!("Entry {} not found", target_id));
        }
        Ok(self.append_ext(
            KIND_LABEL,
            serde_json::json!({
                "targetId": target_id,
                "label": label,
            }),
        ))
    }

    pub fn append_custom_message_entry(
        &mut self,
        custom_type: &str,
        content: serde_json::Value,
    ) -> String {
        self.append_ext(custom_type, content)
    }

    // ── Navigation ───────────────────────────────────────────

    /// Current leaf (head) entry id.
    pub fn get_leaf_id(&self) -> Option<String> {
        self.inner.head().map(|s| s.to_string())
    }

    /// Move the leaf to an existing entry (fork point).
    pub fn set_leaf_id(&mut self, id: &str) -> Result<(), String> {
        self.inner.seek(id).map_err(|e| e.to_string())
    }

    /// If the current branch ends with a user message that has no
    /// corresponding assistant response (an orphan from a cancelled turn),
    /// remove it by rebuilding the session tree without it.
    /// Returns `true` if a message was pruned.
    pub fn prune_orphan_user_message(&mut self) -> bool {
        let head_id = match self.inner.head() {
            Some(id) => id.to_string(),
            None => return false,
        };
        let is_user = self.inner.entry(&head_id).is_some_and(|e| {
            matches!(
                &e.message,
                yoagent::types::AgentMessage::Llm(yoagent::types::Message::User { .. })
            )
        });
        if !is_user {
            return false;
        }
        // Rebuild the session from JSONL, dropping the last line.
        let jsonl = self.inner.to_jsonl();
        let lines: Vec<&str> = jsonl.lines().collect();
        if lines.is_empty() {
            return false;
        }
        let new_jsonl = lines[..lines.len() - 1].join("\n");
        match yoagent::Session::from_jsonl(&new_jsonl) {
            Ok(new_inner) => {
                self.inner = new_inner;
                true
            }
            Err(_) => false,
        }
    }

    // ── Entry access ─────────────────────────────────────────

    /// All entries (all branches) in insertion order.
    pub fn get_entries(&self) -> &[yoagent::session::SessionEntry] {
        self.inner.entries()
    }

    /// Look up a single entry by id.
    pub fn get_entry(&self, id: &str) -> Option<&yoagent::session::SessionEntry> {
        self.inner.entry(id)
    }

    /// Entries on the root→head path.
    pub fn get_branch(&self, from_id: Option<&str>) -> Vec<&yoagent::session::SessionEntry> {
        let ids = match from_id {
            Some(target) => {
                let mut ids = vec![target.to_string()];
                let mut cursor = self.inner.entry(target).and_then(|e| e.parent_id.clone());
                while let Some(pid) = cursor {
                    ids.push(pid.clone());
                    cursor = self.inner.entry(&pid).and_then(|e| e.parent_id.clone());
                }
                ids.reverse();
                ids
            }
            None => self.inner.path_ids(),
        };
        ids.iter().filter_map(|id| self.inner.entry(id)).collect()
    }

    /// Direct children of an entry.
    pub fn get_children(&self, parent_id: &str) -> Vec<&yoagent::session::SessionEntry> {
        self.inner.children(parent_id)
    }

    /// Find entries matching a type by checking Extension kind.
    /// Returns entries whose `message` is an Extension with matching `kind`,
    /// or (if `type_name` is "message") entries whose `message` is an LLM message.
    pub fn find_entries(&self, type_name: &str) -> Vec<&yoagent::session::SessionEntry> {
        if type_name == "message" {
            return self
                .inner
                .entries()
                .iter()
                .filter(|e| e.message.as_llm().is_some())
                .collect();
        }
        self.inner
            .entries()
            .iter()
            .filter(|e| {
                if let AgentMessage::Extension(ext) = &e.message {
                    ext.kind == type_name || ext.kind.strip_prefix("session/") == Some(type_name)
                } else {
                    false
                }
            })
            .collect()
    }

    // ── Label support ────────────────────────────────────────

    /// Get the human-readable label for an entry, if any.
    pub fn get_label(&self, id: &str) -> Option<String> {
        // Labels are stored as Extension entries with KIND_LABEL.
        // Find the most recent one targeting this id.
        self.inner
            .entries()
            .iter()
            .rev()
            .find(|e| {
                if let AgentMessage::Extension(ext) = &e.message {
                    ext.kind == KIND_LABEL && ext.data["targetId"].as_str() == Some(id)
                } else {
                    false
                }
            })
            .and_then(|e| {
                if let AgentMessage::Extension(ext) = &e.message {
                    ext.data["label"].as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
    }

    /// Get the timestamp of the latest label change for an entry.
    pub fn get_label_timestamp(&self, id: &str) -> Option<String> {
        self.inner
            .entries()
            .iter()
            .rev()
            .find(|e| {
                if let AgentMessage::Extension(ext) = &e.message {
                    ext.kind == KIND_LABEL && ext.data["targetId"].as_str() == Some(id)
                } else {
                    false
                }
            })
            .map(|e| {
                // Convert u64 timestamp to RFC3339
                let secs = e.timestamp / 1000;
                let nsecs = (e.timestamp % 1000) * 1_000_000;
                let dt = DateTime::from_timestamp(secs as i64, nsecs as u32).unwrap_or_default();
                dt.to_rfc3339()
            })
    }

    // ── Build context for LLM ─────────────────────────────────

    /// Build the session context (messages + resolved metadata) for the LLM.
    /// Walks the root→head path to find latest thinking level, model, tools,
    /// and handles compaction by replacing summarised prefix with a summary message.
    pub fn build_context(&self) -> SessionContext {
        let path = self.get_branch(None);
        let mut thinking_level = "off".to_string();
        let mut model: Option<(String, String)> = None;
        let mut active_tool_names: Option<Vec<String>> = None;
        let mut compaction_summary: Option<String> = None;
        let mut first_kept_id: Option<String> = None;

        for entry in &path {
            if let AgentMessage::Extension(ext) = &entry.message {
                match ext.kind.as_str() {
                    KIND_THINKING_LEVEL_CHANGE => {
                        if let Some(level) = ext.data["level"].as_str() {
                            thinking_level = level.to_string();
                        }
                    }
                    KIND_MODEL_CHANGE => {
                        let provider = ext.data["provider"].as_str();
                        let model_id = ext.data["modelId"].as_str();
                        if let (Some(p), Some(m)) = (provider, model_id) {
                            model = Some((p.to_string(), m.to_string()));
                        }
                    }
                    KIND_ACTIVE_TOOLS_CHANGE => {
                        if let Some(tools) = ext.data["tools"].as_array() {
                            active_tool_names = Some(
                                tools
                                    .iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect(),
                            );
                        }
                    }
                    KIND_COMPACTION => {
                        compaction_summary = ext.data["summary"].as_str().map(|s| s.to_string());
                        first_kept_id =
                            ext.data["firstKeptEntryId"].as_str().map(|s| s.to_string());
                    }
                    _ => {}
                }
            }
        }

        // Fallback: extract model from assistant messages.
        if model.is_none() {
            for entry in &path {
                if let Some(yoagent::types::Message::Assistant {
                    model: m,
                    provider: p,
                    ..
                }) = entry.message.as_llm()
                    && !m.is_empty()
                    && !p.is_empty()
                {
                    model = Some((p.clone(), m.clone()));
                    break;
                }
            }
        }

        // Build messages list, handling compaction.
        let mut messages = if let (Some(summary), Some(first_kept)) =
            (&compaction_summary, &first_kept_id)
        {
            let mut msgs = Vec::new();

            // 1. Compaction summary as a user message.
            let comp_text = format!(
                "The conversation history before this point was compacted into the following summary:\n\n<summary>\n{}\n</summary>",
                summary
            );
            msgs.push(AgentMessage::Llm(yoagent::types::Message::User {
                content: vec![yoagent::types::Content::Text { text: comp_text }],
                timestamp: Utc::now().timestamp_millis() as u64,
            }));

            // 2. Entries at/after firstKeptEntryId, then after compaction entry.
            //    (path is root→head)
            let mut found_first_kept = false;
            let mut past_compaction = false;

            for entry in &path {
                let is_compaction = matches!(&entry.message, AgentMessage::Extension(ext) if ext.kind == KIND_COMPACTION);

                if is_compaction {
                    past_compaction = true;
                    continue;
                }

                if !past_compaction {
                    if entry.id == *first_kept {
                        found_first_kept = true;
                    }
                    if found_first_kept {
                        append_to_messages(entry, &mut msgs);
                    }
                } else {
                    append_to_messages(entry, &mut msgs);
                }
            }

            msgs
        } else {
            let mut msgs = Vec::new();
            for entry in &path {
                append_to_messages(entry, &mut msgs);
            }
            msgs
        };

        cleanup_dangling_tool_calls(&mut messages);

        SessionContext {
            messages,
            thinking_level,
            model,
            active_tool_names,
        }
    }

    // ── Metadata accessors ───────────────────────────────────

    pub fn session_id(&self) -> &str {
        &self.meta.id
    }

    pub fn session_file(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    pub fn session_name(&self) -> Option<&str> {
        self.meta.name.as_deref()
    }

    pub fn cwd(&self) -> &str {
        &self.meta.cwd
    }

    pub fn created_at(&self) -> &str {
        &self.meta.created_at
    }

    pub fn parent_session_path(&self) -> Option<&str> {
        self.meta.parent_session.as_deref()
    }

    /// Directory suitable for storing this session's file.
    pub fn default_session_dir(&self, base_dir: &Path) -> PathBuf {
        base_dir
            .join("sessions")
            .join(encode_cwd_for_dir(Path::new(&self.meta.cwd)))
    }
}

// ── Context for LLM ───────────────────────────────────────────────

/// Resolved conversation context sent to the LLM.
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub messages: Vec<AgentMessage>,
    pub thinking_level: String,
    pub model: Option<(String, String)>,
    pub active_tool_names: Option<Vec<String>>,
}

/// Append a session entry's message to the messages list.
fn append_to_messages(entry: &yoagent::session::SessionEntry, msgs: &mut Vec<AgentMessage>) {
    if let Some(llm_msg) = entry.message.as_llm() {
        // Skip provider/diagnostic error messages.
        if let yoagent::types::Message::Assistant {
            error_message: Some(_),
            ..
        } = llm_msg
        {
            return;
        }
        msgs.push(AgentMessage::Llm(llm_msg.clone()));
    } else if let AgentMessage::Extension(ext) = &entry.message {
        if ext.kind == KIND_BRANCH_SUMMARY {
            if let Some(summary) = ext.data["summary"].as_str()
                && !summary.is_empty()
            {
                let bs_text = format!(
                    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n{}\n</summary>",
                    summary
                );
                msgs.push(AgentMessage::Llm(yoagent::types::Message::User {
                    content: vec![yoagent::types::Content::Text { text: bs_text }],
                    timestamp: Utc::now().timestamp_millis() as u64,
                }));
            }
        } else if ext.kind.starts_with("session/") {
            // Extension messages (metadata, etc.) are not sent to the LLM.
        } else {
            // Unknown extension kinds: include as extension messages.
            msgs.push(AgentMessage::Extension(ext.clone()));
        }
    }
}

// ── SessionInfo (for listing) ──────────────────────────────────────

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
    pub all_messages_text: String,
}

use crate::agent::types::{
    message_is_assistant, message_is_tool_result, message_is_user, message_tool_call_count,
    message_usage,
};

/// Format a detailed session info string for /session display.
pub fn format_session_info(session: &Session) -> String {
    let entries = session.get_entries();
    let mut message_count: usize = 0;
    let mut user_messages: usize = 0;
    let mut assistant_messages: usize = 0;
    let mut tool_calls: usize = 0;
    let mut tool_results: usize = 0;
    let mut total_tokens: u64 = 0;
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_read_tokens: u64 = 0;
    let mut cache_write_tokens: u64 = 0;
    let mut cost: f64 = 0.0;

    for entry in entries {
        if let Some(llm_msg) = entry.message.as_llm() {
            message_count += 1;
            if message_is_user(&yoagent::types::AgentMessage::Llm(llm_msg.clone())) {
                user_messages += 1;
            } else if message_is_assistant(&yoagent::types::AgentMessage::Llm(llm_msg.clone())) {
                assistant_messages += 1;
                let tc_count =
                    message_tool_call_count(&yoagent::types::AgentMessage::Llm(llm_msg.clone()));
                tool_calls += tc_count;
            } else if message_is_tool_result(&yoagent::types::AgentMessage::Llm(llm_msg.clone())) {
                tool_results += 1;
            }
            if let Some(usage) = message_usage(&yoagent::types::AgentMessage::Llm(llm_msg.clone()))
            {
                input_tokens += usage.input;
                output_tokens += usage.output;
                cache_read_tokens += usage.cache_read;
                cache_write_tokens += usage.cache_write;
                total_tokens += usage.input + usage.output + usage.cache_read + usage.cache_write;
            }
            cost += session.entry_cost(&entry.id).map_or(0.0, |c| c.total);
        }
    }

    let session_id = session.session_id();
    let file_path = session.session_file();
    let name = session.session_name();

    let name_display = name
        .map(|n| n.to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    let file_display = file_path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "in-memory".to_string());

    let mut info = format!(
        "Session Info\n\n\
         Name: {name_display}\n\
         File: {file_display}\n\
         ID: {session_id}\n\
         \n\
         Messages\n\
         User: {user_messages}\n\
         Assistant: {assistant_messages}\n\
         Tool Calls: {tool_calls}\n\
         Tool Results: {tool_results}\n\
         Total: {message_count}\n\
         \n\
         Tokens\n\
         Input: {}\n\
         Output: {}",
        format_number(input_tokens),
        format_number(output_tokens),
    );
    if cache_read_tokens > 0 {
        info += &format!("\nCache Read: {}", format_number(cache_read_tokens));
    }
    if cache_write_tokens > 0 {
        info += &format!("\nCache Write: {}", format_number(cache_write_tokens));
    }
    info += &format!("\nTotal: {}", format_number(total_tokens));

    if cost > 0.0 {
        info += &format!("\n\nCost\nTotal: {:.4}", cost);
    }

    info
}

/// Format a number in human-readable form (e.g. 1500 -> "1.5K", 1200000 -> "1.2M").
fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ── Free functions ─────────────────────────────────────────────────

/// Encode a working directory path into a safe directory name.
pub fn encode_cwd_for_dir(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let cleaned = s
        .trim_start_matches('/')
        .trim_start_matches('\\')
        .replace(['/', '\\', ':'], "-");
    format!("--{}--", cleaned)
}

/// List all sessions in a directory, newest first.
pub fn list_sessions(session_dir: &Path) -> Vec<SessionInfo> {
    let mut sessions: Vec<SessionInfo> = Vec::new();
    let dir = match fs::read_dir(session_dir) {
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

/// List sessions across all project directories under a base directory.
pub fn list_all_sessions(
    base_dir: &Path,
    progress: Option<&dyn Fn(usize, usize)>,
) -> Vec<SessionInfo> {
    let dir = base_dir.to_path_buf();
    let mut all_sessions: Vec<SessionInfo> = Vec::new();

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
        let sessions = list_sessions(session_dir);
        loaded += 1;
        if let Some(ref cb) = progress {
            cb(loaded, total_dirs);
        }
        all_sessions.extend(sessions);
    }

    all_sessions.sort_by_key(|b| std::cmp::Reverse(b.created));
    all_sessions
}

/// Load metadata for a single session file.
pub fn load_session_info(path: &Path) -> Option<SessionInfo> {
    let content = fs::read_to_string(path).ok()?;
    let first_line = content.lines().next()?;
    let meta: SessionMeta = serde_json::from_str(first_line.trim()).ok()?;
    let created = DateTime::parse_from_rfc3339(&meta.created_at)
        .ok()?
        .with_timezone(&Utc);
    let modified = path.metadata().ok()?.modified().ok()?;
    let modified_dt: DateTime<Utc> = modified.into();

    // Parse entries for message count / text.
    let jsonl = content.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
    let session = YoagentSession::from_jsonl(jsonl).ok()?;
    let all_entries = session.entries();

    let message_count = all_entries
        .iter()
        .filter(|e| e.message.as_llm().is_some())
        .count();

    let first_message = all_entries
        .iter()
        .find_map(|e| message_text(&e.message))
        .unwrap_or_default();

    let all_messages_text = all_entries
        .iter()
        .filter_map(|e| message_text(&e.message))
        .collect::<Vec<_>>()
        .join("\n");

    Some(SessionInfo {
        path: path.to_path_buf(),
        id: meta.id,
        cwd: meta.cwd,
        name: meta.name,
        parent_session_path: meta.parent_session,
        created,
        modified: modified_dt,
        message_count,
        first_message,
        all_messages_text,
    })
}

fn message_text(msg: &AgentMessage) -> Option<String> {
    match msg {
        AgentMessage::Llm(m) => match m {
            yoagent::types::Message::User { content, .. }
            | yoagent::types::Message::Assistant { content, .. }
            | yoagent::types::Message::ToolResult { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| {
                        if let yoagent::types::Content::Text { text } = c {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            ),
        },
        AgentMessage::Extension(ext) => ext
            .data
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    }
}

/// Delete a session file.
pub fn delete_session(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Fork a session: create a new session file from an existing one.
pub fn fork_session(
    source_path: &Path,
    target_dir: &Path,
    entry_id: Option<&str>,
    position: Option<&str>,
) -> std::io::Result<String> {
    let source_content = fs::read_to_string(source_path)?;
    let (_, jsonl) = source_content
        .split_once('\n')
        .unwrap_or(("", &source_content));

    let source_session = YoagentSession::from_jsonl(jsonl)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    // Get entries for the fork.
    let entries = if let Some(target_id) = entry_id {
        let effective_leaf = match position.unwrap_or("before") {
            "at" => Some(target_id.to_string()),
            _ => {
                // "before" position: use target's parent as the leaf.
                source_session
                    .entry(target_id)
                    .and_then(|e| e.parent_id.clone())
            }
        };

        let Some(ref leaf) = effective_leaf else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Could not determine fork point",
            ));
        };

        // Get path from leaf to root.
        get_path_to_root(&source_session, leaf)
    } else {
        source_session.entries().to_vec()
    };

    // Create new session.
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let meta = SessionMeta {
        id: id.clone(),
        cwd: ".".to_string(),
        created_at,
        name: None,
        parent_session: Some(source_path.to_string_lossy().to_string()),
    };

    let mut inner = YoagentSession::new();
    for entry in &entries {
        inner.append(entry.message.clone());
    }

    let meta_json = serde_json::to_string(&meta).map_err(std::io::Error::other)?;
    let entries_json = inner.to_jsonl();

    fs::create_dir_all(target_dir)?;
    let file_ts = meta.created_at.replace([':', '.'], "-");
    let file_name = format!("{}_{}.jsonl", file_ts, meta.id);
    let target_path = target_dir.join(&file_name);
    fs::write(&target_path, format!("{}\n{}\n", meta_json, entries_json))?;

    Ok(id)
}

fn get_path_to_root(
    session: &YoagentSession,
    leaf_id: &str,
) -> Vec<yoagent::session::SessionEntry> {
    let mut path = Vec::new();
    let mut cursor = Some(leaf_id.to_string());
    while let Some(id) = cursor {
        if let Some(entry) = session.entry(&id) {
            cursor = entry.parent_id.clone();
            path.push(entry.clone());
        } else {
            break;
        }
    }
    path.reverse();
    path
}

// Re-export yoagent's SessionEntry for convenience.
pub use yoagent::session::SessionEntry;

// ── Helpers ──────────────────────────────────────────────────────────

/// Insert synthetic `ToolResult` messages for orphaned tool calls.
///
/// When the user interrupts the agent mid-tool-execution (e.g. pressing
/// Escape) the session may have `Assistant` messages with `ToolCall`
/// content blocks that never received a matching `ToolResult`. Sending
/// such messages to an LLM API would produce an invalid conversation
/// history and cause an "Upstream request failed" error.
///
/// Pi-compatible: mirrors pi's `transform-messages.ts` second pass.
/// Also skips errored/aborted assistant messages (partial/incomplete).
fn cleanup_dangling_tool_calls(messages: &mut Vec<AgentMessage>) {
    use std::collections::HashSet;

    let mut result: Vec<AgentMessage> = Vec::with_capacity(messages.len());
    let mut pending_tool_calls: Vec<(String, String)> = Vec::new();
    let mut existing_result_ids: HashSet<String> = HashSet::new();

    for msg in messages.drain(..) {
        match &msg {
            AgentMessage::Llm(Message::Assistant {
                content,
                stop_reason,
                ..
            }) => {
                // Flush pending orphaned tool calls before next assistant.
                insert_synthetic_results(
                    &mut result,
                    &mut pending_tool_calls,
                    &mut existing_result_ids,
                );

                // Skip errored/aborted assistant messages — partial/incomplete.
                if matches!(stop_reason, StopReason::Error | StopReason::Aborted) {
                    continue;
                }

                // Track tool calls from this assistant message.
                let tool_calls: Vec<(String, String)> = content
                    .iter()
                    .filter_map(|c| {
                        if let Content::ToolCall { id, name, .. } = c {
                            Some((id.clone(), name.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();

                if !tool_calls.is_empty() {
                    pending_tool_calls = tool_calls;
                    existing_result_ids = HashSet::new();
                }

                result.push(msg);
            }
            AgentMessage::Llm(Message::ToolResult { tool_call_id, .. }) => {
                existing_result_ids.insert(tool_call_id.clone());
                result.push(msg);
            }
            AgentMessage::Llm(Message::User { .. }) => {
                // User message interrupts tool flow — insert synthetic results.
                insert_synthetic_results(
                    &mut result,
                    &mut pending_tool_calls,
                    &mut existing_result_ids,
                );
                result.push(msg);
            }
            _ => {
                result.push(msg);
            }
        }
    }

    // Handle any remaining unresolved tool calls at the end of conversation.
    insert_synthetic_results(
        &mut result,
        &mut pending_tool_calls,
        &mut existing_result_ids,
    );

    *messages = result;
}

/// Insert synthetic error tool results for all pending orphaned tool calls.
fn insert_synthetic_results(
    result: &mut Vec<AgentMessage>,
    pending: &mut Vec<(String, String)>,
    existing: &mut HashSet<String>,
) {
    if pending.is_empty() {
        return;
    }
    for (id, name) in pending.drain(..) {
        if !existing.contains(&id) {
            result.push(AgentMessage::Llm(Message::ToolResult {
                tool_call_id: id,
                tool_name: name,
                content: vec![Content::Text {
                    text: "No result provided".into(),
                }],
                is_error: true,
                timestamp: yoagent::types::now_ms(),
            }));
        }
    }
    existing.clear();
}

// ── get_default_session_dir ─────────────────────────────────────

/// Get the default session directory for a cwd.
pub fn get_default_session_dir(cwd: &Path) -> PathBuf {
    let rab_dir = directories::BaseDirs::new()
        .expect("Could not determine home directory")
        .home_dir()
        .join(".rab");
    rab_dir.join("sessions").join(encode_cwd_for_dir(cwd))
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use yoagent::types::Message;

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(Message::User {
            content: vec![yoagent::types::Content::Text {
                text: text.to_string(),
            }],
            timestamp: yoagent::types::now_ms(),
        })
    }

    fn asst_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(
            Message::assistant(
                vec![yoagent::types::Content::Text {
                    text: text.to_string(),
                }],
                yoagent::types::StopReason::Stop,
                "test-model",
                "test-provider",
                yoagent::types::Usage::default(),
            )
            .with_timestamp(yoagent::types::now_ms()),
        )
    }

    #[test]
    fn test_create_does_not_flush() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut s = Session::create(&cwd, &sessions_dir).unwrap();
        // create no longer flushes — session is persisted on first mutation
        assert!(!s.is_persisted());

        // First mutation triggers flush
        s.append_message(user_msg("hello"));
        // append_message doesn't auto-flush at Session level either —
        // ensure_flushed is called from AgentSession level.
        // Verify flush works when called explicitly.
        s.flush(Some(&sessions_dir)).unwrap();
        assert!(s.is_persisted());
        assert!(s.session_file().unwrap().exists());
    }

    #[test]
    fn test_append_and_context() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut s = Session::create(&cwd, &sessions_dir).unwrap();
        s.append_message(user_msg("hello"));
        s.append_message(asst_msg("hi there"));

        let ctx = s.build_context();
        assert_eq!(ctx.messages.len(), 2);
    }

    #[test]
    fn test_metadata_tracking() {
        let mut s = Session::in_memory(Path::new("/tmp/test"));
        s.append_message(user_msg("hello"));
        s.append_thinking_level_change("high");
        s.append_model_change("opencode_go", "deepseek-v4-pro");
        s.append_active_tools_change(&["read".to_string(), "write".to_string()]);
        s.append_message(asst_msg("response"));

        let ctx = s.build_context();
        assert_eq!(ctx.thinking_level, "high");
        assert_eq!(
            ctx.model,
            Some(("opencode_go".to_string(), "deepseek-v4-pro".to_string()))
        );
        assert_eq!(
            ctx.active_tool_names,
            Some(vec!["read".to_string(), "write".to_string()])
        );
        assert_eq!(ctx.messages.len(), 2); // only conversation messages
    }

    #[test]
    fn test_branch_navigation() {
        let mut s = Session::in_memory(Path::new("/tmp/test"));
        let m1 = s.append_message(user_msg("one"));
        s.append_message(asst_msg("response one"));
        let _m2 = s.append_message(user_msg("two"));
        s.append_message(asst_msg("response two"));

        assert_eq!(s.get_entries().len(), 4);
        assert_eq!(
            s.get_leaf_id().as_deref(),
            Some(s.get_entries().last().unwrap().id.as_str())
        );

        // Branch back to first user message.
        s.set_leaf_id(&m1).unwrap();
        assert_eq!(s.get_leaf_id().as_deref(), Some(m1.as_str()));

        // Append a new branch.
        s.append_message(asst_msg("alternate response"));
        assert_eq!(s.get_entries().len(), 5);

        let ctx = s.build_context();
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.thinking_level, "off");
    }

    #[test]
    fn test_label_support() {
        let mut s = Session::in_memory(Path::new("/tmp/test"));
        let msg_id = s.append_message(user_msg("important message"));
        s.append_message(asst_msg("ok"));

        // Set label.
        s.append_label_change(&msg_id, Some("important")).unwrap();
        assert_eq!(s.get_label(&msg_id).as_deref(), Some("important"));

        // Clear label.
        s.append_label_change(&msg_id, None).unwrap();
        assert_eq!(s.get_label(&msg_id), None);
    }

    #[test]
    fn test_list_sessions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut s = Session::create(&cwd, &sessions_dir).unwrap();
        s.append_message(user_msg("first"));
        s.append_message(asst_msg("response"));
        s.flush(Some(&sessions_dir)).unwrap();
        drop(s);

        let sessions = list_sessions(&sessions_dir);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].message_count, 2);
    }

    #[test]
    fn test_delete_session() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test.jsonl");
        // Write a minimal session file
        let meta = SessionMeta {
            id: "test".to_string(),
            cwd: "/".to_string(),
            created_at: Utc::now().to_rfc3339(),
            name: None,
            parent_session: None,
        };
        let meta_json = serde_json::to_string(&meta).unwrap();
        std::fs::write(&path, format!("{}\n", meta_json)).unwrap();
        assert!(path.exists());
        delete_session(&path).unwrap();
        assert!(!path.exists());
        delete_session(&path).unwrap(); // deleting non-existent should be ok
    }

    #[test]
    fn test_encode_cwd() {
        assert_eq!(
            encode_cwd_for_dir(Path::new("/home/user/project")),
            "--home-user-project--"
        );
    }

    #[test]
    fn test_compaction_context() {
        let mut s = Session::in_memory(Path::new("/tmp/test"));
        s.append_message(user_msg("old message"));
        s.append_compaction("earlier work summarized", "entry_kept", 5000, None);
        s.append_message(user_msg("new message"));

        let ctx = s.build_context();
        // compaction summary + new message
        assert!(matches!(
            &ctx.messages[0],
            AgentMessage::Llm(Message::User { .. })
        ));
        let text = crate::agent::types::message_text(&ctx.messages[0]);
        assert!(text.contains("earlier work summarized"));
        assert_eq!(ctx.messages.len(), 2);
    }

    #[test]
    fn test_branch_summary_in_context() {
        let mut s = Session::in_memory(Path::new("/tmp/test"));
        s.append_message(user_msg("first"));
        s.append_branch_summary("some_entry", "Abandoned branch work", None);
        s.append_message(asst_msg("continued"));

        let ctx = s.build_context();
        // branch summary should be included as a user message
        assert_eq!(ctx.messages.len(), 3);
        assert!(
            crate::agent::types::message_text(&ctx.messages[1]).contains("Abandoned branch work")
        );
    }

    #[test]
    fn test_open_missing_file() {
        let s = Session::open(Path::new("/nonexistent/file.jsonl"), None);
        assert!(!s.session_id().is_empty());
        assert!(s.get_entries().is_empty());
    }

    #[test]
    fn test_find_entries() {
        let mut s = Session::in_memory(Path::new("/tmp/test"));
        s.append_message(user_msg("hello"));
        s.append_thinking_level_change("high");
        s.append_model_change("p", "m");

        let msgs = s.find_entries("message");
        assert_eq!(msgs.len(), 1);
        let thinking = s.find_entries("thinking_level_change");
        assert_eq!(thinking.len(), 1);
        let models = s.find_entries("model_change");
        assert_eq!(models.len(), 1);
    }

    #[test]
    fn test_session_name() {
        let mut s = Session::in_memory(Path::new("/tmp/test"));
        assert!(s.session_name().is_none());
        s.append_session_info("My Task");
        assert_eq!(s.session_name(), Some("My Task"));
        s.append_session_info("");
        assert_eq!(s.session_name(), Some(""));
    }

    #[test]
    fn test_append_message_with_cost() {
        let mut s = Session::in_memory(Path::new("/tmp/test"));
        let cost = MessageCost::new(0.001, 0.002, 0.0, 0.0);
        let id = s.append_message_with_cost(asst_msg("costly"), cost);
        let stored = s.entry_cost(&id);
        assert!(stored.is_some());
        assert!((stored.unwrap().total - 0.003).abs() < f64::EPSILON);
    }

    #[test]
    fn test_continue_recent_creates_new_when_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let s = Session::continue_recent(&cwd, &sessions_dir).unwrap();
        assert!(!s.session_id().is_empty());
        assert!(s.get_entries().is_empty());
    }

    #[test]
    fn test_full_persistence_flow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut session = crate::agent::AgentSession::create(&cwd, None);
        assert!(!session.is_persisted(), "should not persist on create");

        session.on_model_change("anthropic", "claude-sonnet-4-20250514");
        session.on_thinking_level_change("high");
        assert!(
            !session.is_persisted(),
            "should not persist on metadata only"
        );

        use yoagent::types::*;
        let user_msg = AgentMessage::Llm(Message::User {
            content: vec![Content::Text {
                text: "hello".to_string(),
            }],
            timestamp: 1000,
        });
        session.on_agent_event(&AgentEvent::MessageEnd { message: user_msg });

        assert!(session.is_persisted(), "should persist after first message");
        let path = session.session_file().expect("should have a file path");
        assert!(path.exists(), "file should exist on disk");

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("hello"),
            "file should contain the message text"
        );
        assert!(
            content.contains("claude-sonnet-4-20250514"),
            "file should contain model info"
        );

        // Second message — ensure_flushed now always writes, so it should update the file
        let asst_msg = AgentMessage::Llm(Message::assistant(
            vec![Content::Text {
                text: "hi there".to_string(),
            }],
            yoagent::types::StopReason::Stop,
            "claude-sonnet-4-20250514",
            "anthropic",
            yoagent::types::Usage::default(),
        ));
        session.on_agent_event(&AgentEvent::MessageEnd { message: asst_msg });

        let content2 = std::fs::read_to_string(&path).unwrap();
        assert!(
            content2.contains("hello"),
            "file should still contain user message"
        );
        assert!(
            content2.contains("hi there"),
            "file should contain assistant response"
        );
        assert!(content2.len() > content.len(), "file should have grown");
    }

    // ── cleanup_dangling_tool_calls tests ──────────────────────

    fn asst_with_tool_call(id: &str, name: &str) -> AgentMessage {
        AgentMessage::Llm(
            Message::assistant(
                vec![
                    yoagent::types::Content::Text {
                        text: "Let me calculate that".into(),
                    },
                    yoagent::types::Content::tool_call(
                        id,
                        name,
                        serde_json::json!({ "expression": "2 + 2" }),
                    ),
                ],
                yoagent::types::StopReason::ToolUse,
                "test-model",
                "test-provider",
                yoagent::types::Usage::default(),
            )
            .with_timestamp(yoagent::types::now_ms()),
        )
    }

    fn tool_result_msg(tool_call_id: &str) -> AgentMessage {
        AgentMessage::Llm(Message::ToolResult {
            tool_call_id: tool_call_id.into(),
            tool_name: "test-tool".into(),
            content: vec![yoagent::types::Content::Text {
                text: "4".into(),
            }],
            is_error: false,
            timestamp: yoagent::types::now_ms(),
        })
    }

    #[test]
    fn test_cleanup_dangling_tool_call_inserts_synthetic_result() {
        // Assistant makes a tool call, user responds without providing result.
        let mut msgs = vec![
            user_msg("calculate 2+2"),
            asst_with_tool_call("call_123", "calculate"),
            user_msg("never mind, what's your name?"),
        ];

        cleanup_dangling_tool_calls(&mut msgs);

        // Should have 4 messages: user, assistant (tool call), synthetic tool result, user
        assert_eq!(msgs.len(), 4);

        // The synthetic tool result should be an error with "No result provided"
        if let AgentMessage::Llm(Message::ToolResult {
            tool_call_id,
            is_error,
            content,
            ..
        }) = &msgs[2]
        {
            assert_eq!(tool_call_id, "call_123");
            assert!(*is_error);
            if let yoagent::types::Content::Text { text } = &content[0] {
                assert_eq!(text, "No result provided");
            } else {
                panic!("expected text content");
            }
        } else {
            panic!("expected synthetic ToolResult at index 2");
        }
    }

    #[test]
    fn test_cleanup_preserves_matching_tool_result() {
        // Normal flow: assistant makes tool call, tool result is provided.
        let mut msgs = vec![
            user_msg("calculate 2+2"),
            asst_with_tool_call("call_456", "calculate"),
            tool_result_msg("call_456"),
            user_msg("thanks"),
        ];

        cleanup_dangling_tool_calls(&mut msgs);

        // Should be unchanged — the tool call has a matching result.
        assert_eq!(msgs.len(), 4);
        if let AgentMessage::Llm(Message::ToolResult {
            tool_call_id, is_error, ..
        }) = &msgs[2]
        {
            assert_eq!(tool_call_id, "call_456");
            assert!(!*is_error, "original result should not be marked error");
        } else {
            panic!("expected ToolResult at index 2");
        }
    }

    #[test]
    fn test_cleanup_skips_errored_assistant() {
        // Errored/aborted assistant messages should be removed entirely.
        let mut msgs = vec![
            user_msg("hello"),
            AgentMessage::Llm(Message::assistant(
                vec![],
                yoagent::types::StopReason::Error,
                "test-model",
                "test-provider",
                yoagent::types::Usage::default(),
            )
            .with_timestamp(yoagent::types::now_ms())),
            user_msg("hello again"),
        ];

        cleanup_dangling_tool_calls(&mut msgs);

        // The errored assistant should be removed.
        assert_eq!(msgs.len(), 2);
        assert!(
            matches!(&msgs[0], AgentMessage::Llm(Message::User { .. })),
            "first msg should be user"
        );
        assert!(
            matches!(&msgs[1], AgentMessage::Llm(Message::User { .. })),
            "second msg should be user"
        );
    }

    #[test]
    fn test_cleanup_handles_end_of_conversation() {
        // Conversation ends with assistant tool calls (user interrupted mid-execution).
        let mut msgs = vec![
            user_msg("do something"),
            asst_with_tool_call("call_789", "some_tool"),
        ];

        cleanup_dangling_tool_calls(&mut msgs);

        // Should have 3 messages: user, assistant, synthetic tool result.
        assert_eq!(msgs.len(), 3);
        if let AgentMessage::Llm(Message::ToolResult {
            tool_call_id, is_error, ..
        }) = &msgs[2]
        {
            assert_eq!(tool_call_id, "call_789");
            assert!(*is_error);
        } else {
            panic!("expected synthetic ToolResult at end");
        }
    }

    #[test]
    fn test_cleanup_consecutive_assistants() {
        // Two assistant messages in a row (usually shouldn't happen, but be safe).
        let mut msgs = vec![
            user_msg("hello"),
            asst_with_tool_call("call_111", "tool_a"),
            // Second assistant without resolving first tool call
            asst_msg("actually let me just answer"),
        ];

        cleanup_dangling_tool_calls(&mut msgs);

        // Should have 4: user, first asst, synthetic result, second asst.
        assert_eq!(msgs.len(), 4);
        if let AgentMessage::Llm(Message::ToolResult {
            tool_call_id, is_error, ..
        }) = &msgs[2]
        {
            assert_eq!(tool_call_id, "call_111");
            assert!(*is_error);
        } else {
            panic!("expected synthetic ToolResult at index 2");
        }
    }

    #[test]
    fn test_cleanup_handles_aborted_assistant() {
        // Aborted assistant messages should be skipped like errored ones.
        let mut msgs = vec![
            user_msg("hello"),
            AgentMessage::Llm(Message::assistant(
                vec![],
                yoagent::types::StopReason::Aborted,
                "test-model",
                "test-provider",
                yoagent::types::Usage::default(),
            )
            .with_timestamp(yoagent::types::now_ms())),
            user_msg("continue"),
        ];

        cleanup_dangling_tool_calls(&mut msgs);

        assert_eq!(msgs.len(), 2);
        assert!(
            matches!(&msgs[0], AgentMessage::Llm(Message::User { .. })),
            "first msg should be user"
        );
        assert!(
            matches!(&msgs[1], AgentMessage::Llm(Message::User { .. })),
            "second msg should be user"
        );
    }

    #[test]
    fn test_cleanup_multiple_orphaned_tool_calls() {
        // Assistant makes multiple tool calls, none are resolved.
        let mut msgs = vec![
            user_msg("do everything"),
            AgentMessage::Llm(
                Message::assistant(
                    vec![
                        yoagent::types::Content::Text {
                            text: "Running all tools".into(),
                        },
                        yoagent::types::Content::tool_call(
                            "call_1",
                            "tool_a",
                            serde_json::json!({}),
                        ),
                        yoagent::types::Content::tool_call(
                            "call_2",
                            "tool_b",
                            serde_json::json!({}),
                        ),
                    ],
                    yoagent::types::StopReason::ToolUse,
                    "test-model",
                    "test-provider",
                    yoagent::types::Usage::default(),
                )
                .with_timestamp(yoagent::types::now_ms()),
            ),
            user_msg("actually stop"),
        ];

        cleanup_dangling_tool_calls(&mut msgs);

        // Should have: user, assistant, synthetic result for call_1, synthetic result for call_2, user
        assert_eq!(msgs.len(), 5);

        // Check both synthetic results exist
        let synthetic_ids: Vec<&str> = msgs
            .iter()
            .filter_map(|msg| {
                if let AgentMessage::Llm(Message::ToolResult {
                    tool_call_id, is_error, ..
                }) = msg
                {
                    if *is_error { Some(tool_call_id.as_str()) } else { None }
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(synthetic_ids, vec!["call_1", "call_2"]);
    }
}
