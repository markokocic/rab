use crate::agent::branch_summary::{collect_entries_for_branch_summary, generate_branch_summary};
use crate::agent::compaction::{
    self, CompactionReason, CompactionResult, CompactionSettings, compact, prepare_compaction,
};
use crate::agent::extension::Extension;
use crate::agent::session::SessionManager;
use crate::agent::types::{message_text, tool_result_message, user_message};
use std::collections::HashSet;
use yoagent::types::AgentMessage;
use yoagent::types::Content;

// ── Compaction lifecycle events ─────────────────────────────────────

/// Events emitted during the compaction lifecycle.
/// Matches pi's `compaction_start` / `compaction_end` event semantics.
#[derive(Debug, Clone)]
pub enum CompactionEvent {
    /// Compaction has started with the given reason.
    Start { reason: CompactionReason },
    /// Compaction completed successfully.
    End {
        reason: CompactionReason,
        result: CompactionResult,
        aborted: bool,
        will_retry: bool,
        error_message: Option<String>,
    },
}

/// Callback for compaction lifecycle events.
pub type CompactionEventCallback = Box<dyn Fn(&CompactionEvent) + Send + Sync>;

/// Bridges the agent loop events and session persistence.
///
/// Handles:
/// - Event-driven message persistence (persist tool results as they arrive)
/// - Automatic model/thinking/tool change detection and persistence
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
    /// API key for compaction LLM calls.
    compaction_api_key: Option<String>,
    /// Model configuration for compaction LLM calls (base URL, compat flags, etc.).
    model_config: Option<yoagent::provider::model::ModelConfig>,
    /// Current thinking level from the session (for compaction summarization).
    thinking_level: yoagent::types::ThinkingLevel,
    /// Registered extensions (for compaction hooks).
    extensions: Vec<Box<dyn Extension>>,
    /// Lifecycle event listeners.
    event_listeners: Vec<CompactionEventCallback>,
    /// Whether overflow recovery has already been attempted (prevents loops).
    overflow_recovery_attempted: bool,
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
            compaction_api_key: None,
            model_config: None,
            thinking_level: yoagent::types::ThinkingLevel::Off,
            extensions: Vec::new(),
            event_listeners: Vec::new(),
            overflow_recovery_attempted: false,
        }
    }

    /// Configure compaction with API key, model, context window, and model config.
    pub fn set_compaction_config(
        &mut self,
        api_key: String,
        model_name: &str,
        context_window: u64,
        model_config: Option<yoagent::provider::model::ModelConfig>,
    ) {
        self.compaction_api_key = Some(api_key);
        self.model_name = model_name.to_string();
        self.context_window = context_window;
        self.model_config = model_config;
    }

    /// Enable or disable auto-compaction.
    pub fn set_auto_compact(&mut self, enabled: bool) {
        self.compaction_settings.enabled = enabled;
    }

    /// Sync the thinking level from the session context.
    /// Should be called after the session context changes.
    pub fn sync_thinking_level(&mut self) {
        let ctx = self.session.build_session_context();
        let level_str = ctx.thinking_level.to_lowercase();
        self.thinking_level = match level_str.as_str() {
            "off" => yoagent::types::ThinkingLevel::Off,
            "minimal" => yoagent::types::ThinkingLevel::Minimal,
            "low" => yoagent::types::ThinkingLevel::Low,
            "medium" => yoagent::types::ThinkingLevel::Medium,
            "high" => yoagent::types::ThinkingLevel::High,
            _ => yoagent::types::ThinkingLevel::Off,
        };
    }

    /// Get the current compaction settings (mutable, for modification).
    pub fn compaction_settings_mut(&mut self) -> &mut CompactionSettings {
        &mut self.compaction_settings
    }

    /// Get the current compaction settings.
    pub fn compaction_settings(&self) -> &CompactionSettings {
        &self.compaction_settings
    }

    /// Set the list of extensions (for compaction hooks).
    pub fn set_extensions(&mut self, extensions: Vec<Box<dyn Extension>>) {
        self.extensions = extensions;
    }

    /// Register a compaction lifecycle event listener.
    pub fn on_compaction_event(&mut self, callback: CompactionEventCallback) {
        self.event_listeners.push(callback);
    }

    /// Emit a compaction event to all registered listeners.
    fn emit_compaction_event(&self, event: &CompactionEvent) {
        for listener in &self.event_listeners {
            listener(event);
        }
    }

    /// Reset overflow recovery state (called when starting a new turn).
    pub fn reset_overflow_recovery(&mut self) {
        self.overflow_recovery_attempted = false;
    }

    /// Check if a provider error indicates context overflow.
    /// Matches pi's context overflow detection patterns.
    pub fn is_context_overflow_error(msg: &AgentMessage) -> bool {
        let text = message_text(msg);
        let lower = text.to_lowercase();
        // Pi-compatible: detect HTTP 413, "prompt too long", "context_length_exceeded", etc.
        lower.contains("413")
            || lower.contains("request_too_large")
            || lower.contains("prompt too long")
            || lower.contains("context_length_exceeded")
            || lower.contains("context overflow")
            || lower.contains("max context length")
            || lower.contains("exceeded max tokens")
            || lower.contains("maximum context length")
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
    pub fn send_user_message(&mut self, content: &str) -> String {
        let msg = user_message(content);
        let id = self.session.append_message(&msg);
        self.persisted_message_ids.insert(message_text(&msg));
        id
    }

    /// Append a user message (pre-constructed) to the session.
    /// Returns the entry id.
    pub fn send_user_message_obj(&mut self, msg: &AgentMessage) -> String {
        let id = self.session.append_message(msg);
        self.persisted_message_ids.insert(message_text(msg));
        id
    }

    // ── Event-driven persistence ──────────────────────────────────

    /// Process an agent event for automatic persistence.
    ///
    /// - `ToolResult` events are persisted immediately (crash-safe).
    /// - `AgentEnd` persists any remaining assistant messages not yet captured.
    ///
    /// Call this from your agent event handler alongside any UI updates.
    /// Handle a yoagent AgentEvent for session persistence.
    pub fn handle_yo_event(&mut self, event: &yoagent::types::AgentEvent) {
        use yoagent::types::AgentEvent as YoEvent;
        match event {
            YoEvent::ToolExecutionEnd {
                tool_call_id,
                result,
                is_error,
                ..
            } => {
                let content = result
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let Content::Text { text } = c {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let msg = tool_result_message(tool_call_id, content, *is_error);
                self.persist_message(&msg);
            }
            YoEvent::AgentEnd { messages } => {
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
            if crate::agent::types::message_is_user(msg) {
                continue;
            }
            // Skip tool results already persisted via event-driven persistence
            if crate::agent::types::message_is_tool_result(msg)
                && let Some(tcid) = crate::agent::types::message_tool_call_id(msg)
                && self.persisted_tool_call_ids.contains(tcid)
            {
                continue;
            }
            if !self.persisted_message_ids.contains(&message_text(msg)) {
                self.session.append_message(msg);
                self.persisted_message_ids.insert(message_text(msg));
            }
        }
    }

    // ── Compaction ────────────────────────────────────────────────

    /// Check if compaction should run and execute it if needed.
    /// Should be called after the agent finishes a turn (after on_agent_end).
    /// Returns `true` if compaction was performed.
    pub async fn check_auto_compact(&mut self) -> Result<bool, String> {
        Ok(self
            ._run_compaction(CompactionReason::Threshold, None, false)
            .await?
            .is_some())
    }

    /// Run compaction after a context overflow error.
    /// If `will_retry` is true, the agent turn will be retried after compaction.
    /// Returns `Ok(true)` if compaction was performed, `Ok(false)` if recovery already attempted.
    pub async fn check_overflow_compact(&mut self, will_retry: bool) -> Result<bool, String> {
        if self.overflow_recovery_attempted {
            return Ok(false);
        }
        self.overflow_recovery_attempted = true;
        Ok(self
            ._run_compaction(CompactionReason::Overflow, None, will_retry)
            .await?
            .is_some())
    }

    /// Run compaction manually (ignores auto-compact setting).
    /// Returns the compaction summary text, or an error message.
    pub async fn run_manual_compact(
        &mut self,
        custom_instructions: Option<&str>,
    ) -> Result<String, String> {
        let result = self
            ._run_compaction(CompactionReason::Manual, custom_instructions, false)
            .await?;
        Ok(result.map(|r| r.summary).unwrap_or_default())
    }

    /// Internal: run compaction with the given reason, emitting lifecycle events.
    /// Returns the CompactionResult if compaction was performed, or None if skipped.
    async fn _run_compaction(
        &mut self,
        reason: CompactionReason,
        custom_instructions: Option<&str>,
        will_retry: bool,
    ) -> Result<Option<CompactionResult>, String> {
        // For threshold compaction, check if auto-compact is enabled
        if reason == CompactionReason::Threshold && !self.compaction_settings.enabled {
            return Ok(None);
        }

        if self.compaction_api_key.is_none() || self.model_name.is_empty() {
            return Ok(None);
        }

        // Emit compaction_start
        self.emit_compaction_event(&CompactionEvent::Start { reason });

        let entries = self.session.entries();

        // Check threshold for auto-compact
        if reason == CompactionReason::Threshold {
            let context_msgs = self.session.build_session_context().messages;
            let context_tokens = compaction::estimate_context_tokens(&context_msgs);
            if !compaction::should_compact(
                context_tokens,
                self.context_window,
                &self.compaction_settings,
            ) {
                return Ok(None);
            }
        }

        let Some(prep) = prepare_compaction(entries, &self.compaction_settings) else {
            return Ok(None);
        };

        // Extension hooks: before_compact
        let mut from_hook = false;
        let mut hook_summary: Option<String> = None;
        let mut hook_details: Option<serde_json::Value> = None;

        for ext in &self.extensions {
            if let Some(result) = ext.before_compact(
                &prep.first_kept_entry_id,
                prep.tokens_before,
                &reason.to_string(),
            ) {
                if result.cancel {
                    self.emit_compaction_event(&CompactionEvent::End {
                        reason,
                        aborted: true,
                        will_retry: false,
                        error_message: Some("Compaction cancelled by extension".to_string()),
                        result: CompactionResult {
                            summary: String::new(),
                            first_kept_entry_id: prep.first_kept_entry_id.clone(),
                            tokens_before: prep.tokens_before,
                            estimated_tokens_after: 0,
                            details: None,
                        },
                    });
                    return Ok(None);
                }
                if result.summary.is_some() {
                    hook_summary = result.summary;
                    hook_details = result.details;
                    from_hook = true;
                    break;
                }
            }
        }

        let result = if let Some(summary) = hook_summary {
            // Extension provided custom summary
            CompactionResult {
                summary,
                first_kept_entry_id: prep.first_kept_entry_id.clone(),
                tokens_before: prep.tokens_before,
                estimated_tokens_after: 0, // will be computed after append
                details: hook_details,
            }
        } else {
            // Call provider for summarization
            let api_key = self.compaction_api_key.as_ref().unwrap();
            compact(
                &prep,
                api_key,
                &self.model_name,
                custom_instructions,
                self.thinking_level,
                self.model_config.clone(),
            )
            .await?
        };

        // Append the compaction entry to the session
        self.session.append_compaction(
            &result.summary,
            &result.first_kept_entry_id,
            result.tokens_before,
            result.details.clone(),
            Some(from_hook),
        );

        // Compute estimated tokens after compaction
        let context_after = self.session.build_session_context().messages;
        let estimated_tokens_after = compaction::estimate_context_tokens(&context_after);

        let final_result = CompactionResult {
            estimated_tokens_after,
            ..result
        };

        // Extension hooks: after_compact
        for ext in &self.extensions {
            ext.after_compact(
                &final_result.summary,
                &final_result.first_kept_entry_id,
                final_result.tokens_before,
                final_result.estimated_tokens_after,
                from_hook,
                &reason.to_string(),
            );
        }

        // Emit compaction_end
        self.emit_compaction_event(&CompactionEvent::End {
            reason,
            result: final_result.clone(),
            aborted: false,
            will_retry,
            error_message: None,
        });

        Ok(Some(final_result))
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
        if self.compaction_api_key.is_none() || self.model_name.is_empty() {
            return Err("No provider configured for summarization".to_string());
        }

        let (entries, _common_ancestor) =
            collect_entries_for_branch_summary(self.session(), old_leaf_id, target_id);

        if entries.is_empty() {
            return Err("No abandoned entries to summarize".to_string());
        }

        let api_key = self.compaction_api_key.as_ref().unwrap();
        generate_branch_summary(
            &mut self.session,
            &entries,
            target_id,
            api_key,
            &self.model_name,
            self.thinking_level,
            self.model_config.clone(),
        )
        .await
    }

    /// Move the leaf pointer to an earlier entry (starts a new branch).
    /// Optionally summarizes the abandoned path if a provider is configured.
    /// Returns the branch summary text if summarization was performed.
    pub async fn set_branch(&mut self, branch_from_id: &str) -> Result<Option<String>, String> {
        let old_leaf = self.session.leaf_id().map(|s| s.to_string());

        let summary = if self.compaction_api_key.is_some()
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

    /// Persist a tool result message (public so the agent loop can persist crash-safely).
    /// Deduplicates by tool_call_id.
    pub fn persist_tool_result(&mut self, tool_call_id: &str, content: String, is_error: bool) {
        let msg = tool_result_message(tool_call_id, content, is_error);
        self.persist_message(&msg);
    }

    /// Persist an Extension message as a `custom_message` session entry (pi-compatible).
    /// Extension messages are NOT persisted as regular messages — they use the
    /// `custom_message` entry type which supports `custom_type`, `display`, and `details`.
    pub fn persist_extension_message(&mut self, msg: &AgentMessage) {
        let Some(kind) = crate::agent::types::message_extension_kind(msg) else {
            return;
        };
        let text = crate::agent::types::message_extension_text(msg)
            .unwrap_or_else(|| crate::agent::types::message_text(msg));
        let content = serde_json::json!({"text": text});
        self.session
            .append_custom_message_entry(kind, content, true, None);
    }

    /// Persist a single message on `message_end` (pi-compatible pattern).
    ///
    /// Pi persists every message (user, assistant, toolResult) immediately on `message_end`,
    /// not deferred to `agent_end`. This method handles dedup for tool results (already
    /// persisted via `persist_tool_result`) and dedup by text for other message types.
    pub fn persist_message_end(&mut self, msg: &AgentMessage) {
        // Tool results are already persisted crash-safely via persist_tool_result on
        // ToolExecutionEnd — skip them here to avoid duplicates.
        if crate::agent::types::message_is_tool_result(msg)
            && let Some(tcid) = crate::agent::types::message_tool_call_id(msg)
            && self.persisted_tool_call_ids.contains(tcid)
        {
            return;
        }
        // Use persist_message for dedup (checks both tool_call_id and text)
        self.persist_message(msg);
    }

    // ── Internal helpers ──────────────────────────────────────────

    /// Persist a single message, skipping if already persisted (dedup).
    /// Tool results are deduped by tool_call_id; other messages by text.
    fn persist_message(&mut self, msg: &AgentMessage) {
        // Dedup tool results by tool_call_id
        if crate::agent::types::message_is_tool_result(msg)
            && let Some(tcid) = crate::agent::types::message_tool_call_id(msg)
        {
            if self.persisted_tool_call_ids.contains(tcid) {
                return;
            }
            self.session.append_message(msg);
            self.persisted_tool_call_ids.insert(tcid.to_string());
            self.persisted_message_ids.insert(message_text(msg));
            return;
        }
        // Dedup other messages by text
        if self.persisted_message_ids.contains(&message_text(msg)) {
            return;
        }
        self.session.append_message(msg);
        self.persisted_message_ids.insert(message_text(msg));
    }
}
