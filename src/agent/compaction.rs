use serde::Serialize;

use crate::agent::session::SessionEntry;
use yoagent::types::AgentMessage;

// ── CompactionSettings ─────────────────────────────────────────────

/// Per-session config for compaction behaviour.
#[derive(Debug, Clone)]
pub struct CompactionSettings {
    pub enabled: bool,
    /// Tokens to reserve for system prompt, tool defs, and the response.
    pub reserve_tokens: u64,
    /// Number of most-recent tokens to always keep (never summarised).
    pub keep_recent_tokens: u64,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
        }
    }
}

// ── Result types ───────────────────────────────────────────────────

/// Result of prepare_compaction — what to summarise and what to keep.
#[derive(Debug, Clone)]
pub struct CompactionPreparation {
    /// ID of the first entry to keep (everything before is summarised).
    pub first_kept_entry_id: String,
    /// Messages to summarise (will be replaced by a compaction entry).
    pub messages_to_summarize: Vec<AgentMessage>,
    /// Turn-prefix messages when splitting a single turn.
    pub turn_prefix_messages: Vec<AgentMessage>,
    /// Whether the cut point split a turn in half.
    pub is_split_turn: bool,
    /// Estimated total tokens before compaction.
    pub tokens_before: u64,
    /// Previous compaction summary (for incremental update).
    pub previous_summary: Option<String>,
}

/// Result of compact() — ready to append to the session.
#[derive(Debug, Clone, Serialize)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    /// File operation details (readFiles, modifiedFiles).
    pub details: Option<serde_json::Value>,
}

// ── Default context windows ────────────────────────────────────────

/// Known model context windows (in tokens).
/// Falls back to 200_000 for unknown models.
const MODEL_CONTEXT_WINDOWS: &[(&str, u64)] = &[
    ("deepseek", 1_000_000),
    ("claude", 200_000),
    ("gpt-4", 128_000),
    ("gpt-4o", 128_000),
    ("gemini", 1_048_576),
    ("sonnet", 200_000),
    ("haiku", 200_000),
];

/// Look up the context window for a model name.
pub fn get_model_context_window(model: &str) -> u64 {
    let lower = model.to_lowercase();
    for (prefix, window) in MODEL_CONTEXT_WINDOWS {
        if lower.starts_with(prefix) {
            return *window;
        }
    }
    200_000
}

// ── Token estimation ───────────────────────────────────────────────

/// Estimate token count for a single message (chars/4 heuristic, conservative).
pub fn estimate_tokens(message: &AgentMessage) -> u64 {
    let text = crate::agent::types::message_text(message);
    let mut chars: usize = text.len();

    if let AgentMessage::Llm(yoagent::types::Message::Assistant { content, .. }) = message {
        let tcs = crate::agent::types::content_tool_calls(content);
        for (_, name, args) in &tcs {
            chars += name.len();
            chars += serde_json::to_string(args).unwrap_or_default().len();
        }
    }

    (chars as u64).div_ceil(4)
}

/// Estimate context tokens for a slice of messages.
/// Uses recorded usage from the last non-aborted assistant message as the baseline,
/// then adds estimated tokens for any messages after it.
pub fn estimate_context_tokens(messages: &[AgentMessage]) -> u64 {
    let mut last_usage_index = None;
    for (i, msg) in messages.iter().enumerate().rev() {
        if crate::agent::types::message_usage(msg).is_some() {
            last_usage_index = Some(i);
            break;
        }
    }

    if let Some(idx) = last_usage_index {
        if let Some(usage) = crate::agent::types::message_usage(&messages[idx]) {
            let usage_tokens = usage.input + usage.output + usage.cache_read;
            let mut trailing = 0u64;
            for msg in &messages[idx + 1..] {
                trailing += estimate_tokens(msg);
            }
            usage_tokens + trailing
        } else {
            messages.iter().map(estimate_tokens).sum()
        }
    } else {
        messages.iter().map(estimate_tokens).sum()
    }
}

