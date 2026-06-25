use crate::agent::branch_summary::{collect_entries_for_branch_summary, generate_branch_summary};
use crate::agent::compaction::{self, CompactionSettings, compact, prepare_compaction};
use crate::agent::r#loop::AgentEvent;
use crate::agent::provider::Provider;
use crate::agent::session::SessionManager;
use crate::agent::types::{AgentMessage, Role};
use std::collections::HashSet;
use std::sync::Arc;

/// Lifecycle layer that bridges the agent loop and session manager.
///
/// Handles:
/// - Event-driven message persistence (persist tool results as they arrive)
/// - Automatic model/thinking/tool change detection and persistence
/// - Lifecycle hooks for agent start/end
///
/// Usage:
/// ```ignore
/// let mut agent_session = AgentSession::new(session);
///
/// // In your agent event handler:
/// agent_session.handle_event(&event);
///
/// // For model/thinking/tool changes at runtime:
/// agent_session.on_model_change("opencode_go", "deepseek-v4-pro");
/// agent_session.on_thinking_level_change("high");
/// ```
pub struct AgentSession {
    session: SessionManager,
    /// Last known model for change detection.
    last_model: Option<(String, String)>,
    /// Last known thinking level for change detection.
    last_thinking_level: String,
    /// Last known active tool names for change detection.
    last_active_tools: Option<Vec<String>>,
    /// IDs of messages already persisted via event-driven persistence,
    /// to avoid duplicates when AgentEnd fires.
    persisted_message_ids: HashSet<String>,
    /// Tool call IDs already persisted (for tool result dedup).
    persisted_tool_call_ids: HashSet<String>,
    /// Compaction settings (default: enabled).
    compaction_settings: CompactionSettings,
    /// Model context window in tokens (for shouldCompact check).
    context_window: u64,
    /// Model name to use for compaction LLM calls.
    model_name: String,
    /// Provider for compaction LLM calls.
    compaction_provider: Option<Arc<dyn Provider>>,
}

impl AgentSession {
    /// Create a new AgentSession wrapping an existing SessionManager.
    pub fn new(session: SessionManager) -> Self {
        // Snapshot current metadata from the session context for change detection.
        let ctx = session.build_session_context();

        // If the session has no thinking level change entries, set last_thinking_level
        // to empty so the first on_thinking_level_change always detects a change.
        // Pi-compatible: the initial thinking level comes from settings default, not from
        // the session context default ("off"). An empty sentinel ensures the first user
        // cycle is always recorded in the session.
        let has_thinking_entries = !session
            .find_entries_by_type("thinking_level_change")
            .is_empty();
        let last_thinking_level = if has_thinking_entries {
            ctx.thinking_level
        } else {
            String::new()
        };

        Self {
            session,
            last_model: ctx.model,
            last_thinking_level,
            last_active_tools: ctx.active_tool_names,
            persisted_message_ids: HashSet::new(),
            persisted_tool_call_ids: HashSet::new(),
            compaction_settings: CompactionSettings::default(),
            context_window: 200_000,
            model_name: String::new(),
            compaction_provider: None,
        }
    }

    /// Configure compaction with provider, model, and context window.
    pub fn set_compaction_config(
        &mut self,
        provider: Arc<dyn Provider>,
        model_name: &str,
        context_window: u64,
    ) {
        self.compaction_provider = Some(provider);
        self.model_name = model_name.to_string();
        self.context_window = context_window;
    }

    /// Enable or disable auto-compaction.
    pub fn set_auto_compact(&mut self, enabled: bool) {
        self.compaction_settings.enabled = enabled;
    }

    /// Get the current compaction settings (mutable, for modification).
    pub fn compaction_settings_mut(&mut self) -> &mut CompactionSettings {
        &mut self.compaction_settings
    }

    /// Get the current compaction settings.
    pub fn compaction_settings(&self) -> &CompactionSettings {
        &self.compaction_settings
    }

    // ── Accessors ─────────────────────────────────────────────────

    /// Borrow the underlying session manager.
    pub fn session(&self) -> &SessionManager {
        &self.session
    }

