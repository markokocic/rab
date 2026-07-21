use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::compaction as compaction_mod;
pub use crate::agent::compaction::CompactionResult;
pub use crate::agent::compaction::CompactionSettings;
use crate::agent::session::MessageCost;
use crate::agent::session::Session;
use crate::agent::types::{message_text, user_message};
use crate::provider::ProviderRegistry;
use yoagent::types::AgentMessage;
use yoagent::types::Message;

// ── Compaction types ───────────────────────────────────────────────

/// Reason for compaction.
#[derive(Debug, Clone, PartialEq)]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

impl std::fmt::Display for CompactionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

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
    /// The underlying yoagent-based session.
    inner: Session,
    /// Session directory for persistence (if available).
    session_dir: Option<PathBuf>,
    /// Last known model for change detection.
    last_model: Option<(String, String)>,
    /// Last known thinking level for change detection.
    last_thinking_level: String,
    /// Last known active tool names for change detection.
    last_active_tools: Option<Vec<String>>,
    /// Compaction settings (enabled, reserve_tokens, keep_recent_tokens).
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
    /// Create a new AgentSession wrapping a Session.
    pub fn new(inner: Session, session_dir: Option<PathBuf>) -> Self {
        // Snapshot current metadata from the session context for change detection.
        let ctx = inner.build_context();

        // If the session has no thinking level change entries, set last_thinking_level
        // to empty so the first on_thinking_level_change always detects a change.
        let has_thinking_entries = !inner.find_entries("thinking_level_change").is_empty();
        let last_thinking_level = if has_thinking_entries {
            ctx.thinking_level
        } else {
            String::new()
        };

        Self {
            inner,
            session_dir,
            last_model: ctx.model,
            last_thinking_level,
            last_active_tools: ctx.active_tool_names,
            compaction_settings: CompactionSettings::default(),
            context_window: 200_000,
            model_name: String::new(),
            compaction_api_key: None,
            model_config: None,
            thinking_level: yoagent::types::ThinkingLevel::Off,
            event_listeners: Vec::new(),
            overflow_recovery_attempted: false,
            compaction_cancel: crate::agent::extension::Cancel::new(),
            registry: None,
        }
    }

    // ── Static factory methods ─────────────────────────────────

    /// Create a new persisted session.
    pub fn create(cwd: &Path, session_dir: Option<&Path>) -> Self {
        let sd = session_dir.map(|p| p.to_path_buf());
        let inner = match sd.as_ref() {
            Some(dir) => Session::create(cwd, dir).unwrap_or_else(|e| {
                eprintln!("Warning: failed to create session file: {}", e);
                Session::new(cwd)
            }),
            None => Session::new(cwd),
        };
        Self::new(inner, sd)
    }

    /// Open a specific session file.
    pub fn open(path: &Path, session_dir: Option<&Path>, cwd_override: Option<&Path>) -> Self {
        let sd = session_dir.map(|p| p.to_path_buf());
        let inner = Session::open(path, cwd_override);
        Self::new(inner, sd)
    }

    /// Create an in-memory session (no persistence).
    pub fn in_memory(cwd: &Path) -> Self {
        Self::new(Session::in_memory(cwd), None)
    }

    /// Continue most recent session or create new.
    pub fn continue_recent(cwd: &Path, session_dir: Option<&Path>) -> Self {
        let sd = session_dir.map(|p| p.to_path_buf());
        let inner = match sd.as_ref() {
            Some(dir) => Session::continue_recent(cwd, dir).unwrap_or_else(|e| {
                eprintln!("Warning: failed to continue recent session: {}", e);
                Session::new(cwd)
            }),
            None => Session::new(cwd),
        };
        Self::new(inner, sd)
    }

    /// Fork a session from another project directory.
    pub fn fork_from(
        source_path: &Path,
        target_cwd: &Path,
        session_dir: Option<&Path>,
    ) -> std::io::Result<Self> {
        let sd = session_dir.map(|p| p.to_path_buf());
        let dir = sd.as_deref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "session_dir is required for fork_from",
            )
        })?;
        let inner = Session::fork_from(source_path, target_cwd, dir)?;
        Ok(Self::new(inner, sd))
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
        // Prefer model_config.context_window when available (more accurate)
        self.context_window = model_config
            .as_ref()
            .map_or(context_window, |mc| mc.context_window as u64);
        self.model_config = model_config;
    }

    /// Apply compaction settings from the user's settings config.
    pub fn apply_compaction_config(&mut self, config: &crate::agent::settings::CompactionConfig) {
        if let Some(enabled) = config.enabled {
            self.compaction_settings.enabled = enabled;
        }
        if let Some(reserve) = config.reserve_tokens {
            self.compaction_settings.reserve_tokens = reserve;
        }
        if let Some(keep) = config.keep_recent_tokens {
            self.compaction_settings.keep_recent_tokens = keep;
        }
    }

    /// Enable or disable auto-compaction.
    pub fn set_auto_compact(&mut self, enabled: bool) {
        self.compaction_settings.enabled = enabled;
    }

    /// Set the provider registry for per-message cost computation (pi-style).
    pub fn set_registry(&mut self, registry: Arc<ProviderRegistry>) {
        self.registry = Some(registry);
    }

    /// Sync the thinking level from the session context.
    /// Should be called after the session context changes.
    pub fn sync_thinking_level(&mut self) {
        let ctx = self.inner.build_context();
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

    /// Abort any in-progress compaction (matching pi's `abortCompaction()`).
    /// The cancellation will be picked up on the next `cancel.is_cancelled()` check.
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
    /// Pi-compatible: reset overflow recovery when a user message arrives
    /// (matches pi's _overflowRecoveryAttempted reset in message_start for user role).
    pub fn reset_overflow_recovery(&mut self) {
        self.overflow_recovery_attempted = false;
        self.compaction_cancel = crate::agent::extension::Cancel::new();
    }

    /// Check if a provider error indicates context overflow.
    /// Matches pi's context overflow detection patterns.
    pub fn is_context_overflow_error(msg: &AgentMessage) -> bool {
        let text = message_text(msg);
        let lower = text.to_lowercase();
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

    /// Borrow the underlying Session.
    pub fn session(&self) -> &Session {
        &self.inner
    }

    /// Mutably borrow the underlying Session.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.inner
    }

    /// Consume and return the inner Session.
    pub fn into_session(self) -> Session {
        self.inner
    }

    /// Flush is handled automatically by `Session` on every `append_message`.
    /// Call this to force an early flush (e.g. before saving state externally).
    pub fn ensure_flushed(&mut self) {
        self.inner.ensure_flushed(self.session_dir.as_deref());
    }

    // ── App-level accessors ────────────────────────────────────

    pub fn cwd(&self) -> &Path {
        Path::new(self.inner.cwd())
    }

    pub fn session_dir(&self) -> &Path {
        self.session_dir.as_deref().unwrap_or_else(|| Path::new(""))
    }

    pub fn is_persisted(&self) -> bool {
        self.inner.is_persisted()
    }

    pub fn session_id(&self) -> String {
        self.inner.session_id().to_string()
    }

    pub fn session_file(&self) -> Option<PathBuf> {
        self.inner.session_file().map(|p| p.to_path_buf())
    }

    pub fn session_name(&self) -> Option<String> {
        self.inner.session_name().map(|s| s.to_string())
    }

    // ── Model / thinking / tool change tracking ─────────────────

    /// Persist a model change if it differs from the last known model.
    /// Pi-compatible: writes immediately to the session.
    pub fn on_model_change(&mut self, provider: &str, model_id: &str) -> bool {
        let new = (provider.to_string(), model_id.to_string());
        if self.last_model.as_ref() != Some(&new) {
            self.inner.append_model_change(provider, model_id);
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
            self.inner.append_thinking_level_change(level);
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
            self.inner.append_active_tools_change(&tools_vec);
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
        self.inner = Session::new(Path::new(self.inner.cwd()));
        self.last_model = None;
        self.last_thinking_level = String::new();
        self.last_active_tools = None;
        self.compaction_cancel = crate::agent::extension::Cancel::new();
    }

    /// Append a user message to the session (pi-compatible: persists immediately).
    /// Returns the entry id.
    pub fn send_user_message(&mut self, content: &str) -> String {
        let msg = user_message(content);
        self.inner.append_message(msg)
    }

    /// Append a user message (pre-constructed) to the session.
    /// Returns the entry id.
    pub fn send_user_message_obj(&mut self, msg: &AgentMessage) -> String {
        self.inner.append_message(msg.clone())
    }

    // ── Event-driven persistence ──────────────────────────────────

    /// Process an agent event for automatic persistence (pi-compatible).
    ///
    /// Pi persists every message (user, assistant, tool result, custom) immediately
    /// on `message_end`, not deferred to `agent_end`. Extension messages use
    /// `custom_message` entries (excluded from LLM context); all others use regular
    /// `message` entries.
    ///
    /// Cost is computed per-message at creation time using the model's cost config
    /// from the provider registry (pi-style: `calculateCost` in models.ts).
    ///
    /// Call this from your agent event handler.
    pub fn on_agent_event(&mut self, event: &yoagent::types::AgentEvent) {
        if let yoagent::types::AgentEvent::MessageEnd { message } = event {
            if crate::agent::types::message_is_user(message) {
                self.reset_overflow_recovery();
            }
            // Pi-compatible: reset overflow recovery on successful assistant response
            if let AgentMessage::Llm(Message::Assistant { stop_reason, .. }) = message
                && *stop_reason != yoagent::types::StopReason::Error
                && *stop_reason != yoagent::types::StopReason::Aborted
            {
                self.overflow_recovery_attempted = false;
            }
            if crate::agent::types::message_is_extension(message) {
                self.persist_extension_message(message);
            } else {
                let cost = self.compute_message_cost(message);
                self.inner.append_message_with_cost(message.clone(), cost);
            }
        }
    }

    /// Compute the USD cost of a message using the provider registry.
    fn compute_message_cost(&self, message: &AgentMessage) -> MessageCost {
        let (provider, model_id, usage) = match message {
            AgentMessage::Llm(Message::Assistant {
                provider,
                model,
                usage,
                ..
            }) => (provider.as_str(), model.as_str(), usage),
            _ => return MessageCost::ZERO,
        };

        let Some(ref registry) = self.registry else {
            return MessageCost::ZERO;
        };

        let Ok(resolved) = registry.resolve(model_id, Some(provider)) else {
            return MessageCost::ZERO;
        };

        let cost_config = &resolved.model_config.cost;
        let (input, output, cache_read, cache_write, _total) =
            crate::provider::calculate_cost(cost_config, usage);
        MessageCost::new(input, output, cache_read, cache_write)
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

    /// Call the LLM provider to generate a summary.
    /// Returns (summary_text, usage).
    async fn call_provider_for_summary(
        &self,
        system_prompt: &str,
        prompt: &str,
    ) -> Result<(String, yoagent::types::Usage), String> {
        use yoagent::provider::model::ApiProtocol;
        use yoagent::types::*;

        let api_key = self.compaction_api_key.as_ref().ok_or("No API key")?;
        let model_name = &self.model_name;

        let mc = self.model_config.clone().unwrap_or_else(|| {
            let mut mc = crate::agent::base_model_config(model_name);
            mc.context_window = 200_000;
            mc
        });

        let agent = match mc.api {
            ApiProtocol::OpenAiCompletions => yoagent::agent::Agent::from_provider(
                crate::provider::openai_compat::RabOpenAiCompatProvider,
                mc.clone(),
            ),
            ApiProtocol::AnthropicMessages => yoagent::agent::Agent::from_provider(
                crate::provider::anthropic::RabAnthropicProvider,
                mc.clone(),
            ),
            _ => yoagent::agent::Agent::from_config(mc.clone()),
        };

        let summary_msg = AgentMessage::Llm(Message::User {
            content: vec![Content::Text {
                text: prompt.to_string(),
            }],
            timestamp: yoagent::types::now_ms(),
        });

        let agent = agent
            .with_api_key(api_key)
            .with_system_prompt(system_prompt)
            .with_thinking(self.thinking_level)
            .with_messages(vec![summary_msg])
            .with_execution_limits(yoagent::context::ExecutionLimits {
                max_total_tokens: 4096,
                max_turns: 1,
                max_duration: std::time::Duration::from_secs(60),
            });

        let messages = agent.messages().to_vec();
        for msg in messages.iter().rev() {
            if let AgentMessage::Llm(Message::Assistant { content, usage, .. }) = msg {
                let text: String = content
                    .iter()
                    .filter_map(|c| {
                        if let Content::Text { text } = c {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                if !text.is_empty() {
                    return Ok((text, usage.clone()));
                }
            }
        }

        Err("No summary generated by provider".to_string())
    }

    /// Internal: run compaction with the given reason, emitting lifecycle events.
    /// Returns the CompactionResult if compaction was performed, or None if skipped.
    async fn _run_compaction(
        &mut self,
        reason: CompactionReason,
        custom_instructions: Option<&str>,
        will_retry: bool,
    ) -> Result<Option<CompactionResult>, String> {
        if reason == CompactionReason::Threshold && !self.compaction_settings.enabled {
            return Ok(None);
        }

        if self.compaction_api_key.is_none() || self.model_name.is_empty() {
            return Ok(None);
        }

        self.compaction_cancel = crate::agent::extension::Cancel::new();
        let cancel = self.compaction_cancel.clone();

        self.emit_compaction_event(&CompactionEvent::Start {
            reason: reason.clone(),
        });

        if cancel.is_cancelled() {
            return Ok(None);
        }

        let entries = self.inner.get_entries();

        // Check threshold
        if reason == CompactionReason::Threshold {
            let context_msgs = self.inner.build_context().messages;
            let context_tokens: u64 = context_msgs
                .iter()
                .map(|m| yoagent::context::message_tokens(m) as u64)
                .sum();
            if !compaction_mod::should_compact(
                context_tokens,
                self.context_window,
                &self.compaction_settings,
            ) {
                return Ok(None);
            }
        }

        let Some(prep) = compaction_mod::prepare_compaction(entries, &self.compaction_settings)
        else {
            return Ok(None);
        };

        let result = {
            // Build prompts using compaction module, call provider directly
            let (summary_text, _details) = if prep.is_split_turn
                && !prep.turn_prefix_messages.is_empty()
            {
                // History summary
                let history_prompt = compaction_mod::build_summarization_prompt(
                    &prep.messages_to_summarize,
                    prep.previous_summary.as_deref(),
                    custom_instructions,
                );
                let (history_text, _history_usage) = self
                    .call_provider_for_summary(
                        compaction_mod::SUMMARIZATION_SYSTEM_PROMPT,
                        &history_prompt,
                    )
                    .await?;

                // Turn prefix summary
                let prefix_conversation =
                    compaction_mod::serialize_conversation(&prep.turn_prefix_messages);
                let prefix_prompt = format!(
                    "<conversation>\n{}\n</conversation>\n\n{}",
                    prefix_conversation,
                    "This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.\n\n\
                     Summarize the prefix to provide context for the retained suffix:\n\n\
                     ## Original Request\n\
                     [What did the user ask for in this turn?]\n\n\
                     ## Early Progress\n\
                     - [Key decisions and work done in the prefix]\n\n\
                     ## Context for Suffix\n\
                     - [Information needed to understand the retained recent work]\n\n\
                     Be concise. Focus on what's needed to understand the kept suffix."
                );
                let (prefix_text, _prefix_usage) = self
                    .call_provider_for_summary(
                        compaction_mod::SUMMARIZATION_SYSTEM_PROMPT,
                        &prefix_prompt,
                    )
                    .await?;

                let combined = format!(
                    "{}\n\n---\n\n**Turn Context (split turn):**\n\n{}",
                    history_text, prefix_text
                );
                (combined, None::<serde_json::Value>)
            } else {
                let prompt = compaction_mod::build_summarization_prompt(
                    &prep.messages_to_summarize,
                    prep.previous_summary.as_deref(),
                    custom_instructions,
                );
                let (text, _usage) = self
                    .call_provider_for_summary(compaction_mod::SUMMARIZATION_SYSTEM_PROMPT, &prompt)
                    .await?;
                (text, None::<serde_json::Value>)
            };

            // Append file operations to summary
            let (read_files, modified_files) = compaction_mod::compute_file_lists(&prep.file_ops);
            let summary_with_files = format!(
                "{}{}",
                summary_text,
                compaction_mod::format_file_operations(&read_files, &modified_files)
            );

            let details = serde_json::to_value(compaction_mod::CompactionDetails {
                read_files,
                modified_files,
            })
            .ok();

            CompactionResult {
                summary: summary_with_files,
                first_kept_entry_id: prep.first_kept_entry_id.clone(),
                tokens_before: prep.tokens_before,
                estimated_tokens_after: None,
                details,
            }
        };

        // Append the compaction entry to the session
        self.inner.append_compaction(
            &result.summary,
            &result.first_kept_entry_id,
            result.tokens_before,
            result.details.clone(),
        );

        // Compute estimated tokens after compaction
        let context_after = self.inner.build_context().messages;
        let estimated_tokens_after: u64 = context_after
            .iter()
            .map(|m| yoagent::context::message_tokens(m) as u64)
            .sum();

        let final_result = CompactionResult {
            estimated_tokens_after: Some(estimated_tokens_after),
            ..result
        };

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
    pub async fn summarize_branch_navigation(
        &mut self,
        old_leaf_id: Option<&str>,
        target_id: &str,
        _custom_instructions: Option<&str>,
    ) -> Result<String, String> {
        if self.compaction_api_key.is_none() || self.model_name.is_empty() {
            return Err("No provider configured for summarization".to_string());
        }

        let lookup =
            |id: &str| -> Option<&yoagent::session::SessionEntry> { self.inner.get_entry(id) };

        let (entries, _common_ancestor) =
            compaction_mod::collect_entries_for_branch_summary(&lookup, old_leaf_id, target_id);

        if entries.is_empty() {
            return Err("No abandoned entries to summarize".to_string());
        }

        let Some((prompt, read_files, modified_files)) =
            compaction_mod::build_branch_summary_text(&entries)
        else {
            return Err("No content to summarize".to_string());
        };

        let (summary_text, _usage) = self
            .call_provider_for_summary(compaction_mod::SUMMARIZATION_SYSTEM_PROMPT, &prompt)
            .await?;

        const BRANCH_SUMMARY_PREAMBLE: &str = "The user explored a different conversation branch before returning here.\n\
             Summary of that exploration:\n\n";

        let mut full_summary = format!("{}{}", BRANCH_SUMMARY_PREAMBLE, summary_text);
        full_summary += &compaction_mod::format_file_operations(&read_files, &modified_files);

        Ok(full_summary)
    }

    /// Move the leaf pointer to an earlier entry (starts a new branch).
    pub async fn set_branch(
        &mut self,
        branch_from_id: &str,
        custom_instructions: Option<&str>,
    ) -> Result<Option<String>, String> {
        let old_leaf = self.inner.get_leaf_id();

        let summary = if self.compaction_api_key.is_some()
            && !self.model_name.is_empty()
            && let Some(ref old) = old_leaf
            && old != branch_from_id
        {
            match self
                .summarize_branch_navigation(Some(old), branch_from_id, custom_instructions)
                .await
            {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("Warning: branch summarization failed: {}", e);
                    None
                }
            }
        } else {
            None
        };

        self.inner
            .set_leaf_id(branch_from_id)
            .map_err(|e| format!("Failed to set branch: {}", e))?;

        Ok(summary)
    }

    /// Persist a tool result message (public so the agent loop can persist crash-safely).
    pub fn persist_extension_message(&mut self, msg: &AgentMessage) {
        let Some(kind) = crate::agent::types::message_extension_kind(msg) else {
            return;
        };
        let text = crate::agent::types::message_extension_text(msg)
            .unwrap_or_else(|| crate::agent::types::message_text(msg));
        let content = serde_json::json!({"text": text});
        self.inner.append_custom_message_entry(kind, content);
    }

    /// Set the session_dir after construction (for cases where it wasn't known initially).
    pub fn set_session_dir(&mut self, session_dir: PathBuf) {
        self.session_dir = Some(session_dir);
    }
}