// ── shouldCompact ──────────────────────────────────────────────────

/// Determine whether compaction should trigger.
pub fn should_compact(
    context_tokens: u64,
    context_window: u64,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled {
        return false;
    }
    context_tokens > context_window.saturating_sub(settings.reserve_tokens)
}

// ── Cut-point detection ────────────────────────────────────────────

/// Find valid cut-point indices: user and assistant messages (never tool results).
fn find_valid_cut_points(entries: &[SessionEntry], start: usize, end: usize) -> Vec<usize> {
    let mut points = Vec::new();
    for (i, entry) in entries.iter().enumerate().take(end).skip(start) {
        match entry {
            SessionEntry::Message(m) => {
                if crate::agent::types::message_is_user(&m.message)
                    || crate::agent::types::message_is_assistant(&m.message)
                {
                    points.push(i);
                }
            }
            SessionEntry::BranchSummary(_)
            | SessionEntry::CustomMessage(_)
            | SessionEntry::ThinkingLevelChange(_)
            | SessionEntry::ModelChange(_)
            | SessionEntry::ActiveToolsChange(_)
            | SessionEntry::Custom(_)
            | SessionEntry::Label(_)
            | SessionEntry::SessionInfo(_)
            | SessionEntry::Compaction(_)
            | SessionEntry::Leaf(_) => {}
        }
    }
    points
}

/// Find the user message that starts the turn containing `entry_index`.
fn find_turn_start_index(
    entries: &[SessionEntry],
    entry_index: usize,
    start: usize,
) -> Option<usize> {
    for i in (start..=entry_index).rev() {
        match &entries[i] {
            SessionEntry::Message(m) if crate::agent::types::message_is_user(&m.message) => {
                return Some(i);
            }
            SessionEntry::BranchSummary(_) | SessionEntry::CustomMessage(_) => return Some(i),
            _ => {}
        }
    }
    None
}

/// Result of finding the cut point.
struct CutPointResult {
    first_kept_entry_index: usize,
    turn_start_index: Option<usize>,
    is_split_turn: bool,
}

/// Walk backwards from the end, accumulating estimated token sizes,
/// and find where to cut.
fn find_cut_point(
    entries: &[SessionEntry],
    start: usize,
    end: usize,
    keep_recent_tokens: u64,
) -> CutPointResult {
    let cut_points = find_valid_cut_points(entries, start, end);

    if cut_points.is_empty() {
        return CutPointResult {
            first_kept_entry_index: start,
            turn_start_index: None,
            is_split_turn: false,
        };
    }

    let mut accumulated = 0u64;
    let mut cut_index = cut_points[0];

    for i in (start..end).rev() {
        let tokens = match &entries[i] {
            SessionEntry::Message(m) => estimate_tokens(&m.message),
            _ => continue,
        };
        accumulated += tokens;

        if accumulated >= keep_recent_tokens {
            // Find the closest valid cut point at or after this entry
            for &cp in &cut_points {
                if cp >= i {
                    cut_index = cp;
                    break;
                }
            }
            break;
        }
    }

    // Walk backward past non-message entries (label, info, etc.)
    while cut_index > start {
        match &entries[cut_index - 1] {
            SessionEntry::Message(_) | SessionEntry::Compaction(_) => break,
            _ => cut_index -= 1,
        }
    }

    let cut_entry = &entries[cut_index];
    let is_user_msg = matches!(cut_entry, SessionEntry::Message(m) if crate::agent::types::message_is_user(&m.message));
    let turn_start = if is_user_msg {
        None
    } else {
        find_turn_start_index(entries, cut_index, start)
    };

    CutPointResult {
        first_kept_entry_index: cut_index,
        turn_start_index: turn_start,
        is_split_turn: !is_user_msg && turn_start.is_some(),
    }
}

// ── prepareCompaction ──────────────────────────────────────────────