    /// Mutably borrow the underlying session manager.
    pub fn session_mut(&mut self) -> &mut SessionManager {
        &mut self.session
    }

    /// Consume the AgentSession and return the inner SessionManager.
    pub fn into_session(self) -> SessionManager {
        self.session
    }

    // ── Model / thinking / tool change tracking ─────────────────

    /// Persist a model change if it differs from the last known model.
    /// Returns true if a change entry was appended.
    pub fn on_model_change(&mut self, provider: &str, model_id: &str) -> bool {
        let new = (provider.to_string(), model_id.to_string());
        if self.last_model.as_ref() != Some(&new) {
            self.session.append_model_change(provider, model_id);
            self.last_model = Some(new);
            true
        } else {
            false
        }
    }

    /// Persist a thinking level change if it differs from the last known level.
    /// Returns true if a change entry was appended.
    pub fn on_thinking_level_change(&mut self, level: &str) -> bool {
        if self.last_thinking_level != level {
            self.session.append_thinking_level_change(level);
            self.last_thinking_level = level.to_string();
            true
        } else {
            false
        }
    }

    /// Persist an active tools change if it differs from the last known set.
    /// Returns true if a change entry was appended.
    pub fn on_active_tools_change(&mut self, tools: &[String]) -> bool {
        let tools_vec = tools.to_vec();
        if self.last_active_tools.as_ref() != Some(&tools_vec) {
            self.session.append_active_tools_change(&tools_vec);
            self.last_active_tools = Some(tools_vec);
            true
        } else {
            false
        }
    }

    // ── User message submission ───────────────────────────────────

    /// Reset the session (creates a new empty session) and clear
    /// all tracked state so the new session starts fresh.
    pub fn new_session(&mut self) {
        self.session.new_session(None);
        self.persisted_message_ids.clear();
        self.persisted_tool_call_ids.clear();
        self.last_model = None;
        self.last_thinking_level = String::new();
        self.last_active_tools = None;
    }

    /// Append a user message to the session and register it as persisted.
    /// Returns the entry id.
    pub fn submit_user_message(&mut self, content: &str) -> String {
        let msg = AgentMessage::user(content);
        let id = self.session.append_message(&msg);
        self.persisted_message_ids.insert(msg.id);
        id
    }

    /// Append a user message (pre-constructed) to the session.
    /// Returns the entry id.
    pub fn submit_user_message_obj(&mut self, msg: &AgentMessage) -> String {
        let id = self.session.append_message(msg);
        self.persisted_message_ids.insert(msg.id.clone());
        id
    }

    // ── Event-driven persistence ──────────────────────────────────

