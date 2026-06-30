use crate::agent::branch_summary::{collect_entries_for_branch_summary, generate_branch_summary};
use crate::agent::compaction::{
    self, CompactionReason, CompactionResult, CompactionSettings, compact, prepare_compaction,
};
use crate::agent::extension::Extension;
use crate::agent::session::SessionManager;
use crate::agent::session_storage::{InMemorySessionStorage, SessionMetadata, SessionStorage};
use crate::agent::types::{message_dedup_key, message_text, tool_result_message, user_message};
use std::collections::HashSet;
use std::sync::Arc;

use crate::provider::ProviderRegistry;
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
pub struct AgentSession {
    /// The core session (wraps SessionStorage).
    session: crate::agent::session::Session,
    /// Session storage directory on disk.
    session_dir: std::path::PathBuf,
    /// Working directory for this session.
    cwd: std::path::PathBuf,
    /// Whether session persistence is enabled.
    persist: bool,
    /// Whether the session file has been written at least once (lazy write).
    flushed: bool,
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
    /// Cancellation token for in-progress compaction (pi-compatible abort).
    compaction_cancel: crate::agent::extension::Cancel,
    /// Provider registry for resolving model cost configs per message (pi-style).
    registry: Option<Arc<ProviderRegistry>>,
}

impl AgentSession {
    /// Create a new AgentSession from a SessionManager (extracts inner Session + config).
    pub fn new(mgr: SessionManager) -> Self {
        // Snapshot current metadata from the session context for change detection.
        let ctx = mgr.build_session_context();

        // Extract config before consuming mgr
        let cwd = mgr.cwd().to_path_buf();
        let session_dir = mgr.session_dir().to_path_buf();
        let persist = mgr.is_persisted();
        let session = mgr.into_session();

        // If the session has no thinking level change entries, set last_thinking_level
        // to empty so the first on_thinking_level_change always detects a change.
        let has_thinking_entries = !session.find_entries("thinking_level_change").is_empty();
        let last_thinking_level = if has_thinking_entries {
            ctx.thinking_level
        } else {
            String::new()
        };

        Self {
            session,
            session_dir,
            cwd,
            persist,
            flushed: false,
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
            compaction_cancel: crate::agent::extension::Cancel::new(),
            registry: None,
        }
    }

    // ── Static factory methods ─────────────────────────────────

    /// Create a new persisted session.
    pub fn create(cwd: &std::path::Path, session_dir: Option<&std::path::Path>) -> Self {
        Self::new(SessionManager::create(cwd, session_dir))
    }

    /// Open a specific session file.
    pub fn open(
        path: &std::path::Path,
        session_dir: Option<&std::path::Path>,
        cwd_override: Option<&std::path::Path>,
    ) -> Self {
        Self::new(SessionManager::open(path, session_dir, cwd_override))
    }

    /// Create an in-memory session (no persistence).
    pub fn in_memory(cwd: &std::path::Path) -> Self {
        Self::new(SessionManager::in_memory(cwd))
    }

    /// Continue most recent session or create new.
    pub fn continue_recent(cwd: &std::path::Path, session_dir: Option<&std::path::Path>) -> Self {
        Self::new(SessionManager::continue_recent(cwd, session_dir))
    }