/// Analyse the session branch and determine what should be compacted.
///
/// Returns `None` when the last entry is already a compaction (nothing new to do).
pub fn prepare_compaction(
    entries: &[SessionEntry],
    settings: &CompactionSettings,
) -> Option<CompactionPreparation> {
    // Don't compact if no entries
    if entries.is_empty() {
        return None;
    }
    // Don't compact if the last entry is already a compaction
    if let Some(SessionEntry::Compaction(_)) = entries.last() {
        return None;
    }

    // Find previous compaction boundary
    let mut prev_compaction_idx = None;
    for (i, entry) in entries.iter().enumerate().rev() {
        if matches!(entry, SessionEntry::Compaction(_)) {
            prev_compaction_idx = Some(i);
            break;
        }
    }

    let mut previous_summary: Option<String> = None;
    let boundary_start = if let Some(ci) = prev_compaction_idx {
        if let SessionEntry::Compaction(c) = &entries[ci] {
            previous_summary = Some(c.summary.clone());
            // Find where the previous compaction's kept region starts
            let kept_idx = entries.iter().position(|e| e.id() == c.first_kept_entry_id);
            kept_idx.unwrap_or(ci + 1)
        } else {
            0
        }
    } else {
        0
    };

    let boundary_end = entries.len();
    let context_msgs: Vec<AgentMessage> = entries
        .iter()
        .filter_map(|e| match e {
            SessionEntry::Message(m) => Some(m.message.clone()),
            SessionEntry::BranchSummary(s) => Some(crate::agent::types::assistant_message(
                format!("[Branch: from {}] {}", s.from_id, s.summary),
            )),
            SessionEntry::CustomMessage(c) => {
                Some(crate::agent::types::assistant_message(format!(
                    "[{}] {}",
                    c.custom_type,
                    serde_json::to_string(&c.content).unwrap_or_default()
                )))
            }
            _ => None,
        })
        .collect();

    let tokens_before = estimate_context_tokens(&context_msgs);

    let cut = find_cut_point(
        entries,
        boundary_start,
        boundary_end,
        settings.keep_recent_tokens,
    );

    let first_kept = &entries[cut.first_kept_entry_index];
    let first_kept_entry_id = first_kept.id().to_string();

    let history_end = if cut.is_split_turn {
        cut.turn_start_index.unwrap_or(cut.first_kept_entry_index)
    } else {
        cut.first_kept_entry_index
    };

    // Collect messages to summarise
    let messages_to_summarize: Vec<AgentMessage> = entries[boundary_start..history_end]
        .iter()
        .filter_map(|e| match e {
            SessionEntry::Message(m) => Some(m.message.clone()),
            _ => None,
        })
        .collect();

    // Turn prefix messages (when splitting a turn)
    let turn_prefix_messages: Vec<AgentMessage> = if cut.is_split_turn {
        entries[cut.turn_start_index.unwrap_or(0)..cut.first_kept_entry_index]
            .iter()
            .filter_map(|e| match e {
                SessionEntry::Message(m) => Some(m.message.clone()),
                _ => None,
            })
            .collect()
    } else {
        vec![]
    };

    if messages_to_summarize.is_empty() && turn_prefix_messages.is_empty() {
        return None;
    }

    Some(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages,
        is_split_turn: cut.is_split_turn,
        tokens_before,
        previous_summary,
    })
}

// ── Summarization prompts ──────────────────────────────────────────

const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a precise summarizer. Your task is to create a structured summary of a conversation that another LLM will use to continue the work.

Focus on:
- What the user is trying to accomplish
- What has been done so far (completed tasks, changes made)
- What is in progress
- Key decisions and their rationale
- Exact file paths, function names, and error messages
- What should happen next

Be concise but preserve all specific details needed to continue the work."#;

const SUMMARIZATION_PROMPT: &str = r#"The conversation above is to be summarized. Create a structured context checkpoint summary.

Use this format:

## Goal
[What is the user trying to accomplish?]

## Progress
### Done
- [Completed items]

### In Progress
- [Current work]

### Blocked
- [Issues, if any]

