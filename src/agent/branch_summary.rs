use crate::agent::compaction;
use crate::agent::compaction::{CompactionSettings, estimate_tokens};
use crate::agent::session::{SessionEntry, SessionManager};
use crate::agent::types::{
    assistant_message, message_is_assistant, message_is_tool_result, message_is_user, message_text,
    user_message,
};
use std::collections::HashSet;
use yoagent::types::AgentMessage;

/// Collect entries from an abandoned branch path for summarization.
///
/// Walks from `old_leaf_id` back to the common ancestor with `target_id`,
/// collecting entries along the way.
///
/// Returns the entries to summarize (chronological order) and the common ancestor id.
pub fn collect_entries_for_branch_summary(
    session: &SessionManager,
    old_leaf_id: Option<&str>,
    target_id: &str,
) -> (Vec<SessionEntry>, Option<String>) {
    let Some(old_leaf) = old_leaf_id else {
        return (vec![], None);
    };

    // Build set of ids on the path from old leaf to root
    let old_path: HashSet<&str> = session
        .branch(Some(old_leaf))
        .iter()
        .map(|e| e.id())
        .collect();

    // Walk target path from root to leaf, find deepest common ancestor
    let target_path = session.branch(Some(target_id));
    let mut common_ancestor_id: Option<String> = None;
    for entry in target_path.iter().rev() {
        if old_path.contains(entry.id()) {
            common_ancestor_id = Some(entry.id().to_string());
            break;
        }
    }

    // Collect entries from old leaf back to common ancestor
    let mut entries: Vec<SessionEntry> = Vec::new();
    let mut current: Option<String> = Some(old_leaf.to_string());

    while let Some(ref cur_id) = current {
        if Some(cur_id.as_str()) == common_ancestor_id.as_deref() {
            break;
        }
        if let Some(entry) = session.entry(cur_id) {
            entries.push(entry.clone());
            current = entry.parent_id().map(|s| s.to_string());
        } else {
            break;
        }
    }

    // Reverse to get chronological order
    entries.reverse();
    (entries, common_ancestor_id)
}

/// Extract messages from session entries for branch summarization.
///
/// Walks from newest to oldest, accumulating messages until token budget.
/// Returns messages and total token count.
pub fn prepare_branch_entries(
    entries: &[SessionEntry],
    token_budget: u64,
) -> (Vec<AgentMessage>, u64) {
    let mut messages: Vec<AgentMessage> = Vec::new();
    let mut total_tokens = 0u64;

    for entry in entries.iter().rev() {
        let msg = match entry {
            SessionEntry::Message(m) => {
                // Skip tool results - context is in the assistant's tool call
                if message_is_tool_result(&m.message) {
                    continue;
                }
                m.message.clone()
            }
            SessionEntry::BranchSummary(s) if !s.summary.is_empty() => {
                assistant_message(format!("[Branch: from {}] {}", s.from_id, s.summary))
            }
            SessionEntry::Compaction(c) => assistant_message(format!(
                "[Compaction: {} tokens → summary] {}",
                c.tokens_before, c.summary
            )),
            SessionEntry::CustomMessage(c) => assistant_message(format!(
                "[{}] {}",
                c.custom_type,
                serde_json::to_string(&c.content).unwrap_or_default()
            )),
            _ => continue,
        };

        let tokens = estimate_tokens(&msg);

        // Always include compaction/branch_summary entries if they're important context
        if token_budget > 0 && total_tokens + tokens > token_budget {
            let is_summary = matches!(
                entry,
                SessionEntry::Compaction(_) | SessionEntry::BranchSummary(_)
            );
            if !is_summary {
                break;
            }
            if total_tokens >= (token_budget as f64 * 0.9) as u64 {
                break;
            }
        }

        messages.insert(0, msg);
        total_tokens += tokens;
    }

    (messages, total_tokens)
}

/// Generate a branch summary for the given entries by calling the provider.
///
/// The summary is appended to the session as a `BranchSummaryEntry`.
/// Returns the summary text, or an error message.
pub async fn generate_branch_summary(
    session: &mut SessionManager,
    entries: &[SessionEntry],
    target_id: &str,
    api_key: &str,
    model: &str,
) -> Result<String, String> {
    let settings = CompactionSettings::default();
    let context_window = crate::agent::compaction::get_model_context_window(model);
    let token_budget = context_window.saturating_sub(settings.reserve_tokens);

    let (messages, _total_tokens) = prepare_branch_entries(entries, token_budget);

    if messages.is_empty() {
        return Err("No messages to summarize in branch".to_string());
    }

    // Serialize messages for the summarization prompt
    let mut conversation_text = String::new();
    for msg in &messages {
        let role_label = if message_is_user(msg) {
            "User"
        } else if message_is_assistant(msg) {
            "Assistant"
        } else {
            "Tool Result"
        };
        conversation_text.push_str(&format!(
            "<{}>\n{}\n</{}>\n",
            role_label,
            message_text(msg),
            role_label
        ));
    }

    let prompt = format!(
        r#"<conversation>
{conversation_text}
</conversation>

The user explored a different conversation branch before returning here.
Create a structured summary of this branch for context when continuing.

## Goal
[What was the user trying to accomplish in this branch?]

## Progress
### Done
- [x] [Completed changes]

### In Progress
- [ ] [Work started but not finished]

### Blocked
- [Issues, if any]

## Key Decisions
- [Decisions made]

## Next Steps
1. [What should happen next]

Keep it concise. Preserve exact file paths, function names, and error messages."#
    );

    let summary_msg = user_message(&prompt);
    let system_prompt = "You are a precise summarizer. Summarize the conversation branch above.";

    let summary = compaction::summarize_text(api_key, model, system_prompt, &[summary_msg]).await?;

    if summary.is_empty() {
        return Err("Branch summarization returned empty response".to_string());
    }

    // Prepend preamble
    let final_summary = format!(
        "The user explored a different conversation branch before returning here.\nSummary of that exploration:\n\n{}",
        summary
    );

    // Extract file operations from branch entries for details
    let details = extract_branch_file_ops(entries);

    // Append branch summary entry to session
    session.append_branch_summary(target_id, &final_summary, details, None);

    Ok(final_summary)
}

/// Extract file operations from a list of branch entries (for details metadata).
fn extract_branch_file_ops(entries: &[SessionEntry]) -> Option<serde_json::Value> {
    let mut read_files: Vec<String> = Vec::new();
    let mut modified_files: Vec<String> = Vec::new();

    for entry in entries {
        let msg = match entry {
            SessionEntry::Message(m) => &m.message,
            _ => continue,
        };
        if let yoagent::types::AgentMessage::Llm(yoagent::types::Message::Assistant {
            content: c,
            ..
        }) = msg
        {
            let tcs = crate::agent::types::content_tool_calls(c);
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
    read_files.retain(|f| !modified_files.contains(f));

    Some(serde_json::json!({
        "readFiles": read_files,
        "modifiedFiles": modified_files,
    }))
}

#[cfg(test)]
mod tests {}
