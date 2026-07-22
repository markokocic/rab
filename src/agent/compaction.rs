//! Context compaction for long sessions (pi-compatible).
//!
//! Pure functions for compaction logic. The session manager handles I/O,
//! and after compaction the session is reloaded.
//!
//! Modeled after pi's packages/coding-agent/src/core/compaction/
//! Uses yoagent::context for token estimation and in-process compaction.

use crate::agent::session::KIND_COMPACTION;
use crate::agent::types::content_text;
use yoagent::context::message_tokens;
use yoagent::session::SessionEntry;
use yoagent::types::{AgentMessage, Content, Message};

// ═══════════════════════════════════════════════════════════════════════
// File Operation Tracking
// ═══════════════════════════════════════════════════════════════════════

/// Tracked file operations extracted from tool calls.
#[derive(Debug, Clone, Default)]
pub struct FileOperations {
    pub read: Vec<String>,
    pub written: Vec<String>,
    pub edited: Vec<String>,
}

pub fn create_file_ops() -> FileOperations {
    FileOperations::default()
}

/// Extract file operations from tool calls in an assistant message.
pub fn extract_file_ops_from_message(msg: &AgentMessage, file_ops: &mut FileOperations) {
    let content = match msg {
        AgentMessage::Llm(Message::Assistant { content, .. }) => content,
        _ => return,
    };
    for block in content {
        if let Content::ToolCall {
            name, arguments, ..
        } = block
        {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let Some(path) = path else { continue };
            match name.as_str() {
                "read" => file_ops.read.push(path),
                "write" => file_ops.written.push(path),
                "edit" => file_ops.edited.push(path),
                _ => {}
            }
        }
    }
}

/// Compute final file lists: read-only vs modified files.
pub fn compute_file_lists(file_ops: &FileOperations) -> (Vec<String>, Vec<String>) {
    let modified: std::collections::BTreeSet<String> = file_ops
        .edited
        .iter()
        .chain(file_ops.written.iter())
        .cloned()
        .collect();
    let read_only: Vec<String> = {
        let mut ro: Vec<String> = file_ops
            .read
            .iter()
            .filter(|f| !modified.contains(*f))
            .cloned()
            .collect();
        ro.sort();
        ro.dedup();
        ro
    };
    let mut modified_files: Vec<String> = modified.into_iter().collect();
    modified_files.sort();
    (read_only, modified_files)
}

/// Format file operations as XML tags for summary.
pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
        ));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}

/// Details stored in compaction entry for file tracking.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompactionDetails {
    #[serde(default)]
    pub read_files: Vec<String>,
    #[serde(default)]
    pub modified_files: Vec<String>,
}

/// Extract file operations from messages and previous compaction entries.
fn collect_file_operations(
    messages: &[AgentMessage],
    entries: &[SessionEntry],
    prev_compaction_index: isize,
) -> FileOperations {
    let mut file_ops = create_file_ops();

    // Collect from previous compaction's details
    if prev_compaction_index >= 0 {
        let idx = prev_compaction_index as usize;
        if let AgentMessage::Extension(ext) = &entries[idx].message
            && ext.kind == KIND_COMPACTION
            && let Some(details) = ext.data.get("details")
            && let Ok(d) = serde_json::from_value::<CompactionDetails>(details.clone())
        {
            for f in d.read_files {
                file_ops.read.push(f);
            }
            for f in d.modified_files {
                file_ops.edited.push(f);
            }
        }
    }

    for msg in messages {
        extract_file_ops_from_message(msg, &mut file_ops);
    }

    file_ops
}

// ═══════════════════════════════════════════════════════════════════════
// Compaction Settings
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,
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

/// Result of a compaction operation (pi-compatible).
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    pub estimated_tokens_after: Option<u64>,
    pub details: Option<serde_json::Value>,
}

/// Check if compaction should trigger based on context usage.
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

// ═══════════════════════════════════════════════════════════════════════
// Entry helpers (yoagent-based token estimation)
// ═══════════════════════════════════════════════════════════════════════

