use serde::Serialize;

use crate::agent::provider::{Provider, StopReason, StreamEvent};
use crate::agent::session::SessionEntry;
use crate::agent::types::{AgentMessage, Role};

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
    ("deepseek", 128_000),
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
    let chars: usize = match message.role {
        Role::User | Role::ToolResult => message.content.len(),
        Role::Assistant => {
            let mut total = message.content.len();
            for tc in &message.tool_calls {
                total += tc.name.len();
                total += serde_json::to_string(&tc.arguments)
                    .unwrap_or_default()
                    .len();
            }
            total
        }
    };
    (chars as u64).div_ceil(4)
}

/// Estimate context tokens for a slice of messages.
/// Uses recorded usage from the last non-aborted assistant message as the baseline,
/// then adds estimated tokens for any messages after it.
pub fn estimate_context_tokens(messages: &[AgentMessage]) -> u64 {
    // Find last assistant message with usage data
    let mut last_usage_index = None;
    for (i, msg) in messages.iter().enumerate().rev() {
        if msg.role == Role::Assistant && msg.usage.is_some() {
            last_usage_index = Some(i);
            break;
        }
    }

    if let Some(idx) = last_usage_index {
        let usage = messages[idx].usage.as_ref().unwrap();
        let usage_tokens = usage.input_tokens.unwrap_or(0) as u64
            + usage.output_tokens.unwrap_or(0) as u64
            + usage.cache_tokens.unwrap_or(0) as u64;

        // Add estimated tokens for messages after the last usage point
        let mut trailing = 0u64;
        for msg in &messages[idx + 1..] {
            trailing += estimate_tokens(msg);
        }
        // Add estimated tokens for messages before (in case they weren't included)
        // This is conservative: use usage total + trailing only
        usage_tokens + trailing
    } else {
        // No usage data — estimate all from scratch
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
            SessionEntry::Message(m) => match m.message.role {
                Role::User | Role::Assistant => points.push(i),
                Role::ToolResult => {} // never cut at tool results
            },
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
            SessionEntry::Message(m) if m.message.role == Role::User => return Some(i),
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
    let is_user_msg = matches!(cut_entry, SessionEntry::Message(m) if m.message.role == Role::User);
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
            SessionEntry::BranchSummary(s) => Some(AgentMessage {
                id: String::new(),
                parent_id: None,
                role: Role::Assistant,
                content: format!("[Branch: from {}] {}", s.from_id, s.summary),
                tool_calls: vec![],
                tool_call_id: None,
                usage: None,
                is_error: false,
                timestamp: 0,
            }),
            SessionEntry::CustomMessage(c) => Some(AgentMessage {
                id: String::new(),
                parent_id: None,
                role: Role::Assistant,
                content: format!(
                    "[{}] {}",
                    c.custom_type,
                    serde_json::to_string(&c.content).unwrap_or_default()
                ),
                tool_calls: vec![],
                tool_call_id: None,
                usage: None,
                is_error: false,
                timestamp: 0,
            }),
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
        for tc in &msg.tool_calls {
            let path = tc
                .arguments
                .get("file_path")
                .or_else(|| tc.arguments.get("path"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            if let Some(p) = path {
                match tc.name.as_str() {
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
    provider: &dyn Provider,
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
    let summary_msg = AgentMessage::user(&prompt);

    // Get summary from provider
    let summary_text = call_provider_for_summary(provider, model, system, &[summary_msg]).await?;

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
async fn call_provider_for_summary(
    provider: &dyn Provider,
    model: &str,
    system_prompt: &str,
    messages: &[AgentMessage],
) -> Result<String, String> {
    let mut stream = provider
        .stream(model, system_prompt, messages, &[])
        .await
        .map_err(|e| format!("Summarization failed: {}", e))?;

    let mut text = String::new();
    let mut last_error: Option<String> = None;

    use futures::StreamExt;
    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::TextDelta { text: delta } => {
                text.push_str(&delta);
            }
            StreamEvent::Done {
                text: final_text,
                stop_reason,
                ..
            } => {
                // If we got no deltas, use the final text
                if text.is_empty() && !final_text.is_empty() {
                    text = final_text;
                }
                if stop_reason == StopReason::Error {
                    last_error = Some("Provider returned error status".to_string());
                }
                break;
            }
            StreamEvent::Error { message } => {
                last_error = Some(message);
                break;
            }
            _ => {} // ignore thinking, tool calls etc.
        }
    }

    if let Some(err) = last_error {
        return Err(format!("Summarization failed: {}", err));
    }

    if text.is_empty() {
        return Err("Summarization returned empty response".to_string());
    }

    Ok(text)
}

/// Format a message for inclusion in the summarisation prompt.
fn format_message_for_summary(msg: &AgentMessage) -> String {
    let role_label = match msg.role {
        Role::User => "User",
        Role::Assistant => "Assistant",
        Role::ToolResult => "Tool Result",
    };
    let mut result = format!("<{}>\n", role_label);
    result.push_str(&msg.content);

    // Include tool calls for assistant messages
    if !msg.tool_calls.is_empty() {
        result.push_str("\n\nTool calls:\n");
        for tc in &msg.tool_calls {
            result.push_str(&format!(
                "  - {}: {}\n",
                tc.name,
                serde_json::to_string(&tc.arguments).unwrap_or_default()
            ));
        }
    }
    result.push_str(&format!("\n</{}>", role_label));
    result
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::{ToolCall, Usage};

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
            timestamp: 0,
        }
    }

    fn make_msg_with_tc(role: Role, content: &str, tool_calls: Vec<ToolCall>) -> AgentMessage {
        AgentMessage {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role,
            content: content.to_string(),
            tool_calls,
            tool_call_id: None,
            usage: None,
            is_error: false,
            timestamp: 0,
        }
    }

    #[test]
    fn test_estimate_tokens_empty() {
        let msg = make_msg(Role::User, "");
        assert_eq!(estimate_tokens(&msg), 0);
    }

    #[test]
    fn test_estimate_tokens_basic() {
        let msg = make_msg(Role::User, "hello world");
        // "hello world" = 11 chars, (11+3)/4 = 3
        assert_eq!(estimate_tokens(&msg), 3);
    }

    #[test]
    fn test_estimate_tokens_assistant_with_tool_calls() {
        let msg = make_msg_with_tc(
            Role::Assistant,
            "Let me read that file",
            vec![ToolCall {
                id: "tc1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"path": "foo.txt"}),
            }],
        );
        let tokens = estimate_tokens(&msg);
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_context_tokens_no_usage() {
        let msgs = vec![
            make_msg(Role::User, "hello"),
            make_msg(Role::Assistant, "world"),
        ];
        let tokens = estimate_context_tokens(&msgs);
        // "hello" = (5+3)/4 = 2, "world" = (5+3)/4 = 2
        assert_eq!(tokens, 4);
    }

    #[test]
    fn test_estimate_context_tokens_with_usage() {
        let mut asst = make_msg(Role::Assistant, "response");
        asst.usage = Some(Usage {
            input_tokens: Some(100),
            output_tokens: Some(50),
            cache_tokens: None,
            cache_write_tokens: None,
            cost_total: None,
        });
        let msgs = vec![
            make_msg(Role::User, "hello"),
            asst,
            make_msg(Role::User, "follow up"),
        ];
        let tokens = estimate_context_tokens(&msgs);
        // 100 + 50 + estimate("follow up") = 150 + (9+3)/4 = 150 + 3 = 153
        assert_eq!(tokens, 153);
    }

    #[test]
    fn test_should_compact_disabled() {
        let settings = CompactionSettings {
            enabled: false,
            ..Default::default()
        };
        assert!(!should_compact(100_000, 128_000, &settings));
    }

    #[test]
    fn test_should_compact_enabled() {
        let settings = CompactionSettings::default();
        // reserve_tokens = 16384, so compact when > 128000 - 16384 = 111616
        assert!(should_compact(120_000, 128_000, &settings));
        assert!(!should_compact(100_000, 128_000, &settings));
    }

    #[test]
    fn test_get_model_context_window_known() {
        let w = get_model_context_window("deepseek-v4-flash");
        assert_eq!(w, 128_000);
    }

    #[test]
    fn test_get_model_context_window_unknown() {
        let w = get_model_context_window("some-unknown-model");
        assert_eq!(w, 200_000);
    }

    #[test]
    fn test_prepare_compaction_last_entry_is_compaction() {
        let entries = vec![
            SessionEntry::Message(crate::agent::session::MessageEntry {
                id: "1".to_string(),
                parent_id: None,
                timestamp: String::new(),
                message: make_msg(Role::User, "hello"),
            }),
            SessionEntry::Compaction(crate::agent::session::CompactionEntry {
                id: "c1".to_string(),
                parent_id: None,
                timestamp: String::new(),
                summary: "Previous summary".to_string(),
                first_kept_entry_id: "1".to_string(),
                tokens_before: 100,
                details: None,
                from_hook: None,
            }),
        ];
        let result = prepare_compaction(&entries, &CompactionSettings::default());
        assert!(result.is_none());
    }

    #[test]
    fn test_prepare_compaction_empty_entries() {
        let result = prepare_compaction(&[], &CompactionSettings::default());
        // Boundary starts at 0, boundary_end = 0, context_msgs is empty, so tokens_before = 0
        assert!(result.is_none());
    }

    #[test]
    fn test_prepare_compaction_basic() {
        let entries: Vec<SessionEntry> = (0..10)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                SessionEntry::Message(crate::agent::session::MessageEntry {
                    id: format!("m{}", i),
                    parent_id: if i > 0 {
                        Some(format!("m{}", i - 1))
                    } else {
                        None
                    },
                    timestamp: String::new(),
                    message: make_msg(role, &format!("message {}", i)),
                })
            })
            .collect();

        // With very small keep_recent_tokens, should compact most
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 1000,
            keep_recent_tokens: 5, // keep very little
        };
        let result = prepare_compaction(&entries, &settings);
        assert!(result.is_some());
        let prep = result.unwrap();
        assert!(!prep.first_kept_entry_id.is_empty());
        assert!(prep.tokens_before > 0);
        assert!(!prep.messages_to_summarize.is_empty());
        assert!(!prep.is_split_turn); // Messages alternate user/assistant, cut at user
    }

    #[test]
    fn test_find_valid_cut_points_skips_tool_results() {
        use crate::agent::session::MessageEntry;
        let entries = vec![
            SessionEntry::Message(MessageEntry {
                id: "u1".to_string(),
                parent_id: None,
                timestamp: String::new(),
                message: make_msg(Role::User, "hello"),
            }),
            SessionEntry::Message(MessageEntry {
                id: "a1".to_string(),
                parent_id: Some("u1".to_string()),
                timestamp: String::new(),
                message: make_msg(Role::Assistant, "do tool"),
            }),
            SessionEntry::Message(MessageEntry {
                id: "t1".to_string(),
                parent_id: Some("a1".to_string()),
                timestamp: String::new(),
                message: make_msg(Role::ToolResult, "result"),
            }),
        ];
        let points = find_valid_cut_points(&entries, 0, entries.len());
        assert_eq!(points, vec![0, 1]); // tool result at index 2 is skipped
    }

    #[test]
    fn test_format_message_for_summary() {
        let msg = make_msg(Role::User, "hello there");
        let formatted = format_message_for_summary(&msg);
        assert!(formatted.contains("<User>"));
        assert!(formatted.contains("hello there"));
        assert!(formatted.contains("</User>"));
    }

    #[test]
    fn test_format_message_with_tool_calls() {
        let msg = make_msg_with_tc(
            Role::Assistant,
            "reading file",
            vec![ToolCall {
                id: "tc1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"path": "foo.txt"}),
            }],
        );
        let formatted = format_message_for_summary(&msg);
        assert!(formatted.contains("Tool calls:"));
        assert!(formatted.contains("read"));
    }

    #[test]
    fn test_extract_file_ops_empty() {
        let result = extract_file_ops(&[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_file_ops_read_only() {
        let mut msg = make_msg(Role::Assistant, "reading");
        msg.tool_calls = vec![
            ToolCall {
                id: "tc1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"file_path": "src/main.rs"}),
            },
            ToolCall {
                id: "tc2".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"path": "Cargo.toml"}),
            },
        ];
        let result = extract_file_ops(&[msg]);
        assert!(result.is_some());
        let json = result.unwrap();
        assert_eq!(json["readFiles"].as_array().unwrap().len(), 2);
        assert_eq!(json["modifiedFiles"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_extract_file_ops_modified() {
        let mut msg = make_msg(Role::Assistant, "editing");
        msg.tool_calls = vec![
            ToolCall {
                id: "tc1".to_string(),
                name: "edit".to_string(),
                arguments: serde_json::json!({"file_path": "src/main.rs"}),
            },
            ToolCall {
                id: "tc2".to_string(),
                name: "write".to_string(),
                arguments: serde_json::json!({"file_path": "src/lib.rs"}),
            },
            ToolCall {
                id: "tc3".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"file_path": "src/main.rs"}), // read+modified → only in modified
            },
        ];
        let result = extract_file_ops(&[msg]);
        assert!(result.is_some());
        let json = result.unwrap();
        let modified = json["modifiedFiles"].as_array().unwrap();
        assert_eq!(modified.len(), 2);
        // read+modified: should not appear in readFiles
        let read = json["readFiles"].as_array().unwrap();
        assert!(!read.iter().any(|v| v == "src/main.rs"));
    }

    #[test]
    fn test_compaction_result_details() {
        let result = CompactionResult {
            summary: "test".to_string(),
            first_kept_entry_id: "e1".to_string(),
            tokens_before: 100,
            details: Some(serde_json::json!({
                "readFiles": ["a.txt"],
                "modifiedFiles": ["b.txt"],
            })),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["details"]["readFiles"][0], "a.txt");
        assert_eq!(json["details"]["modifiedFiles"][0], "b.txt");
    }
}