    /// Fork a session from another project directory.
    pub fn fork_from(
        source_path: &std::path::Path,
        target_cwd: &std::path::Path,
        session_dir: Option<&std::path::Path>,
        options: Option<&crate::agent::session::NewSessionOptions>,
    ) -> std::io::Result<Self> {
        SessionManager::fork_from(source_path, target_cwd, session_dir, options).map(Self::new)
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

    /// Set the provider registry for per-message cost computation (pi-style).
    pub fn set_registry(&mut self, registry: Arc<ProviderRegistry>) {
        self.registry = Some(registry);
    }

    /// Compute the cost of a message in USD using its model's cost config.
    /// Returns 0.0 if the message is not an assistant message or if the model
    /// can't be resolved.
    fn message_cost(&self, msg: &AgentMessage) -> f64 {
        let Some(yoagent::types::Message::Assistant {
            usage,
            model,
            provider,
            ..
        }) = msg.as_llm()
        else {
            return 0.0;
        };

        let Some(ref registry) = self.registry else {
            return 0.0;
        };

        match registry.resolve(model, Some(provider)) {
            Ok(resolved) => {
                let cost = &resolved.model_config.cost;
                (usage.input as f64 * cost.input_per_million
                    + usage.output as f64 * cost.output_per_million
                    + usage.cache_read as f64 * cost.cache_read_per_million
                    + usage.cache_write as f64 * cost.cache_write_per_million)
                    / 1_000_000.0
            }
            Err(_) => 0.0,
        }
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

    /// Abort any in-progress compaction (matching pi's `abortCompaction()`).
    /// The cancellation will be picked up by extension hooks on their next
    /// `cancel.is_cancelled()` check.
    pub fn abort_compaction(&self) {
        self.compaction_cancel.cancel();
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
        self.compaction_cancel = crate::agent::extension::Cancel::new();
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
    /// Borrow the underlying Session.
    pub fn session(&self) -> &crate::agent::session::Session {
        &self.session
    }

    /// Mutably borrow the underlying Session.
    pub fn session_mut(&mut self) -> &mut crate::agent::session::Session {
        &mut self.session
    }

    /// Consume and return the inner Session.
    pub fn into_session(self) -> crate::agent::session::Session {
        self.session
    }

    /// Ensure the session file has been written (lazy write on first assistant message).
    pub fn ensure_flushed(&mut self) {
        if self.flushed || !self.persist {
            return;
        }
        let id = self.session.session_id();
        let cwd_str = self.cwd.to_string_lossy().to_string();
        let parent_session = self.session.metadata().parent_session_path.clone();
        let created_at = self.session.metadata().created_at.clone();
        let file_ts = created_at.replace([':', '.'], "-");
        let file_path = self.session_dir.join(format!("{}_{}.jsonl", file_ts, id));

        let existing_entries = self.session.get_entries();

        match crate::agent::session_storage::JsonlSessionStorage::create(
            file_path,
            &cwd_str,
            &id,
            parent_session,
        ) {
            Ok(mut file_storage) => {
                for entry in &existing_entries {
                    if let Err(e) = file_storage.append_entry(entry.clone()) {
                        eprintln!("Warning: failed to write entry to session file: {}", e);
                    }
                }
                self.session = crate::agent::session::Session::new(Box::new(file_storage));
                self.flushed = true;
            }
            Err(e) => {
                eprintln!("Warning: failed to create session file: {}", e);
                self.flushed = true;
            }
        }
    }

    // ── App-level accessors ────────────────────────────────────

    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

    pub fn session_dir(&self) -> &std::path::Path {
        &self.session_dir
    }

    pub fn is_persisted(&self) -> bool {
        self.persist
    }

    pub fn session_id(&self) -> String {
        self.session.session_id()
    }

    pub fn session_file(&self) -> Option<std::path::PathBuf> {
        self.session.session_file()
    }

    pub fn session_name(&self) -> Option<String> {
        self.session.session_name()
    }

    // ── Model / thinking / tool change tracking ─────────────────

    /// Persist a model change if it differs from the last known model.
    /// Pi-compatible: writes immediately to the session.
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
    /// Pi-compatible: writes immediately to the session.
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
    /// Pi-compatible: writes immediately to the session.
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
        // Create a fresh in-memory session
        let meta = SessionMetadata {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            cwd: self.cwd.to_string_lossy().to_string(),
            path: None,
            parent_session_path: None,
        };
        let storage = Box::new(InMemorySessionStorage::new(meta));
        self.session = crate::agent::session::Session::new(storage);
        self.flushed = false;
        self.persisted_message_ids.clear();
        self.persisted_tool_call_ids.clear();
        self.last_model = None;
        self.last_thinking_level = String::new();
        self.last_active_tools = None;
        self.compaction_cancel = crate::agent::extension::Cancel::new();
    }

    /// Append a user message to the session and register it as persisted.
    /// Returns the entry id.
    pub fn send_user_message(&mut self, content: &str) -> String {
        let msg = user_message(content);
        let id = self.session.append_message(&msg);
        self.persisted_message_ids.insert(message_dedup_key(&msg));
        id
    }

    /// Append a user message (pre-constructed) to the session.
    /// Returns the entry id.
    pub fn send_user_message_obj(&mut self, msg: &AgentMessage) -> String {
        let id = self.session.append_message(msg);
        self.persisted_message_ids.insert(message_dedup_key(msg));
        id
    }

    // ── Event-driven persistence ──────────────────────────────────

    /// Process an agent event for automatic persistence (pi-compatible).
    ///
    /// - `ToolResult` events are persisted immediately (crash-safe).
    /// - `MessageEnd` persists every message in real-time (pi-compatible, crash-safe).
    /// - `AgentEnd` persists any remaining assistant messages not yet captured.
    ///
    /// Call this from your agent event handler alongside any UI updates.
    /// This is the mode-agnostic persistence handler, matching pi's `_handleAgentEvent`.
    pub fn on_agent_event(&mut self, event: &yoagent::types::AgentEvent) {
        use yoagent::types::AgentEvent as YoEvent;
        match event {
            YoEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
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
                let msg = tool_result_message(tool_call_id, tool_name, content, *is_error);
                self.persist_message(&msg);
            }
            YoEvent::MessageEnd { message } => {
                // Pi-compatible: reset overflow recovery when a user message arrives
                // (matches pi's _overflowRecoveryAttempted reset in message_start for user role).
                if crate::agent::types::message_is_user(message) {
                    self.reset_overflow_recovery();
                }
                // Pi-compatible: persist every message immediately on message_end,
                // not deferred to agent_end. Extension messages use custom_message
                // entries (excluded from LLM context); all others use regular messages.
                if crate::agent::types::message_is_extension(message) {
                    self.persist_extension_message(message);
                } else {
                    self.persist_message_end(message);
                }
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
            // Skip Llm-form error messages — they're already persisted as
            // Extension (custom_message) in the MessageEnd handler and should
            // not be persisted again as Llm messages, which would be included
            // in the LLM context on subsequent turns.
            if crate::agent::types::message_error(msg).is_some() {
                continue;
            }
            // Skip tool results already persisted via event-driven persistence
            if crate::agent::types::message_is_tool_result(msg)
                && let Some(tcid) = crate::agent::types::message_tool_call_id(msg)
                && self.persisted_tool_call_ids.contains(tcid)
            {
                continue;
            }
            if !self.persisted_message_ids.contains(&message_dedup_key(msg)) {
                let cost = self.message_cost(msg);
                self.session.append_message_with_cost(msg, cost);
                self.persisted_message_ids.insert(message_dedup_key(msg));
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

        // Create a fresh cancellation token for this compaction run
        // (pi-compatible: matches AbortController per compaction call)
        self.compaction_cancel = crate::agent::extension::Cancel::new();
        let cancel = self.compaction_cancel.clone();

        // Emit compaction_start
        self.emit_compaction_event(&CompactionEvent::Start { reason });

        // Check for cancellation before proceeding
        if cancel.is_cancelled() {
            return Ok(None);
        }

        let entries = self.session.get_entries();

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

        let Some(prep) = prepare_compaction(&entries, &self.compaction_settings) else {
            return Ok(None);
        };

        // Extension hooks: before_compact
        let mut from_hook = false;
        let mut hook_summary: Option<String> = None;
        let mut hook_details: Option<serde_json::Value> = None;

        for ext in &self.extensions {
            if cancel.is_cancelled() {
                break;
            }
            if let Some(result) = ext.before_compact(
                &prep.first_kept_entry_id,
                prep.tokens_before,
                &reason.to_string(),
                &cancel,
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
            if cancel.is_cancelled() {
                break;
            }
            ext.after_compact(
                &final_result.summary,
                &final_result.first_kept_entry_id,
                final_result.tokens_before,
                final_result.estimated_tokens_after,
                from_hook,
                &reason.to_string(),
                &cancel,
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
        let old_leaf = self.session.get_leaf_id();

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
            .set_leaf_id(Some(branch_from_id))
            .map_err(|e| format!("Failed to set branch: {}", e))?;

        Ok(summary)
    }

    /// Persist a tool result message (public so the agent loop can persist crash-safely).
    /// Deduplicates by tool_call_id.
    pub fn persist_tool_result(
        &mut self,
        tool_call_id: &str,
        tool_name: &str,
        content: String,
        is_error: bool,
    ) {
        let msg = tool_result_message(tool_call_id, tool_name, content, is_error);
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
    /// Persist a message directly (pi-compatible: messages are written immediately, not queued).
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
            self.persisted_message_ids.insert(message_dedup_key(msg));
            return;
        }
        // Dedup other messages by dedup key (role + content signature)
        if self.persisted_message_ids.contains(&message_dedup_key(msg)) {
            return;
        }
        // Compute cost for assistant messages using the message's own model
        // (pi-style: cost is pre-computed and stored per message).
        let cost = self.message_cost(msg);
        self.session.append_message_with_cost(msg, cost);
        self.persisted_message_ids.insert(message_dedup_key(msg));
    }
}