/// Get the context-visible message from an entry.
fn entry_context_message(entry: &SessionEntry) -> Option<AgentMessage> {
    match &entry.message {
        AgentMessage::Llm(m) => match m {
            Message::Assistant {
                error_message: Some(_),
                ..
            } => None,
            _ => Some(entry.message.clone()),
        },
        AgentMessage::Extension(ext) => {
            if ext.kind == KIND_COMPACTION {
                let summary = ext.data["summary"].as_str().unwrap_or("");
                if summary.is_empty() {
                    return None;
                }
                Some(AgentMessage::Llm(Message::User {
                    content: vec![Content::Text {
                        text: format!(
                            "The conversation history before this point was compacted into the following summary:\n\n<summary>\n{}\n</summary>",
                            summary
                        ),
                    }],
                    timestamp: yoagent::types::now_ms(),
                }))
            } else if ext.kind == crate::agent::session::KIND_BRANCH_SUMMARY {
                let summary = ext.data["summary"].as_str().unwrap_or("");
                if summary.is_empty() {
                    return None;
                }
                Some(AgentMessage::Llm(Message::User {
                    content: vec![Content::Text {
                        text: format!(
                            "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n{}\n</summary>",
                            summary
                        ),
                    }],
                    timestamp: yoagent::types::now_ms(),
                }))
            } else if ext.kind.starts_with("session/") {
                None // metadata entries not in context
            } else {
                Some(entry.message.clone())
            }
        }
    }
}

/// Get the message from an entry for compaction summarization (skips compaction entries).
fn get_message_from_entry_for_compaction(entry: &SessionEntry) -> Option<AgentMessage> {
    match &entry.message {
        AgentMessage::Extension(ext) if ext.kind == KIND_COMPACTION => None,
        _ => entry_context_message(entry),
    }
}

/// Estimate tokens for a single entry (uses yoagent::context::message_tokens).
fn estimate_entry_tokens(entry: &SessionEntry) -> u64 {
    entry_context_message(entry)
        .map(|m| message_tokens(&m) as u64)
        .unwrap_or(0)
}

// ═══════════════════════════════════════════════════════════════════════
// Cut point detection (pi-compatible)
// ═══════════════════════════════════════════════════════════════════════

fn is_cut_point_message(msg: &AgentMessage) -> bool {
    matches!(
        msg,
        AgentMessage::Llm(Message::User { .. }) | AgentMessage::Llm(Message::Assistant { .. })
    )
}

fn is_turn_start_message(msg: &AgentMessage) -> bool {
    matches!(msg, AgentMessage::Llm(Message::User { .. }))
        || matches!(msg, AgentMessage::Extension(ext) if ext.kind == crate::agent::session::KIND_BRANCH_SUMMARY)
}

fn is_turn_start_entry(entry: &SessionEntry) -> bool {
    entry_context_message(entry).is_some_and(|m| is_turn_start_message(&m))
}

/// Find valid cut points: indices of entries whose context messages are cut-point-eligible.
fn find_valid_cut_points(
    entries: &[SessionEntry],
    start_index: usize,
    end_index: usize,
) -> Vec<usize> {
    let mut points = Vec::new();
    for (i, entry) in entries.iter().enumerate().take(end_index).skip(start_index) {
        if let Some(msg) = entry_context_message(entry)
            && is_cut_point_message(&msg)
        {
            points.push(i);
        }
    }
    points
}

/// Find the turn-start entry index before a given entry.
fn find_turn_start_index(
    entries: &[SessionEntry],
    entry_index: usize,
    start_index: usize,
) -> isize {
    for i in (start_index..=entry_index).rev() {
        if is_turn_start_entry(&entries[i]) {
            return i as isize;
        }
    }
    -1
}

/// Result from find_cut_point (pi-compatible).
pub struct CutPointResult {
    pub first_kept_entry_index: usize,
    pub turn_start_index: isize,
    pub is_split_turn: bool,
}

