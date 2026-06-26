use crate::agent::branch_summary::{collect_entries_for_branch_summary, generate_branch_summary};
use crate::agent::compaction::{self, CompactionSettings, compact, prepare_compaction};
use crate::agent::extension::Extension;
use crate::agent::session::SessionManager;
use crate::agent::types::{message_text, tool_result_message, user_message};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use yoagent::agent::Agent;
use yoagent::provider::OpenAiCompatProvider;
use yoagent::provider::model::ModelConfig;
use yoagent::types::AgentEvent;
use yoagent::types::AgentMessage;
use yoagent::types::AgentTool;
use yoagent::types::Content;
use yoagent::types::ThinkingLevel;

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
///
/// Build parameters for initializing and recreating the Agent.
#[derive(Clone)]
pub struct AgentBuildParams {
    pub model: String,
    pub api_key: String,
    pub system_prompt: String,
    pub thinking_level: ThinkingLevel,
    pub model_config: ModelConfig,
}
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

    // ── Persistent agent (pi-compatible lifecycle) ──
    /// The Agent, stored between turns. Taken on prompt(), returned on completion.
    agent_slot: Arc<StdMutex<Option<Agent>>>,
    /// Cancellation token for the current turn's agent loop.
    cancel_token: Arc<StdMutex<Option<CancellationToken>>>,
    /// JoinHandle of the current turn's spawned task (for abort).
    current_turn_handle: Arc<StdMutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Persistent event sender (cloned to spawned tasks).
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    /// Extensions (used to rebuild tools if agent is recreated after abort).
    extensions: Option<Arc<Vec<Box<dyn Extension>>>>,
    /// Stored build parameters for recreating the agent after abort.
    build_params: Option<AgentBuildParams>,
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
            agent_slot: Arc::new(StdMutex::new(None)),
            cancel_token: Arc::new(StdMutex::new(None)),
            current_turn_handle: Arc::new(StdMutex::new(None)),
            event_tx: None,
            extensions: None,
            build_params: None,
        }
    }

    /// Configure compaction with API key, model, and context window.
    pub fn set_compaction_config(
        &mut self,
        api_key: String,
        model_name: &str,
        context_window: u64,
    ) {
        self.compaction_api_key = Some(api_key);
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

    // ── Persistent agent lifecycle (pi-compatible) ─────────────────

    /// Initialize the persistent Agent. Call once before the first turn.
    /// The agent is stored in `agent_slot` and reused across turns.
    pub fn init_agent(
        &mut self,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
        extensions: Arc<Vec<Box<dyn Extension>>>,
        params: AgentBuildParams,
        initial_messages: Vec<AgentMessage>,
    ) {
        self.build_params = Some(params.clone());
        self.event_tx = Some(event_tx);
        self.extensions = Some(extensions);

        let tools = self._collect_tools();
        let agent = Self::_build_agent(
            &params.model,
            &params.api_key,
            &params.system_prompt,
            params.thinking_level,
            &params.model_config,
            tools,
            initial_messages,
        );
        self.agent_slot.lock().unwrap().replace(agent);
    }

    /// Check whether the persistent agent has been initialized.
    pub fn is_agent_initialized(&self) -> bool {
        self.agent_slot.lock().unwrap().is_some()
    }

    /// Send a prompt to the agent (pi-compatible).
    /// Takes the agent from the slot, spawns a task, returns it when done.
    /// If the agent was lost (abort), recreates it from stored params.
    pub fn prompt(&self, message: String) {
        let slot = self.agent_slot.clone();
        let cancel_token = self.cancel_token.clone();
        let handle_arc = self.current_turn_handle.clone();
        let event_tx = self.event_tx.clone();
        let build_params = self.build_params.clone();
        let extensions = self.extensions.clone();

        let handle_arc2 = handle_arc.clone();
        let build_params2 = build_params.clone();
        let handle = tokio::spawn(async move {
            let mut agent = slot.lock().unwrap().take().unwrap_or_else(|| {
                // Agent lost (aborted) — recreate from stored params
                let params = build_params.expect("init_agent must be called before prompt");
                let tools: Vec<Box<dyn AgentTool>> = extensions
                    .as_ref()
                    .map(|exts| {
                        exts.iter()
                            .flat_map(|ext| ext.tools())
                            .map(|twm| Box::new(twm) as Box<dyn AgentTool>)
                            .collect()
                    })
                    .unwrap_or_default();
                Self::_build_agent(
                    &params.model,
                    &params.api_key,
                    &params.system_prompt,
                    params.thinking_level,
                    &params.model_config,
                    tools,
                    Vec::new(),
                )
            });

            // Create a cancellation token and store it so abort() can cancel
            // cooperatively instead of just killing the outer JoinHandle.
            let cancel = tokio_util::sync::CancellationToken::new();
            cancel_token.lock().unwrap().replace(cancel.clone());

            let (yo_tx, mut yo_rx) = mpsc::unbounded_channel();

            let inner_handle = tokio::spawn(async move {
                agent.prompt_with_sender(message, yo_tx).await;
                agent
            });

            // Forward events until AgentEnd or cancellation
            let mut aborted_by_cancel = false;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        // abort() was called — abort inner task and send synthetic AgentEnd
                        inner_handle.abort();
                        if let Some(ref tx) = event_tx {
                            tx.send(AgentEvent::AgentEnd { messages: vec![] }).ok();
                            tx.send(AgentEvent::ProgressMessage {
                                tool_call_id: String::new(),
                                tool_name: String::new(),
                                text: "\n\u{26a0} Agent loop aborted.\n".to_string(),
                            }).ok();
                        }
                        aborted_by_cancel = true;
                        break;
                    }
                    event = yo_rx.recv() => {
                        match event {
                            Some(event) => {
                                let is_end = matches!(&event, AgentEvent::AgentEnd { .. });
                                if let Some(ref tx) = event_tx {
                                    tx.send(event).ok();
                                }
                                if is_end {
                                    break;
                                }
                            }
                            None => {
                                // Channel closed — agent loop finished (or panicked)
                                break;
                            }
                        }
                    }
                }
            }

            // Get agent back (or recreate if inner task was aborted/panicked)
            let agent = if aborted_by_cancel {
                // Agent was lost (inner task aborted). Create fresh from stored params.
                let params = build_params2.expect("build_params must be set");
                let tools: Vec<Box<dyn AgentTool>> = extensions
                    .as_ref()
                    .map(|exts| {
                        exts.iter()
                            .flat_map(|ext| ext.tools())
                            .map(|twm| Box::new(twm) as Box<dyn AgentTool>)
                            .collect()
                    })
                    .unwrap_or_default();
                Self::_build_agent(
                    &params.model,
                    &params.api_key,
                    &params.system_prompt,
                    params.thinking_level,
                    &params.model_config,
                    tools,
                    Vec::new(),
                )
            } else {
                match inner_handle.await {
                    Ok(agent) => agent,
                    Err(_) => {
                        // Send synthetic AgentEnd to unstick the UI
                        if let Some(ref tx) = event_tx {
                            tx.send(AgentEvent::AgentEnd { messages: vec![] }).ok();
                            tx.send(AgentEvent::ProgressMessage {
                                tool_call_id: String::new(),
                                tool_name: String::new(),
                                text: "\n\u{26a0} Agent loop ended unexpectedly — it may have crashed or encountered a network error.\n".to_string(),
                            }).ok();
                        }
                        // Task aborted/panicked — create a fresh agent
                        let params = build_params2.expect("build_params must be set");
                        let tools: Vec<Box<dyn AgentTool>> = extensions
                            .as_ref()
                            .map(|exts| {
                                exts.iter()
                                    .flat_map(|ext| ext.tools())
                                    .map(|twm| Box::new(twm) as Box<dyn AgentTool>)
                                    .collect()
                            })
                            .unwrap_or_default();
                        Self::_build_agent(
                            &params.model,
                            &params.api_key,
                            &params.system_prompt,
                            params.thinking_level,
                            &params.model_config,
                            tools,
                            Vec::new(),
                        )
                    }
                }
            };

            slot.lock().unwrap().replace(agent);
            handle_arc2.lock().unwrap().take();
            // cancel_token is consumed by abort() or the next prompt() call
        });

        *handle_arc.lock().unwrap() = Some(handle);
    }

    /// Push a steering message into the agent's built-in queue.
    /// Call this before `prompt` to drain app-level queues into the agent.
    pub fn steer(&self, msg: AgentMessage) {
        let mut slot = self.agent_slot.lock().unwrap();
        if let Some(ref mut agent) = *slot {
            agent.steer(msg);
        }
    }

    /// Push a follow-up message into the agent's built-in queue.
    pub fn follow_up(&self, msg: AgentMessage) {
        let mut slot = self.agent_slot.lock().unwrap();
        if let Some(ref mut agent) = *slot {
            agent.follow_up(msg);
        }
    }

    /// Abort the current turn. The agent is lost and will be recreated on the next turn.
    pub fn abort(&self) {
        // Cancel the token first (cooperative cancellation). The spawned task
        // checks this inside the forwarding loop and aborts the inner agent
        // loop cleanly, sending a synthetic AgentEnd to unstick the UI.
        let guard = self.cancel_token.lock().unwrap();
        if let Some(token) = guard.as_ref() {
            token.cancel();
        }
        drop(guard);

        // Abort the JoinHandle as a fallback (forceful cancellation).
        // If the token was already consumed (normal completion), this is a no-op.
        if let Some(handle) = self.current_turn_handle.lock().unwrap().take() {
            handle.abort();
        }
    }

    /// Set the model on the idle agent (pi-compatible: setModel).
    pub fn set_model(&mut self, model: &str) {
        if let Some(ref mut params) = self.build_params {
            params.model = model.to_string();
        }
        let mut slot = self.agent_slot.lock().unwrap();
        if let Some(ref mut agent) = *slot {
            agent.model = model.to_string();
        }
    }

    /// Set the thinking level on the idle agent (pi-compatible: setThinkingLevel).
    pub fn set_thinking_level(&mut self, level: ThinkingLevel) {
        let mut slot = self.agent_slot.lock().unwrap();
        if let Some(ref mut agent) = *slot {
            agent.thinking_level = level;
        }
    }

    fn _collect_tools(&self) -> Vec<Box<dyn AgentTool>> {
        self.extensions
            .as_ref()
            .map(|exts| {
                exts.iter()
                    .flat_map(|ext| ext.tools())
                    .map(|twm| Box::new(twm) as Box<dyn AgentTool>)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn _build_agent(
        model: &str,
        api_key: &str,
        system_prompt: &str,
        thinking_level: ThinkingLevel,
        model_config: &ModelConfig,
        tools: Vec<Box<dyn AgentTool>>,
        initial_messages: Vec<AgentMessage>,
    ) -> Agent {
        Agent::new(OpenAiCompatProvider)
            .with_model(model)
            .with_api_key(api_key)
            .with_model_config(model_config.clone())
            .with_system_prompt(system_prompt)
            .with_thinking(thinking_level)
            .with_messages(initial_messages)
            .with_tools(tools)
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
        if !self.compaction_settings.enabled {
            return Ok(false);
        }
        if self.compaction_api_key.is_none() || self.model_name.is_empty() {
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

        let api_key = self.compaction_api_key.as_ref().unwrap();
        let result = compact(&prep, api_key, &self.model_name, None).await?;

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
        if self.compaction_api_key.is_none() || self.model_name.is_empty() {
            return Err("No provider configured for compaction".to_string());
        }

        let entries = self.session.entries();
        let Some(prep) = prepare_compaction(entries, &self.compaction_settings) else {
            return Err("Nothing to compact – session is already compacted or empty".to_string());
        };

        let api_key = self.compaction_api_key.as_ref().unwrap();
        let result = compact(&prep, api_key, &self.model_name, None).await?;

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
        )
        .await
    }

    /// Clean up resources held by the current session (pi-compatible: dispose).
    /// Should be called before switching to a different session or disposing.
    pub fn dispose(&mut self) {
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