    /// Process an agent event for automatic persistence.
    ///
    /// - `ToolResult` events are persisted immediately (crash-safe).
    /// - `AgentEnd` persists any remaining assistant messages not yet captured.
    ///
    /// Call this from your agent event handler alongside any UI updates.
    pub fn handle_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::ToolResult {
                id,
                content,
                is_error,
                ..
            } => {
                // Persist tool result messages immediately (event-driven, crash-safe).
                let msg = AgentMessage::tool_result(id, content, *is_error);
                self.persist_message(&msg);
            }
            AgentEvent::AgentEnd { messages } => {
                self.on_agent_end(messages);
            }
            _ => {}
        }
    }

    /// Persist all new messages from an agent run that haven't been
    /// persisted yet (e.g. assistant messages not captured by event-driven
    /// persistence, or error messages).
    ///
    /// Call this when the agent loop finishes, or let `handle_event` do it
    /// automatically on `AgentEnd`.
    pub fn on_agent_end(&mut self, messages: &[AgentMessage]) {
        for msg in messages {
            if msg.role == Role::User {
                continue;
            }
            // Skip tool results already persisted via event-driven persistence
            if msg.role == Role::ToolResult
                && let Some(ref tcid) = msg.tool_call_id
                && self.persisted_tool_call_ids.contains(tcid)
            {
                continue;
            }
            if !self.persisted_message_ids.contains(&msg.id) {
                self.session.append_message(msg);
                self.persisted_message_ids.insert(msg.id.clone());
            }
        }
    }

    // ── Compaction ────────────────────────────────────────────────

    /// Check if compaction should run and execute it if needed.
    /// Should be called after the agent finishes a turn (after on_agent_end).
    /// Returns `true` if compaction was performed.
    pub async fn check_auto_compact(&mut self) -> Result<bool, String> {
        if !self.compaction_settings.enabled {
            return Ok(false);
        }
        if self.compaction_provider.is_none() || self.model_name.is_empty() {
            return Ok(false);
        }

        let entries = self.session.entries();
        let context_msgs = self.session.build_session_context().messages;
        let context_tokens = compaction::estimate_context_tokens(&context_msgs);

        if !compaction::should_compact(
            context_tokens,
            self.context_window,
            &self.compaction_settings,
        ) {
            return Ok(false);
        }

        let Some(prep) = prepare_compaction(entries, &self.compaction_settings) else {
            return Ok(false);
        };

        let provider = self.compaction_provider.as_ref().unwrap();
        let result = compact(&prep, provider.as_ref(), &self.model_name, None).await?;

        // Append the compaction entry to the session
        self.session.append_compaction(
            &result.summary,
            &result.first_kept_entry_id,
            result.tokens_before,
            result.details,
            None, // from_hook: pi-generated
        );

        Ok(true)
    }

    /// Run compaction manually (ignores auto-compact setting).
    /// Returns the compaction summary text, or an error message.
    pub async fn run_manual_compact(&mut self) -> Result<String, String> {
        if self.compaction_provider.is_none() || self.model_name.is_empty() {
            return Err("No provider configured for compaction".to_string());
        }

        let entries = self.session.entries();
        let Some(prep) = prepare_compaction(entries, &self.compaction_settings) else {
            return Err("Nothing to compact – session is already compacted or empty".to_string());
        };

        let provider = self.compaction_provider.as_ref().unwrap();
        let result = compact(&prep, provider.as_ref(), &self.model_name, None).await?;

        // Append the compaction entry to the session
        self.session.append_compaction(
            &result.summary,
            &result.first_kept_entry_id,
            result.tokens_before,
            result.details,
            None, // from_hook: pi-generated
        );

        Ok(result.summary)
    }

    // ── Branch summarization ───────────────────────────────────────

    /// Summarise the abandoned branch when navigating to a different node.
    ///
    /// Collects entries between `old_leaf_id` and the common ancestor with
    /// `target_id`, summarises them via the provider, and appends a
    /// `BranchSummaryEntry` to the session.
    ///
    /// Returns the summary text, or an error message.
    pub async fn summarize_branch_navigation(
        &mut self,
        old_leaf_id: Option<&str>,
        target_id: &str,
    ) -> Result<String, String> {
        if self.compaction_provider.is_none() || self.model_name.is_empty() {
            return Err("No provider configured for summarization".to_string());
        }

        let (entries, _common_ancestor) =
            collect_entries_for_branch_summary(self.session(), old_leaf_id, target_id);

        if entries.is_empty() {
            return Err("No abandoned entries to summarize".to_string());
        }

        let provider = self.compaction_provider.as_ref().unwrap();
        generate_branch_summary(
            &mut self.session,
            &entries,
            target_id,
            provider.as_ref(),
            &self.model_name,
        )
        .await
    }

    /// Clean up resources held by the current session (provider connections, etc.).
    /// Should be called before switching to a different session or disposing.
    pub fn cleanup_session_resources(&mut self) {
        // Reset persisted message tracking (they belong to the old session)
        self.persisted_message_ids.clear();
        self.persisted_tool_call_ids.clear();
        // Provider connections will be dropped when the Arc is cleaned up
    }

    /// Move the leaf pointer to an earlier entry (starts a new branch).
    /// Optionally summarizes the abandoned path if a provider is configured.
    /// Returns the branch summary text if summarization was performed.
    pub async fn set_branch(&mut self, branch_from_id: &str) -> Result<Option<String>, String> {
        let old_leaf = self.session.leaf_id().map(|s| s.to_string());

        let summary = if self.compaction_provider.is_some()
            && !self.model_name.is_empty()
            && let Some(ref old) = old_leaf
            && old != branch_from_id
        {
            // Summarize the abandoned path
            match self
                .summarize_branch_navigation(Some(old), branch_from_id)
                .await
            {
                Ok(s) => Some(s),
                Err(e) => {
                    // Non-fatal: still allow the branch move
                    eprintln!("Warning: branch summarization failed: {}", e);
                    None
                }
            }
        } else {
            None
        };

        self.session
            .set_branch(branch_from_id)
            .map_err(|e| format!("Failed to set branch: {}", e))?;

        Ok(summary)
    }

    // ── Internal helpers ──────────────────────────────────────────

    /// Persist a single message, skipping if already persisted (dedup).
    /// Tool results are deduped by tool_call_id; other messages by message id.
    fn persist_message(&mut self, msg: &AgentMessage) {
        // Dedup tool results by tool_call_id
        if msg.role == Role::ToolResult
            && let Some(ref tcid) = msg.tool_call_id
        {
            if self.persisted_tool_call_ids.contains(tcid) {
                return;
            }
            self.session.append_message(msg);
            self.persisted_tool_call_ids.insert(tcid.clone());
            self.persisted_message_ids.insert(msg.id.clone());
            return;
        }
        // Dedup other messages by message id
        if self.persisted_message_ids.contains(&msg.id) {
            return;
        }
        self.session.append_message(msg);
        self.persisted_message_ids.insert(msg.id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    fn make_temp_session() -> (TempDir, SessionManager) {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let sm = SessionManager::create(&cwd, Some(&sessions_dir));
        (tmp, sm)
    }

    fn make_msg(role: Role, content: &str) -> AgentMessage {
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
    fn test_new_tracks_metadata() {
        let (_tmp, mut sm) = make_temp_session();
        sm.append_thinking_level_change("high");
        sm.append_model_change("opencode_go", "deepseek-v4-pro");
        sm.append_active_tools_change(&["read".to_string(), "write".to_string()]);

        let as_ = AgentSession::new(sm);
        assert_eq!(as_.last_thinking_level, "high");
        assert_eq!(
            as_.last_model,
            Some(("opencode_go".to_string(), "deepseek-v4-pro".to_string()))
        );
        assert_eq!(
            as_.last_active_tools,
            Some(vec!["read".to_string(), "write".to_string()])
        );
    }

    #[test]
    fn test_model_change_detection() {
        let (_tmp, sm) = make_temp_session();
        let mut as_ = AgentSession::new(sm);

        // First change should persist
        assert!(as_.on_model_change("opencode_go", "deepseek-v4-pro"));
        assert_eq!(as_.session.entries().len(), 1);

        // Same model again should NOT persist (no duplicate)
        assert!(!as_.on_model_change("opencode_go", "deepseek-v4-pro"));
        assert_eq!(as_.session.entries().len(), 1);

        // Different model should persist
        assert!(as_.on_model_change("opencode_go", "deepseek-v4-flash"));
        assert_eq!(as_.session.entries().len(), 2);
    }

    #[test]
    fn test_thinking_level_change_detection() {
        let (_tmp, sm) = make_temp_session();
        let mut as_ = AgentSession::new(sm);

        assert!(as_.on_thinking_level_change("high"));
        assert_eq!(as_.session.entries().len(), 1);

        assert!(!as_.on_thinking_level_change("high"));
        assert_eq!(as_.session.entries().len(), 1);

        assert!(as_.on_thinking_level_change("off"));
        assert_eq!(as_.session.entries().len(), 2);
    }

    #[test]
    fn test_active_tools_change_detection() {
        let (_tmp, sm) = make_temp_session();
        let mut as_ = AgentSession::new(sm);

        let tools1 = vec!["read".to_string(), "write".to_string()];
        assert!(as_.on_active_tools_change(&tools1));
        assert_eq!(as_.session.entries().len(), 1);

        assert!(!as_.on_active_tools_change(&tools1));
        assert_eq!(as_.session.entries().len(), 1);

        let tools2 = vec!["read".to_string()];
        assert!(as_.on_active_tools_change(&tools2));
        assert_eq!(as_.session.entries().len(), 2);
    }

    #[test]
    fn test_submit_user_message() {
        let (_tmp, sm) = make_temp_session();
        let mut as_ = AgentSession::new(sm);

        let id = as_.submit_user_message("hello");
        assert!(!id.is_empty());
        assert_eq!(as_.session.entries().len(), 1);
        assert!(as_.persisted_message_ids.len() == 1);

        // Check entry is a user message
        match &as_.session.entries()[0] {
            crate::agent::session::SessionEntry::Message(m) => {
                assert_eq!(m.message.role, Role::User);
                assert_eq!(m.message.content, "hello");
            }
            _ => panic!("Expected Message entry"),
        }
    }

    #[test]
    fn test_tool_result_event_persists_immediately() {
        let (_tmp, sm) = make_temp_session();
        let mut as_ = AgentSession::new(sm);

        let event = AgentEvent::ToolResult {
            id: "tool-1".to_string(),
            name: "read".to_string(),
            content: "file content".to_string(),
            compact: None,
            is_error: false,
            details: None,
        };

        as_.handle_event(&event);
        assert_eq!(as_.session.entries().len(), 1);
        assert!(as_.persisted_message_ids.len() == 1);

        // Same event again should be deduped
        as_.handle_event(&event);
        assert_eq!(as_.session.entries().len(), 1);
    }

    #[test]
    fn test_agent_end_persists_remaining() {
        let (_tmp, sm) = make_temp_session();
        let mut as_ = AgentSession::new(sm);

        // Simulate: tool result was already persisted via event
        let tool_result = make_msg(Role::ToolResult, "output");
        as_.persist_message(&tool_result);

        // AgentEnd brings back all messages including the assistant msg
        let assistant_msg = make_msg(Role::Assistant, "hello from AI");
        let all_messages = vec![tool_result, assistant_msg.clone()];

        as_.handle_event(&AgentEvent::AgentEnd {
            messages: all_messages.clone(),
        });

        // tool_result was already persisted (dedup), assistant_msg added
        assert_eq!(as_.session.entries().len(), 2);
    }

    #[test]
    fn test_agent_end_skips_user_messages() {
        let (_tmp, sm) = make_temp_session();
        let mut as_ = AgentSession::new(sm);

        let user_msg = make_msg(Role::User, "prompt");
        let assistant_msg = make_msg(Role::Assistant, "response");

        as_.handle_event(&AgentEvent::AgentEnd {
            messages: vec![user_msg.clone(), assistant_msg.clone()],
        });

        // Only assistant message should be persisted (user messages are
        // persisted separately via submit_user_message)
        assert_eq!(as_.session.entries().len(), 1);
    }

    #[test]
    fn test_into_session() {
        let (_tmp, sm) = make_temp_session();
        let mut as_ = AgentSession::new(sm);
        as_.submit_user_message("test");
        let session = as_.into_session();
        assert_eq!(session.entries().len(), 1);
    }

    #[test]
    fn test_event_no_duplicate_on_agent_end() {
        let (_tmp, sm) = make_temp_session();
        let mut as_ = AgentSession::new(sm);

        // Tool result arrives during streaming
        let tool_event = AgentEvent::ToolResult {
            id: "t1".to_string(),
            name: "bash".to_string(),
            content: "done".to_string(),
            compact: None,
            is_error: false,
            details: None,
        };
        as_.handle_event(&tool_event);

        // AgentEnd fires with same tool result + assistant message
        let tool_msg = AgentMessage::tool_result("t1", "done", false);
        let asst_msg = make_msg(Role::Assistant, "ok");
        as_.handle_event(&AgentEvent::AgentEnd {
            messages: vec![tool_msg, asst_msg],
        });

        // Should have: tool result (once) + assistant message
        assert_eq!(as_.session.entries().len(), 2);
    }
}