/// Find the cut point in session entries that keeps approximately
/// `keep_recent_tokens` worth of recent context.
///
/// Algorithm matches pi: walk backwards from newest, accumulating estimated
/// message sizes (via yoagent::context::message_tokens). Stop when accumulated
/// >= keep_recent_tokens. Cut at the nearest valid cut point.
///
/// Never cuts at tool results. When cutting mid-turn, returns the split-turn info.
pub fn find_cut_point(
    entries: &[SessionEntry],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: u64,
) -> CutPointResult {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);

    if cut_points.is_empty() {
        return CutPointResult {
            first_kept_entry_index: start_index,
            turn_start_index: -1,
            is_split_turn: false,
        };
    }

    // Walk backwards from newest, accumulating estimated message sizes
    let mut accumulated: u64 = 0;
    let mut cut_index = cut_points[0]; // Default: keep from first valid cut

    for i in (start_index..end_index).rev() {
        let tokens = estimate_entry_tokens(&entries[i]);
        if tokens == 0 {
            continue;
        }
        accumulated = accumulated.saturating_add(tokens);

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

    // Scan backwards from cut_index to include adjacent metadata entries
    let mut adjusted_cut = cut_index;
    while adjusted_cut > start_index {
        let prev = &entries[adjusted_cut - 1];
        if entry_context_message(prev).is_some() {
            break;
        }
        adjusted_cut -= 1;
    }
    cut_index = adjusted_cut;

    // Determine if this is a split turn
    let starts_turn = cut_index < entries.len() && is_turn_start_entry(&entries[cut_index]);
    let turn_start_index = if starts_turn {
        -1
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };

    CutPointResult {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !starts_turn && turn_start_index >= 0,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Compaction Preparation (pi-compatible)
// ═══════════════════════════════════════════════════════════════════════

/// Preparation data for a compaction run.
pub struct CompactionPreparation {
    pub first_kept_entry_id: String,
    pub messages_to_summarize: Vec<AgentMessage>,
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: u64,
    pub previous_summary: Option<String>,
    pub file_ops: FileOperations,
}

/// Prepare compaction data from session entries.
///
/// Finds the cut point, extracts messages to summarize, handles split turns,
/// and collects file operations.
pub fn prepare_compaction(
    path_entries: &[SessionEntry],
    settings: &CompactionSettings,
) -> Option<CompactionPreparation> {
    if !settings.enabled || path_entries.is_empty() {
        return None;
    }

    // Skip if the last entry is already a compaction
    if let Some(last) = path_entries.last()
        && let AgentMessage::Extension(ext) = &last.message
        && ext.kind == KIND_COMPACTION
    {
        return None;
    }

    // Find the most recent previous compaction
    let prev_compaction_index = path_entries.iter().rposition(
        |e| matches!(&e.message, AgentMessage::Extension(ext) if ext.kind == KIND_COMPACTION),
    );

    // Extract previous summary and boundary start (pi-compatible: uses firstKeptEntryId)
    let previous_summary: Option<String>;
    let boundary_start: usize;

    if let Some(prev_idx) = prev_compaction_index {
        if let AgentMessage::Extension(ext) = &path_entries[prev_idx].message {
            previous_summary = ext.data["summary"].as_str().map(|s| s.to_string());
            let first_kept_id = ext.data["firstKeptEntryId"].as_str().map(|s| s.to_string());
            if let Some(ref fkid) = first_kept_id {
                let fki = path_entries.iter().position(|e| e.id == *fkid);
                boundary_start = fki.unwrap_or(prev_idx + 1);
            } else {
                boundary_start = prev_idx + 1;
            }
        } else {
            previous_summary = None;
            boundary_start = prev_idx + 1;
        }
    } else {
        previous_summary = None;
        boundary_start = 0;
    }

    let boundary_end = path_entries.len();

    // Token estimate for the entire context via yoagent
    let tokens_before: u64 = path_entries
        .iter()
        .filter_map(entry_context_message)
        .map(|m| message_tokens(&m) as u64)
        .sum();

    // Find cut point
    let cut = find_cut_point(
        path_entries,
        boundary_start,
        boundary_end,
        settings.keep_recent_tokens,
    );

    let first_kept_entry = &path_entries[cut.first_kept_entry_index];
    let first_kept_entry_id = first_kept_entry.id.clone();

    // Determine the end of history to summarize
    let history_end = if cut.is_split_turn {
        cut.turn_start_index as usize
    } else {
        cut.first_kept_entry_index
    };

    // Messages to summarize (will be discarded after summary)
    let messages_to_summarize: Vec<AgentMessage> = path_entries[boundary_start..history_end]
        .iter()
        .filter_map(get_message_from_entry_for_compaction)
        .collect();

    // Messages for turn prefix summary (when splitting a turn)
    let mut turn_prefix_messages = Vec::new();
    if cut.is_split_turn {
        let turn_start = cut.turn_start_index as usize;
        turn_prefix_messages = path_entries[turn_start..cut.first_kept_entry_index]
            .iter()
            .filter_map(get_message_from_entry_for_compaction)
            .collect();
    }

    if messages_to_summarize.is_empty() && turn_prefix_messages.is_empty() {
        return None;
    }

    // Extract file operations
    let file_ops = collect_file_operations(
        &messages_to_summarize,
        path_entries,
        prev_compaction_index.map(|i| i as isize).unwrap_or(-1),
    );

    // Also extract file ops from turn prefix if splitting
    if cut.is_split_turn {
        for msg in &turn_prefix_messages {
            extract_file_ops_from_message(msg, &mut (file_ops.clone()));
        }
    }

    Some(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages,
        is_split_turn: cut.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
    })
}

// ═══════════════════════════════════════════════════════════════════════
// Message Serialization for Summarization
// ═══════════════════════════════════════════════════════════════════════

const TOOL_RESULT_MAX_CHARS: usize = 2000;

fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let truncated = text.len() - max_chars;
    format!(
        "{}\n\n[... {} more characters truncated]",
        &text[..max_chars],
        truncated
    )
}

