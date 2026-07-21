use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::extension::Extension;
use crate::agent::session::MessageCost;
use crate::agent::session::Session;
use crate::agent::types::{message_text, user_message};
use crate::provider::ProviderRegistry;
use yoagent::context::ContextConfig;
use yoagent::types::AgentMessage;
use yoagent::types::Message;

// ── Compaction types (previously in compaction module) ──────────────

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

/// Result of a compaction operation.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    pub estimated_tokens_after: u64,
    pub details: Option<serde_json::Value>,
}

/// Settings for automatic compaction.
pub struct CompactionSettings {
    pub enabled: bool,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Preparation data for a compaction run.
pub struct CompactionPreparation {
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    /// Messages to summarize (everything before the cut point).
    pub messages_to_summarize: Vec<AgentMessage>,
}

/// Find a cut point in the session entries that keeps approximately
/// `keep_recent_tokens` worth of recent context.
///
/// Walks backwards from the end, accumulating token estimates. Returns the
/// index of the first entry to keep (everything before it is summarized).
fn find_cut_point(
    entries: &[yoagent::session::SessionEntry],
    keep_recent_tokens: u64,
) -> Option<usize> {
    if entries.is_empty() {
        return None;
    }

    let mut accumulated: u64 = 0;
    let budget = keep_recent_tokens.max(5000); // minimum 5K tokens kept

    for i in (0..entries.len()).rev() {
        let entry = &entries[i];
        // Only count LLM messages (skip metadata entries like model_change, etc.)
        if entry.message.as_llm().is_some() {
            let tokens = yoagent::context::message_tokens(&entry.message) as u64;
            accumulated = accumulated.saturating_add(tokens);
        }
        if accumulated >= budget && i > 0 {
            // Found a cut point — walk forward to the next LLM message boundary
            // so we don't split in the middle of a turn.
            for (j, entry) in entries.iter().enumerate().skip(i) {
                if let Some(llm) = entry.message.as_llm()
                    && matches!(llm, Message::User { .. })
                {
                    return Some(j);
                }
            }
            return Some(i);
        }
    }

    // Even the full history is under budget — no compaction needed
    None
}

pub(crate) fn prepare_compaction(
    entries: &[yoagent::session::SessionEntry],
    settings: &CompactionSettings,
) -> Option<CompactionPreparation> {
    if !settings.enabled || entries.is_empty() {
        return None;
    }

    // Skip if the last entry is already a compaction (nothing new to compact)
    if let Some(last) = entries.last()
        && let AgentMessage::Extension(ext) = &last.message
        && ext.kind == crate::agent::session::KIND_COMPACTION
    {
        return None;
    }

    // Find previous compaction boundary, if any
    let prev_compaction_idx = entries.iter().rposition(|e| {
        matches!(&e.message, AgentMessage::Extension(ext) if ext.kind == crate::agent::session::KIND_COMPACTION)
    });

    // Start from after the last compaction
    let start_idx = prev_compaction_idx.map(|i| i + 1).unwrap_or(0);

    // Only consider entries from start_idx onward
    let recent_entries = &entries[start_idx..];
    if recent_entries.len() < 4 {
        // Too few entries to compact
        return None;
    }

    // Token estimate for the entire context
    let messages: Vec<AgentMessage> = entries
        .iter()
        .filter_map(|e| {
            if e.message.as_llm().is_some() {
                Some(e.message.clone())
            } else {
                None
            }
        })
        .collect();
    let tokens_before = yoagent::context::total_tokens(&messages) as u64;

    // Find cut point in the recent entries (after last compaction)
    let cut_idx = find_cut_point(recent_entries, 20_000)?;
    let absolute_cut = start_idx + cut_idx;

    let first_kept_id = entries[absolute_cut].id.clone();

    // Collect messages to summarize (before the cut point, after last compaction)
    let messages_to_summarize: Vec<AgentMessage> = entries[start_idx..absolute_cut]
        .iter()
        .filter_map(|e| {
            if e.message.as_llm().is_some() {
                Some(e.message.clone())
            } else {
                None
            }
        })
        .collect();

    if messages_to_summarize.is_empty() {
        return None;
    }

    Some(CompactionPreparation {
        first_kept_entry_id: first_kept_id,
        tokens_before,
        messages_to_summarize,
    })
}

pub(crate) fn estimate_context_tokens(messages: &[AgentMessage]) -> u64 {
    yoagent::context::total_tokens(messages) as u64
}

/// Check whether the current context exceeds the compaction threshold.
pub(crate) fn should_compact(
    total_tokens: u64,
    context_window: u64,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled {
        return false;
    }
    if context_window == 0 {
        return false;
    }
    // Reserve 16K tokens for the summary prompt and output
    let reserve_tokens: u64 = 16_384;
    total_tokens > context_window.saturating_sub(reserve_tokens)
}

pub fn get_model_context_window(_model: &str) -> u64 {
    // Default: use a conservative 200K context window
    200_000
}

/// Generate a compaction summary using the LLM provider.
///
/// This calls the provider (same model as the agent) to summarize the
/// conversation history up to the cut point. When the provider is unavailable,
/// falls back to yoagent's in-process compaction.
pub(crate) async fn compact(
    prep: &CompactionPreparation,
    api_key: &str,
    model_name: &str,
    custom_instructions: Option<&str>,
    thinking_level: yoagent::types::ThinkingLevel,
    model_config: Option<yoagent::provider::model::ModelConfig>,
) -> Result<CompactionResult, String> {
    // Serialize the messages to summarize into a text representation
    let conversation_text = format_conversation_for_summary(&prep.messages_to_summarize);

    let system_prompt = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

    let base_prompt = r#"The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish?]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [What should happen next]

## Critical Context
- [Data, examples, or references needed to continue]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

    let prompt = if let Some(instructions) = custom_instructions {
        format!(
            "<conversation>\n{}\n</conversation>\n\n{}\n\nAdditional focus: {}",
            conversation_text, base_prompt, instructions
        )
    } else {
        format!(
            "<conversation>\n{}\n</conversation>\n\n{}",
            conversation_text, base_prompt
        )
    };

    // Use yoagent to call the provider for summarization.
    // Fall back to in-process compaction if the provider call fails.
    let result = call_provider_for_summary(
        api_key,
        model_name,
        system_prompt,
        &prompt,
        thinking_level,
        model_config,
    )
    .await;

    match result {
        Ok(summary) => Ok(CompactionResult {
            summary,
            first_kept_entry_id: prep.first_kept_entry_id.clone(),
            tokens_before: prep.tokens_before,
            estimated_tokens_after: 0,
            details: None,
        }),
        Err(e) => {
            // Fallback: use yoagent's in-process compaction
            tracing::warn!(
                "LLM summarization failed ({}), falling back to in-process compaction",
                e
            );
            let config = ContextConfig {
                keep_recent: 10,
                keep_first: 2,
                tool_output_max_lines: 50,
                ..ContextConfig::default()
            };
            let compacted =
                yoagent::context::compact_messages(prep.messages_to_summarize.clone(), &config);
            let summary_text: String = compacted
                .iter()
                .filter_map(|m| {
                    let (content_opt, _) = match m {
                        AgentMessage::Llm(Message::User { content, .. })
                        | AgentMessage::Llm(Message::Assistant { content, .. }) => {
                            (Some(content), false)
                        }
                        _ => (None, false),
                    };
                    content_opt
                        .map(|content| {
                            content
                                .iter()
                                .filter_map(|c| {
                                    if let yoagent::types::Content::Text { text } = c {
                                        Some(text.as_str())
                                    } else {
                                        None
                                    }
                                })
                                .collect::<String>()
                        })
                        .filter(|t| !t.is_empty())
                })
                .collect::<Vec<_>>()
                .join("\n");
            let summary = if summary_text.is_empty() {
                format!(
                    "Conversation compacted ({} messages summarized)",
                    prep.messages_to_summarize.len()
                )
            } else {
                summary_text
            };

            Ok(CompactionResult {
                summary,
                first_kept_entry_id: prep.first_kept_entry_id.clone(),
                tokens_before: prep.tokens_before,
                estimated_tokens_after: 0, // recomputed after append
                details: None,
            })
        }
    }
}

/// Serialize conversation messages to plain text for the summarization prompt.
fn format_conversation_for_summary(messages: &[AgentMessage]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        match msg {
            AgentMessage::Llm(llm) => match llm {
                Message::User { content, .. } => {
                    let text: String = content
                        .iter()
                        .filter_map(|c| {
                            if let yoagent::types::Content::Text { text } = c {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !text.is_empty() {
                        parts.push(format!("[User]: {}", text));
                    }
                }
                Message::Assistant { content, .. } => {
                    let mut thinking_parts = Vec::new();
                    let mut text_parts = Vec::new();
                    let mut tool_calls = Vec::new();
                    for c in content {
                        match c {
                            yoagent::types::Content::Text { text } => {
                                text_parts.push(text.as_str());
                            }
                            yoagent::types::Content::Thinking { thinking, .. } => {
                                thinking_parts.push(thinking.as_str());
                            }
                            yoagent::types::Content::ToolCall {
                                name, arguments, ..
                            } => {
                                tool_calls.push(format!(
                                    "{}({})",
                                    name,
                                    serde_json::to_string(arguments).unwrap_or_default()
                                ));
                            }
                            _ => {}
                        }
                    }
                    if !thinking_parts.is_empty() {
                        parts.push(format!(
                            "[Assistant thinking]: {}",
                            thinking_parts.join("\n")
                        ));
                    }
                    if !text_parts.is_empty() {
                        parts.push(format!("[Assistant]: {}", text_parts.join(" ")));
                    }
                    if !tool_calls.is_empty() {
                        parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                    }
                }
                Message::ToolResult {
                    content, tool_name, ..
                } => {
                    let text: String = content
                        .iter()
                        .filter_map(|c| {
                            if let yoagent::types::Content::Text { text } = c {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !text.is_empty() {
                        let truncated = if text.len() > 2000 {
                            format!(
                                "{}... [{} more characters]",
                                &text[..2000],
                                text.len() - 2000
                            )
                        } else {
                            text.clone()
                        };
                        parts.push(format!("[Tool result ({}):] {}", tool_name, truncated));
                    }
                }
            },
            AgentMessage::Extension(ext) => {
                if let Some(text) = ext.data.get("text").and_then(|v| v.as_str())
                    && !text.is_empty()
                {
                    parts.push(format!("[{}]: {}", ext.kind, text));
                }
            }
        }
    }
    parts.join("\n\n")
}

/// Call the LLM provider to generate a summary.
async fn call_provider_for_summary(
    api_key: &str,
    model_name: &str,
    system_prompt: &str,
    prompt: &str,
    thinking_level: yoagent::types::ThinkingLevel,
    model_config: Option<yoagent::provider::model::ModelConfig>,
) -> Result<String, String> {
    use yoagent::provider::model::ApiProtocol;
    use yoagent::types::*;

    let mc = model_config.clone().unwrap_or_else(|| {
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

    // Build summary messages
    let summary_msg = AgentMessage::Llm(Message::User {
        content: vec![Content::Text {
            text: prompt.to_string(),
        }],
        timestamp: yoagent::types::now_ms(),
    });

    let mut agent = agent
        .with_api_key(api_key)
        .with_system_prompt(system_prompt)
        .with_thinking(thinking_level)
        .with_messages(vec![summary_msg])
        .with_execution_limits(yoagent::context::ExecutionLimits {
            max_total_tokens: 4096,
            max_turns: 1,
            max_duration: std::time::Duration::from_secs(60),
        });

    // Use prompt_structured to get a clean response
    if let Ok(result) = agent
        .prompt_structured::<serde_json::Value>(
            "",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "summary": {"type": "string"}
                }
            }),
        )
        .await
        && let Some(s) = result.get("summary").and_then(|v| v.as_str())
    {
        return Ok(s.to_string());
    }

    // Fallback: get the raw assistant text
    let messages = agent.messages().to_vec();
    for msg in messages.iter().rev() {
        if let AgentMessage::Llm(Message::Assistant { content, .. }) = msg {
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
                return Ok(text);
            }
        }
    }

    Err("No summary generated by provider".to_string())
}

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
            extensions: Vec::new(),
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
        // Provide a default if none was set
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
        // Pi-compatible: persist every message immediately on message_end
        if let yoagent::types::AgentEvent::MessageEnd { message } = event {
            // Pi-compatible: reset overflow recovery when a user message arrives
            // (matches pi's _overflowRecoveryAttempted reset in message_start for user role).
            if crate::agent::types::message_is_user(message) {
                self.reset_overflow_recovery();
            }
            // Pi-compatible: persist every message immediately on message_end.
            // Extension messages use custom_message entries (excluded from LLM context);
            // all others use regular messages.
            if crate::agent::types::message_is_extension(message) {
                self.persist_extension_message(message);
            } else {
                // Compute cost per-message using model's cost config (pi-style).
                let cost = self.compute_message_cost(message);
                self.inner.append_message_with_cost(message.clone(), cost);
            }
        }
    }

    /// Compute the USD cost of a message using the provider registry.
    /// Returns 0.0 if the message isn't an assistant message, the registry is unset,
    /// or the model can't be resolved.
    fn compute_message_cost(&self, message: &AgentMessage) -> MessageCost {
        // Only assistant messages have usage data.
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

        // Resolve the model to get its cost config.
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
        self.emit_compaction_event(&CompactionEvent::Start {
            reason: reason.clone(),
        });

        // Check for cancellation before proceeding
        if cancel.is_cancelled() {
            return Ok(None);
        }

        let entries = self.inner.get_entries();

        // Check threshold for auto-compact
        if reason == CompactionReason::Threshold {
            let context_msgs = self.inner.build_context().messages;
            let context_tokens = estimate_context_tokens(&context_msgs);
            if !should_compact(
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
        self.inner.append_compaction(
            &result.summary,
            &result.first_kept_entry_id,
            result.tokens_before,
            result.details.clone(),
        );

        // Compute estimated tokens after compaction
        let context_after = self.inner.build_context().messages;
        let estimated_tokens_after = estimate_context_tokens(&context_after);

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
        custom_instructions: Option<&str>,
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
            self.session(),
            &entries,
            target_id,
            api_key,
            &self.model_name,
            self.thinking_level,
            self.model_config.clone(),
            custom_instructions,
        )
        .await
    }

    /// Move the leaf pointer to an earlier entry (starts a new branch).
    /// Optionally summarizes the abandoned path if a provider is configured.
    /// `custom_instructions` are passed to the summarization prompt (pi-compatible).
    /// Returns the branch summary text if summarization was performed.
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
            // Summarize the abandoned path
            match self
                .summarize_branch_navigation(Some(old), branch_from_id, custom_instructions)
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

        self.inner
            .set_leaf_id(branch_from_id)
            .map_err(|e| format!("Failed to set branch: {}", e))?;

        Ok(summary)
    }

    /// Persist a tool result message (public so the agent loop can persist crash-safely).
    /// Deduplicates by tool_call_id.
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
        self.inner.append_custom_message_entry(kind, content);
    }

    /// Set the session_dir after construction (for cases where it wasn't known initially).
    pub fn set_session_dir(&mut self, session_dir: PathBuf) {
        self.session_dir = Some(session_dir);
    }
}

// ── Free helper functions (branch summarization) ───────────────────

/// Collect entries between `old_leaf_id` and the common ancestor with `target_id`.
/// Returns (entries_in_branch, common_ancestor_id).
fn collect_entries_for_branch_summary<'a>(
    session: &'a Session,
    old_leaf_id: Option<&str>,
    target_id: &str,
) -> (Vec<&'a yoagent::session::SessionEntry>, String) {
    // Get the path from old_leaf_id (or current leaf) to root
    let leaf = match old_leaf_id {
        Some(id) => id.to_string(),
        None => session.get_leaf_id().unwrap_or_default(),
    };
    if leaf.is_empty() {
        return (vec![], String::new());
    }

    // Walk from leaf to root collecting ids
    let mut leaf_path: Vec<String> = Vec::new();
    let mut cursor: Option<&str> = Some(leaf.as_str());
    while let Some(id) = cursor {
        leaf_path.push(id.to_string());
        cursor = session.get_entry(id).and_then(|e| e.parent_id.as_deref());
    }

    // Walk from target to root collecting ids
    let mut target_path: Vec<String> = Vec::new();
    cursor = Some(target_id);
    while let Some(id) = cursor {
        target_path.push(id.to_string());
        cursor = session.get_entry(id).and_then(|e| e.parent_id.as_deref());
    }

    // Find common ancestor
    let mut common_ancestor = String::new();
    'outer: for leaf_id in &leaf_path {
        for target_id in &target_path {
            if leaf_id == target_id {
                common_ancestor = leaf_id.clone();
                break 'outer;
            }
        }
    }

    // Collect entries from leaf down to (but not including) common ancestor
    let entries: Vec<&yoagent::session::SessionEntry> = leaf_path
        .iter()
        .filter_map(|id| session.get_entry(id))
        .take_while(|e| e.id != common_ancestor)
        .collect();

    (entries, common_ancestor)
}

/// Generate a branch summary using the configured provider.
#[allow(clippy::too_many_arguments)]
async fn generate_branch_summary(
    _session: &Session,
    _entries: &[&yoagent::session::SessionEntry],
    _target_id: &str,
    _api_key: &str,
    _model_name: &str,
    _thinking_level: yoagent::types::ThinkingLevel,
    _model_config: Option<yoagent::provider::model::ModelConfig>,
    _custom_instructions: Option<&str>,
) -> Result<String, String> {
    // Stub: return empty summary
    Ok(String::new())
}