## Key Decisions
- [Decisions made]

## Next Steps
1. [Ordered list]

## Critical Context
- [Exact file paths, function names, error messages needed to continue]

Keep each section concise."#;

const UPDATE_SUMMARIZATION_PROMPT: &str = r#"The conversation above contains NEW messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing summary with new information. RULES:
- PRESERVE all existing information
- ADD new progress, decisions, and context
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages

Use the same structured format."#;

const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = r#"This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix:

## Original Request
[What did the user ask for?]

## Early Progress
- [Key decisions and work done]

## Context for Suffix
- [Information needed to understand the kept suffix]"#;

// ── File operation extraction ──────────────────────────────────────

/// Extract file operations from a list of messages (for compaction details).
fn extract_file_ops(messages: &[AgentMessage]) -> Option<serde_json::Value> {
    let mut read_files: Vec<String> = Vec::new();
    let mut modified_files: Vec<String> = Vec::new();

    for msg in messages {
        if let AgentMessage::Llm(yoagent::types::Message::Assistant { content, .. }) = msg {
            let tcs = crate::agent::types::content_tool_calls(content);
            for (_, name, args) in &tcs {
                let path = args
                    .get("file_path")
                    .or_else(|| args.get("path"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                if let Some(p) = path {
                    match name.as_str() {
                        "read" => {
                            if !read_files.contains(&p) {
                                read_files.push(p);
                            }
                        }
                        "write" | "edit" if !modified_files.contains(&p) => {
                            modified_files.push(p);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    if read_files.is_empty() && modified_files.is_empty() {
        return None;
    }

    read_files.sort();
    modified_files.sort();

    // Deduplicate: files that are both read and modified go only in modified
    read_files.retain(|f| !modified_files.contains(f));

    Some(serde_json::json!({
        "readFiles": read_files,
        "modifiedFiles": modified_files,
    }))
}

// ── compact ────────────────────────────────────────────────────────

/// Execute compaction: send messages to the provider for summarisation
/// and return the result ready to append to the session.
pub async fn compact(
    preparation: &CompactionPreparation,
    api_key: &str,
    model: &str,
    system_prompt_override: Option<&str>,
) -> Result<CompactionResult, String> {
    // Serialize messages to summarise into a single text block
    let mut conversation_text = String::new();
    for msg in &preparation.messages_to_summarize {
        conversation_text.push_str(&format_message_for_summary(msg));
        conversation_text.push('\n');
    }

    // Build the summarisation prompt
    let system = system_prompt_override.unwrap_or(SUMMARIZATION_SYSTEM_PROMPT);
    let mut prompt = String::new();
    if !conversation_text.is_empty() {
        prompt.push_str("<conversation>\n");
        prompt.push_str(&conversation_text);
        prompt.push_str("\n</conversation>\n\n");
    }

    // Add previous summary if available (incremental update)
    if let Some(ref prev) = preparation.previous_summary {
        prompt.push_str(&format!(
            "<previous-summary>\n{}\n</previous-summary>\n\n",
            prev
        ));
    }

    if preparation.is_split_turn && !preparation.turn_prefix_messages.is_empty() {
        // Two-part summary: history + turn prefix
        let mut history_text = String::new();
        for msg in &preparation.turn_prefix_messages {
            history_text.push_str(&format_message_for_summary(msg));
            history_text.push('\n');
        }
        let turn_prompt = format!(
            "{}\n\n<turn-prefix>\n{}\n</turn-prefix>\n\n{}",
            prompt, history_text, TURN_PREFIX_SUMMARIZATION_PROMPT
        );
        prompt = turn_prompt;
    } else if preparation.previous_summary.is_some() {
        prompt.push_str(UPDATE_SUMMARIZATION_PROMPT);
    } else {
        prompt.push_str(SUMMARIZATION_PROMPT);
    }

    // Create a summarisation message
    let summary_msg = crate::agent::types::user_message(&prompt);

    // Get summary from provider via yoagent
    let summary_text = summarize_text(api_key, model, system, &[summary_msg]).await?;

    // Extract file operations from messages being summarised
    let mut all_messages = preparation.messages_to_summarize.clone();
    all_messages.extend(preparation.turn_prefix_messages.clone());
    let details = extract_file_ops(&all_messages);

    // Build the result
    Ok(CompactionResult {
        summary: summary_text,
        first_kept_entry_id: preparation.first_kept_entry_id.clone(),
        tokens_before: preparation.tokens_before,
        details,
    })
}

/// Call the provider for a simple text completion (no tools, no streaming).
///
/// Format a message for inclusion in the summarisation prompt.
fn format_message_for_summary(msg: &AgentMessage) -> String {
    let role_label = if crate::agent::types::message_is_user(msg) {
        "User"
    } else if crate::agent::types::message_is_assistant(msg) {
        "Assistant"
    } else {
        "Tool Result"
    };
    let content = crate::agent::types::message_text(msg);
    let mut result = format!("<{}>\n", role_label);
    result.push_str(&content);

    // Include tool calls for assistant messages
    if crate::agent::types::message_tool_call_count(msg) > 0
        && let AgentMessage::Llm(yoagent::types::Message::Assistant { content: c, .. }) = msg
    {
        let tcs = crate::agent::types::content_tool_calls(c);
        if !tcs.is_empty() {
            result.push_str("\n\nTool calls:\n");
            for (_, name, args) in &tcs {
                result.push_str(&format!(
                    "  - {}: {}\n",
                    name,
                    serde_json::to_string(args).unwrap_or_default()
                ));
            }
        }
    }
    result.push_str(&format!("\n</{}>", role_label));
    result
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {}

// ── Summarization helper (shared with branch_summary) ──

/// Call yoagent's provider for a simple text completion (no tools, no streaming).
pub async fn summarize_text(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    messages: &[AgentMessage],
) -> Result<String, String> {
    use yoagent::provider::StreamProvider;
    use yoagent::provider::traits::StreamConfig;

    let yoagent_messages: Vec<yoagent::types::Message> = messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(msg) => Some(msg.clone()),
            AgentMessage::Extension(_) => None,
        })
        .collect();

    let mut model_config = yoagent::provider::model::ModelConfig::openai_compat(
        "https://opencode.ai/zen/go/v1",
        model,
        "opencode-go",
        yoagent::provider::model::OpenAiCompat::deepseek(),
    );
    model_config.context_window = 1_000_000;

    let config = StreamConfig {
        model: model.to_string(),
        system_prompt: system_prompt.to_string(),
        messages: yoagent_messages,
        tools: vec![],
        thinking_level: yoagent::types::ThinkingLevel::Off,
        api_key: api_key.to_string(),
        max_tokens: Some(2048),
        temperature: Some(0.3),
        model_config: Some(model_config),
        cache_config: yoagent::types::CacheConfig::default(),
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = tokio_util::sync::CancellationToken::new();

    tokio::spawn(async move {
        let _ = yoagent::provider::OpenAiCompatProvider
            .stream(config, tx, cancel)
            .await;
    });

    let mut text = String::new();
    let mut last_error: Option<String> = None;

    while let Some(event) = rx.recv().await {
        match event {
            yoagent::provider::traits::StreamEvent::TextDelta { delta, .. } => {
                text.push_str(&delta);
            }
            yoagent::provider::traits::StreamEvent::Done { message } => {
                if let yoagent::types::Message::Assistant { content, .. } = &message {
                    for c in content {
                        if let yoagent::types::Content::Text { text: t } = c
                            && text.is_empty()
                        {
                            text = t.clone();
                        }
                    }
                }
                break;
            }
            yoagent::provider::traits::StreamEvent::Error { .. } => {
                last_error = Some("Provider returned error".to_string());
                break;
            }
            _ => {}
        }
    }

    if let Some(err) = last_error {
        return Err(err);
    }
    Ok(text)
}