/// Serialize messages to text for summarization (pi-compatible format).
/// Prevents the model from treating it as a conversation to continue.
/// Tool results are truncated to keep the summarization request within
/// reasonable token budgets.
pub fn serialize_conversation(messages: &[AgentMessage]) -> String {
    let mut parts = Vec::new();

    for msg in messages {
        match msg {
            AgentMessage::Llm(m) => match m {
                Message::User { content, .. } => {
                    let text = content_text(content);
                    if !text.is_empty() {
                        parts.push(format!("[User]: {}", text));
                    }
                }
                Message::Assistant { content, .. } => {
                    let mut thinking_parts: Vec<&str> = Vec::new();
                    let mut text_parts: Vec<&str> = Vec::new();
                    let mut tool_calls: Vec<String> = Vec::new();

                    for block in content {
                        match block {
                            Content::Text { text } => text_parts.push(text.as_str()),
                            Content::Thinking { thinking, .. } => {
                                thinking_parts.push(thinking.as_str());
                            }
                            Content::ToolCall {
                                name, arguments, ..
                            } => {
                                let args_str: Vec<String> = arguments
                                    .as_object()
                                    .map(|obj| {
                                        obj.iter()
                                            .map(|(k, v)| {
                                                format!(
                                                    "{}={}",
                                                    k,
                                                    serde_json::to_string(v).unwrap_or_default()
                                                )
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                tool_calls.push(format!("{}({})", name, args_str.join(", ")));
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
                    let text = content_text(content);
                    if !text.is_empty() {
                        parts.push(format!(
                            "[Tool result ({})]: {}",
                            tool_name,
                            truncate_for_summary(&text, TOOL_RESULT_MAX_CHARS)
                        ));
                    }
                }
            },
            AgentMessage::Extension(ext) => {
                // Include branch summaries for context
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

// ═══════════════════════════════════════════════════════════════════════
// Summarization Prompts (pi-compatible)
// ═══════════════════════════════════════════════════════════════════════

pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation \
     between a user and an AI assistant, then produce a structured summary following \
     the exact format specified.\n\n\
     Do NOT continue the conversation. Do NOT respond to any questions in the \
     conversation. ONLY output the structured summary.";

const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or \"(none)\" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed
- UPDATE \"Next Steps\" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

// ═══════════════════════════════════════════════════════════════════════
// Summarization Generation
// ═══════════════════════════════════════════════════════════════════════

/// Build a prompt for the LLM summarizer.
pub fn build_summarization_prompt(
    messages: &[AgentMessage],
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
) -> String {
    let conversation_text = serialize_conversation(messages);

    let base_prompt = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT
    } else {
        SUMMARIZATION_PROMPT
    };

    let base = if let Some(instructions) = custom_instructions {
        format!("{}\n\nAdditional focus: {}", base_prompt, instructions)
    } else {
        base_prompt.to_string()
    };

    if let Some(prev) = previous_summary {
        format!(
            "<conversation>\n{}\n</conversation>\n\n<previous-summary>\n{}\n</previous-summary>\n\n{}",
            conversation_text, prev, base
        )
    } else {
        format!(
            "<conversation>\n{}\n</conversation>\n\n{}",
            conversation_text, base
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Branch Summarization
// ═══════════════════════════════════════════════════════════════════════

/// Collect entries between `old_leaf_id` and the common ancestor with `target_id`.
pub fn collect_entries_for_branch_summary<'a>(
    lookup_entry: &dyn Fn(&str) -> Option<&'a SessionEntry>,
    old_leaf_id: Option<&str>,
    target_id: &str,
) -> (Vec<&'a SessionEntry>, Option<String>) {
    let leaf = match old_leaf_id {
        Some(id) if !id.is_empty() => id,
        _ => return (vec![], None),
    };

    // Walk from leaf to root collecting ids
    let mut leaf_path: Vec<String> = Vec::new();
    let mut cursor: Option<&str> = Some(leaf);
    while let Some(id) = cursor {
        leaf_path.push(id.to_string());
        cursor = lookup_entry(id).and_then(|e| e.parent_id.as_deref());
    }

    // Walk from target to root collecting ids
    let mut target_path: Vec<String> = Vec::new();
    cursor = Some(target_id);
    while let Some(id) = cursor {
        target_path.push(id.to_string());
        cursor = lookup_entry(id).and_then(|e| e.parent_id.as_deref());
    }

    // Find common ancestor
    let mut common_ancestor_id = None;
    for leaf_id in &leaf_path {
        if target_path.iter().any(|tid| tid == leaf_id) {
            common_ancestor_id = Some(leaf_id.clone());
            break;
        }
    }

    // Collect entries from leaf down to (but not including) common ancestor
    let collected: Vec<&SessionEntry> = leaf_path
        .iter()
        .filter_map(|id| lookup_entry(id))
        .take_while(|e| Some(e.id.as_str()) != common_ancestor_id.as_deref())
        .collect();

    // Reverse to get chronological order
    let mut collected = collected;
    collected.reverse();

    (collected, common_ancestor_id)
}

/// Extract AgentMessage from a session entry for branch summarization.
fn branch_entry_message(entry: &SessionEntry) -> Option<AgentMessage> {
    match &entry.message {
        AgentMessage::Llm(m) => match m {
            Message::Assistant {
                error_message: Some(_),
                ..
            } => None,
            Message::ToolResult { .. } => None,
            _ => Some(entry.message.clone()),
        },
        AgentMessage::Extension(ext) => {
            if ext.kind == KIND_COMPACTION {
                let summary = ext.data["summary"].as_str().unwrap_or("");
                if summary.is_empty() {
                    return None;
                }
                Some(AgentMessage::Llm(Message::User {
                    content: vec![Content::Text {
                        text: format!(
                            "The conversation history before this point was compacted into the following summary:\n\n<summary>\n{}\n</summary>",
                            summary
                        ),
                    }],
                    timestamp: yoagent::types::now_ms(),
                }))
            } else if ext.kind == crate::agent::session::KIND_BRANCH_SUMMARY {
                let summary = ext.data["summary"].as_str().unwrap_or("");
                if summary.is_empty() {
                    return None;
                }
                Some(AgentMessage::Llm(Message::User {
                    content: vec![Content::Text {
                        text: format!(
                            "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n{}\n</summary>",
                            summary
                        ),
                    }],
                    timestamp: yoagent::types::now_ms(),
                }))
            } else if ext.kind.starts_with("session/") {
                None
            } else {
                Some(entry.message.clone())
            }
        }
    }
}

/// Build the prompt for branch summarization.
pub fn build_branch_summary_prompt(
    entries: &[&SessionEntry],
) -> Option<(Vec<AgentMessage>, FileOperations)> {
    let mut messages = Vec::new();
    let mut file_ops = create_file_ops();

    // First pass: collect file ops from existing branch summaries
    for entry in entries {
        if let AgentMessage::Extension(ext) = &entry.message
            && ext.kind == crate::agent::session::KIND_BRANCH_SUMMARY
            && let Some(details) = ext.data.get("details")
            && let Ok(d) = serde_json::from_value::<CompactionDetails>(details.clone())
        {
            for f in d.read_files {
                file_ops.read.push(f);
            }
            for f in d.modified_files {
                file_ops.edited.push(f);
            }
        }
    }

    // Second pass: extract messages
    for entry in entries {
        let msg = branch_entry_message(entry);
        if let Some(msg) = msg {
            extract_file_ops_from_message(&msg, &mut file_ops);
            messages.push(msg);
        }
    }

    if messages.is_empty() {
        return None;
    }

    Some((messages, file_ops))
}

const BRANCH_SUMMARY_PROMPT: &str =
    "Create a structured summary of this conversation branch for context when returning later.

Use this EXACT format:

## Goal
[What was the user trying to accomplish in this branch?]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Work that was started but not finished]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [What should happen next to continue this work]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

/// Build the prompt text and metadata for a branch summary.
pub fn build_branch_summary_text(
    entries: &[&SessionEntry],
) -> Option<(String, Vec<String>, Vec<String>)> {
    let (messages, file_ops) = build_branch_summary_prompt(entries)?;
    let conversation_text = serialize_conversation(&messages);
    let prompt = format!(
        "<conversation>\n{}\n</conversation>\n\n{}",
        conversation_text, BRANCH_SUMMARY_PROMPT
    );
    let (read_files, modified_files) = compute_file_lists(&file_ops);
    Some((prompt, read_files, modified_files))
}
