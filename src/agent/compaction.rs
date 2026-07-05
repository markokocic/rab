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

// ── Compaction reason ──────────────────────────────────────────────

/// Why compaction was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionReason {
    /// User manually triggered `/compact`.
    Manual,
    /// Context usage exceeded the configured threshold.
    Threshold,
    /// Provider returned a context overflow error.
    Overflow,
}

impl std::fmt::Display for CompactionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompactionReason::Manual => write!(f, "manual"),
            CompactionReason::Threshold => write!(f, "threshold"),
            CompactionReason::Overflow => write!(f, "overflow"),
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
    /// Estimated context tokens immediately after compaction is applied.
    pub estimated_tokens_after: u64,
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
    use yoagent::types::Content;

    let text = crate::agent::types::message_text(message);
    let mut chars: usize = text.len();

    if let AgentMessage::Llm(yoagent::types::Message::Assistant { content, .. }) = message {
        // Account for thinking blocks and images in assistant messages (pi-compatible).
        // text.len() is already counted via message_text above, so only add extra non-text content.
        for c in content {
            match c {
                Content::Text { .. } => {
                    // Already counted in message_text
                }
                Content::Thinking { thinking, .. } => {
                    chars += thinking.len();
                }
                Content::ToolCall {
                    name, arguments, ..
                } => {
                    chars += name.len();
                    chars += serde_json::to_string(arguments).unwrap_or_default().len();
                }
                Content::Image { .. } => {
                    // Pi estimates 4800 chars per image
                    chars += 4800;
                }
                _ => {}
            }
        }
    } else if let AgentMessage::Llm(yoagent::types::Message::User { content: c, .. }) = message {
        // Account for images in user messages (pi-compatible)
        for c in c {
            if matches!(c, Content::Image { .. }) {
                chars += 4800;
            }
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
        if let Some(usage) = crate::agent::types::message_usage(msg) {
            // Skip usage records that are all zeros (e.g. from test helpers)
            if usage.input > 0 || usage.output > 0 || usage.cache_read > 0 {
                last_usage_index = Some(i);
                break;
            }
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
            // Pi-compatible: branch_summary and custom_message are valid cut points
            SessionEntry::BranchSummary(_) | SessionEntry::CustomMessage(_) => {
                points.push(i);
            }
            SessionEntry::ThinkingLevelChange(_)
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
            // Pi-compatible: branch_summary and custom_message start a turn
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

const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.\n\nUse this EXACT format:\n\n## Goal\n[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned by user]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Current work]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [Ordered list of what should happen next]\n\n## Critical Context\n- [Any data, examples, or references needed to continue]\n- [Or \"(none)\" if not applicable]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.\n\nUpdate the existing structured summary with new information. RULES:\n- PRESERVE all existing information from the previous summary\n- ADD new progress, decisions, and context from the new messages\n- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed\n- UPDATE \"Next Steps\" based on what was accomplished\n- PRESERVE exact file paths, function names, and error messages\n- If something is no longer relevant, you may remove it\n\nUse this EXACT format:\n\n## Goal\n[Preserve existing goals, add new ones if the task expanded]\n\n## Constraints & Preferences\n- [Preserve existing, add new ones discovered]\n\n## Progress\n### Done\n- [x] [Include previously done items AND newly completed items]\n\n### In Progress\n- [ ] [Current work - update based on progress]\n\n### Blocked\n- [Current blockers - remove if resolved]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale] (preserve all previous, add new)\n\n## Next Steps\n1. [Update based on current state]\n\n## Critical Context\n- [Preserve important context, add new if needed]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = r#"This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix:

## Original Request
[What did the user ask for?]

## Early Progress
- [Key decisions and work done]

## Context for Suffix
- [Information needed to understand the kept suffix]"#;

// ── File operation extraction ──────────────────────────────────────

/// File operations accumulator, matching pi's FileOperations / createFileOps.
pub struct FileOps {
    pub read: std::collections::HashSet<String>,
    pub written: std::collections::HashSet<String>,
    pub edited: std::collections::HashSet<String>,
}

impl FileOps {
    pub fn new() -> Self {
        Self {
            read: std::collections::HashSet::new(),
            written: std::collections::HashSet::new(),
            edited: std::collections::HashSet::new(),
        }
    }

    /// Extract file ops from a single assistant message (pi-compatible).
    pub fn extract_from_message(&mut self, msg: &AgentMessage) {
        if let AgentMessage::Llm(yoagent::types::Message::Assistant { content, .. }) = msg {
            let tcs = crate::agent::types::content_tool_calls(content);
            for (_, name, args) in &tcs {
                // Pi only checks `path` field (not `file_path`)
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let Some(p) = path else { continue };
                match name.as_str() {
                    "read" => {
                        self.read.insert(p);
                    }
                    "write" => {
                        self.written.insert(p);
                    }
                    "edit" => {
                        self.edited.insert(p);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Compute sorted read-only and modified file lists (pi-compatible).
    pub fn compute_lists(&self) -> (Vec<String>, Vec<String>) {
        let modified: std::collections::HashSet<String> =
            self.edited.union(&self.written).cloned().collect();
        let mut read_only: Vec<String> = self.read.difference(&modified).cloned().collect();
        read_only.sort();
        let mut modified_sorted: Vec<String> = modified.into_iter().collect();
        modified_sorted.sort();
        (read_only, modified_sorted)
    }

    /// Serialize to JSON for compaction details (pi-compatible).
    pub fn to_json_value(&self) -> Option<serde_json::Value> {
        let (read_files, modified_files) = self.compute_lists();
        if read_files.is_empty() && modified_files.is_empty() {
            return None;
        }
        Some(serde_json::json!({
            "readFiles": read_files,
            "modifiedFiles": modified_files,
        }))
    }
}

impl Default for FileOps {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract file operations from a list of messages (for compaction details).
fn extract_file_ops(messages: &[AgentMessage]) -> Option<serde_json::Value> {
    let mut ops = FileOps::new();
    for msg in messages {
        ops.extract_from_message(msg);
    }
    ops.to_json_value()
}

// ── compact ────────────────────────────────────────────────────────

/// Execute compaction: send messages to the provider for summarisation
/// and return the result ready to append to the session.
///
/// `model_config` should be the session's current model configuration.
/// `thinking_level` controls whether the summarization uses reasoning mode.
pub async fn compact(
    preparation: &CompactionPreparation,
    api_key: &str,
    model: &str,
    system_prompt_override: Option<&str>,
    thinking_level: yoagent::types::ThinkingLevel,
    model_config: Option<yoagent::provider::model::ModelConfig>,
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
    let summary_text = summarize_text(
        api_key,
        model,
        system,
        &[summary_msg],
        thinking_level,
        model_config,
    )
    .await?;

    // Extract file operations from messages being summarised
    let mut all_messages = preparation.messages_to_summarize.clone();
    all_messages.extend(preparation.turn_prefix_messages.clone());
    let details = extract_file_ops(&all_messages);

    // Estimate tokens after compaction:
    //   summary text + kept messages (estimated via heuristic)
    let summary_msg_est = (summary_text.len() as u64).div_ceil(4);
    let kept_tokens = preparation
        .tokens_before
        .saturating_sub(
            preparation
                .messages_to_summarize
                .iter()
                .map(estimate_tokens)
                .sum::<u64>(),
        )
        .saturating_sub(
            preparation
                .turn_prefix_messages
                .iter()
                .map(estimate_tokens)
                .sum::<u64>(),
        );
    let estimated_tokens_after = summary_msg_est + kept_tokens;

    // Build the result
    Ok(CompactionResult {
        summary: summary_text,
        first_kept_entry_id: preparation.first_kept_entry_id.clone(),
        tokens_before: preparation.tokens_before,
        estimated_tokens_after,
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
mod tests {
    use super::*;
    use crate::agent::session::{CompactionEntry, MessageCost, MessageEntry};
    use crate::agent::types::{assistant_message, tool_result_message, user_message};
    use yoagent::types::{AgentMessage, Content, Message};

    // ── get_model_context_window tests ──────────────────────────────

    #[test]
    fn test_context_window_known_model() {
        assert_eq!(get_model_context_window("deepseek-v4-flash"), 1_000_000);
        assert_eq!(get_model_context_window("claude-sonnet-4"), 200_000);
        assert_eq!(get_model_context_window("gpt-4o"), 128_000);
        assert_eq!(get_model_context_window("gemini-2.0-flash"), 1_048_576);
    }

    #[test]
    fn test_context_window_unknown_model_falls_back() {
        assert_eq!(get_model_context_window("unknown-model-42"), 200_000);
    }

    #[test]
    fn test_context_window_case_insensitive() {
        assert_eq!(get_model_context_window("DeepSeek-V4"), 1_000_000);
        assert_eq!(get_model_context_window("CLAUDE-OPUS"), 200_000);
    }

    // ── estimate_tokens tests ───────────────────────────────────────

    #[test]
    fn test_estimate_tokens_empty_message() {
        let msg = user_message("");
        assert_eq!(estimate_tokens(&msg), 0);
    }

    #[test]
    fn test_estimate_tokens_short_message() {
        let msg = user_message("hello");
        // 5 chars / 4 = 2 (div_ceil)
        assert_eq!(estimate_tokens(&msg), 2);
    }

    #[test]
    fn test_estimate_tokens_long_message() {
        let text = "a".repeat(100);
        let msg = user_message(&text);
        // 100 / 4 = 25
        assert_eq!(estimate_tokens(&msg), 25);
    }

    #[test]
    fn test_estimate_tokens_tool_call_includes_arguments() {
        let content = vec![
            Content::Text {
                text: "checking".into(),
            },
            Content::tool_call(
                "call1",
                "read",
                serde_json::json!({"path": "/tmp/file.txt"}),
            ),
        ];
        let msg = AgentMessage::Llm(
            Message::assistant(
                content,
                yoagent::types::StopReason::Stop,
                String::new(),
                String::new(),
                yoagent::types::Usage::default(),
            )
            .with_timestamp(0),
        );
        let tokens = estimate_tokens(&msg);
        // text "checking" (8) + name "read" (4) + args json length >= 17
        assert!(tokens >= 8, "tokens={}", tokens);
    }

    // ── estimate_context_tokens tests ───────────────────────────────

    #[test]
    fn test_estimate_context_tokens_empty() {
        assert_eq!(estimate_context_tokens(&[]), 0);
    }

    #[test]
    fn test_estimate_context_tokens_no_usage_uses_heuristic() {
        let msgs = vec![user_message("hello"), assistant_message("world")];
        let tokens = estimate_context_tokens(&msgs);
        // 5/4 + 5/4 = 2 + 2 = 4
        assert_eq!(tokens, 4);
    }

    #[test]
    fn test_estimate_context_tokens_with_usage_baseline() {
        let msg_with_usage = AgentMessage::Llm(
            Message::assistant(
                vec![Content::Text {
                    text: "response".into(),
                }],
                yoagent::types::StopReason::Stop,
                String::new(),
                String::new(),
                yoagent::types::Usage {
                    input: 100,
                    output: 50,
                    cache_read: 20,
                    cache_write: 0,
                    total_tokens: 0,
                },
            )
            .with_timestamp(0),
        );
        let msgs = vec![
            user_message("hello"),
            msg_with_usage,
            user_message("follow-up"),
        ];
        let tokens = estimate_context_tokens(&msgs);
        // usage: 100 + 50 + 20 = 170 + trailing "follow-up" (9/4=3) = 173
        assert_eq!(tokens, 173);
    }

    // ── should_compact tests ────────────────────────────────────────

    #[test]
    fn test_should_compact_disabled() {
        let settings = CompactionSettings {
            enabled: false,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
        };
        assert!(!should_compact(999_999, 1_000_000, &settings));
    }

    #[test]
    fn test_should_compact_under_threshold() {
        let settings = CompactionSettings::default();
        assert!(!should_compact(100_000, 200_000, &settings));
    }

    #[test]
    fn test_should_compact_at_threshold() {
        let settings = CompactionSettings {
            reserve_tokens: 10_000,
            keep_recent_tokens: 20_000,
            ..Default::default()
        };
        // context_tokens > context_window - reserve = 190_000
        assert!(should_compact(190_001, 200_000, &settings));
        assert!(!should_compact(190_000, 200_000, &settings));
    }

    #[test]
    fn test_should_compact_exact_boundary() {
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        };
        assert!(!should_compact(200_000, 200_000, &settings));
        assert!(should_compact(200_001, 200_000, &settings));
    }

    // ── find_valid_cut_points (via prepare_compaction) ──────────────

    /// Build a minimal session entry list for compaction testing.
    fn make_msg_entry(content: &str) -> SessionEntry {
        SessionEntry::Message(MessageEntry {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            timestamp: String::new(),
            message: user_message(content),
            cost: MessageCost::ZERO,
        })
    }

    fn make_asst_entry(content: &str) -> SessionEntry {
        SessionEntry::Message(MessageEntry {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            timestamp: String::new(),
            message: assistant_message(content),
            cost: MessageCost::ZERO,
        })
    }

    fn make_compaction_entry(first_kept_id: &str) -> SessionEntry {
        SessionEntry::Compaction(CompactionEntry {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            timestamp: String::new(),
            summary: "previous summary".into(),
            first_kept_entry_id: first_kept_id.to_string(),
            tokens_before: 1000,
            details: None,
            from_hook: None,
        })
    }

    #[test]
    fn test_prepare_compaction_empty_entries() {
        let settings = CompactionSettings::default();
        assert!(prepare_compaction(&[], &settings).is_none());
    }

    #[test]
    fn test_prepare_compaction_last_entry_is_compaction() {
        let entries = vec![make_msg_entry("hello"), make_compaction_entry("some-id")];
        let settings = CompactionSettings::default();
        assert!(prepare_compaction(&entries, &settings).is_none());
    }

    #[test]
    fn test_prepare_compaction_returns_preparation() {
        // Create enough entries that keep_recent_tokens forces a cut
        let mut entries: Vec<SessionEntry> = (0..10)
            .map(|i| {
                make_msg_entry(&format!(
                    "message {} with enough text to accumulate tokens",
                    i
                ))
            })
            .collect();
        // Add some assistant messages too
        for i in 0..5 {
            entries.push(make_asst_entry(&format!("response {} with enough text", i)));
        }

        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 100_000,
            keep_recent_tokens: 2, // very small, will cut early
        };
        let result = prepare_compaction(&entries, &settings);
        assert!(result.is_some(), "should return preparation");
        let prep = result.unwrap();
        assert!(!prep.messages_to_summarize.is_empty());
        assert!(!prep.first_kept_entry_id.is_empty());
        assert!(prep.tokens_before > 0);
    }

    #[test]
    fn test_prepare_compaction_with_previous_compaction() {
        let mut entries: Vec<SessionEntry> = vec![make_msg_entry("old message")];

        // First compaction entry
        let first_id = entries[0].id().to_string();
        entries.push(make_compaction_entry(&first_id));

        // New messages after compaction
        entries.push(make_msg_entry("new message"));
        entries.push(make_asst_entry("new response"));

        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 100_000,
            keep_recent_tokens: 1,
        };
        let result = prepare_compaction(&entries, &settings);
        assert!(result.is_some(), "should compact new messages");
        let prep = result.unwrap();
        assert!(prep.previous_summary.is_some());
        assert_eq!(prep.previous_summary.as_deref(), Some("previous summary"));
    }

    // ── extract_file_ops tests ──────────────────────────────────────

    fn make_asst_with_tool_call(name: &str, path: &str) -> AgentMessage {
        AgentMessage::Llm(
            Message::assistant(
                vec![
                    Content::Text {
                        text: "using tool".into(),
                    },
                    Content::tool_call("call-1", name, serde_json::json!({"path": path})),
                ],
                yoagent::types::StopReason::ToolUse,
                String::new(),
                String::new(),
                yoagent::types::Usage::default(),
            )
            .with_timestamp(0),
        )
    }

    #[test]
    fn test_extract_file_ops_empty() {
        assert!(extract_file_ops(&[]).is_none());
    }

    #[test]
    fn test_extract_file_ops_no_tools() {
        let msgs = vec![user_message("hello"), assistant_message("hi")];
        assert!(extract_file_ops(&msgs).is_none());
    }

    #[test]
    fn test_extract_file_ops_read_and_write() {
        let msgs = vec![
            make_asst_with_tool_call("read", "/tmp/a.txt"),
            make_asst_with_tool_call("read", "/tmp/b.txt"),
            make_asst_with_tool_call("write", "/tmp/a.txt"),
        ];
        let result = extract_file_ops(&msgs).unwrap();
        let obj = result.as_object().unwrap();
        let read: Vec<String> = serde_json::from_value(obj["readFiles"].clone()).unwrap();
        let modified: Vec<String> = serde_json::from_value(obj["modifiedFiles"].clone()).unwrap();
        // a.txt is both read and modified -> goes only in modified
        assert_eq!(read, vec!["/tmp/b.txt".to_string()]);
        assert_eq!(modified, vec!["/tmp/a.txt".to_string()]);
    }

    #[test]
    fn test_extract_file_ops_deduplicates() {
        let msgs = vec![
            make_asst_with_tool_call("read", "/tmp/x.txt"),
            make_asst_with_tool_call("read", "/tmp/x.txt"),
        ];
        let result = extract_file_ops(&msgs).unwrap();
        let obj = result.as_object().unwrap();
        let read: Vec<String> = serde_json::from_value(obj["readFiles"].clone()).unwrap();
        assert_eq!(read.len(), 1);
    }

    // ── format_message_for_summary tests ────────────────────────────

    #[test]
    fn test_format_user_message() {
        let msg = user_message("hello world");
        let formatted = format_message_for_summary(&msg);
        assert!(formatted.contains("<User>"));
        assert!(formatted.contains("hello world"));
        assert!(formatted.contains("</User>"));
    }

    #[test]
    fn test_format_assistant_message_with_tool_calls() {
        let msg = make_asst_with_tool_call("edit", "/tmp/f.py");
        let formatted = format_message_for_summary(&msg);
        assert!(formatted.contains("<Assistant>"));
        assert!(formatted.contains("using tool"));
        assert!(formatted.contains("Tool calls"));
        assert!(formatted.contains("edit"));
    }

    #[test]
    fn test_format_tool_result_message() {
        let msg = tool_result_message("call-1", "bash", "command output", false);
        let formatted = format_message_for_summary(&msg);
        assert!(formatted.contains("Tool Result"));
        assert!(formatted.contains("command output"));
    }
}

// ── Summarization helper (shared with branch_summary) ──

/// Call yoagent's provider for a simple text completion (no tools, no streaming).
///
/// Uses the provided `model_config` (base URL, compat flags, etc.) and `thinking_level`
/// instead of hardcoded values. When `model_config` is None, falls back to the default
/// OpenCode Go endpoint for backward compatibility.
pub async fn summarize_text(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    messages: &[AgentMessage],
    thinking_level: yoagent::types::ThinkingLevel,
    model_config: Option<yoagent::provider::model::ModelConfig>,
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

    // Use provided model config, or fall back to hardcoded OpenCode Go for backward compat
    let model_config = model_config.unwrap_or_else(|| crate::agent::base_model_config(model));

    let retry_config = yoagent::RetryConfig::default();

    for attempt in 0..=retry_config.max_retries {
        let config = StreamConfig {
            model: model.to_string(),
            system_prompt: system_prompt.to_string(),
            messages: yoagent_messages.clone(),
            tools: vec![],
            thinking_level,
            api_key: api_key.to_string(),
            max_tokens: Some(2048),
            temperature: Some(0.3),
            model_config: Some(model_config.clone()),
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
            if attempt < retry_config.max_retries {
                let delay = retry_config.delay_for_attempt(attempt + 1);
                tokio::time::sleep(delay).await;
                continue;
            }
            return Err(err);
        }
        return Ok(text);
    }

    unreachable!()
}
