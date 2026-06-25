use crate::agent::compaction::{CompactionSettings, estimate_tokens};
use crate::agent::yo_bridge;
use crate::agent::session::{SessionEntry, SessionManager};
use crate::agent::types::{AgentMessage, Role};
use std::collections::HashSet;

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
                if m.message.role == Role::ToolResult {
                    continue;
                }
                m.message.clone()
            }
            SessionEntry::BranchSummary(s) if !s.summary.is_empty() => AgentMessage {
                id: String::new(),
                parent_id: None,
                role: Role::Assistant,
                content: format!("[Branch: from {}] {}", s.from_id, s.summary),
                tool_calls: vec![],
                tool_call_id: None,
                usage: None,
                is_error: false,
                timestamp: 0,
            },
            SessionEntry::Compaction(c) => AgentMessage {
                id: String::new(),
                parent_id: None,
                role: Role::Assistant,
                content: format!(
                    "[Compaction: {} tokens → summary] {}",
                    c.tokens_before, c.summary
                ),
                tool_calls: vec![],
                tool_call_id: None,
                usage: None,
                is_error: false,
                timestamp: 0,
            },
            SessionEntry::CustomMessage(c) => AgentMessage {
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
            },
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
        let role_label = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::ToolResult => "Tool Result",
        };
        conversation_text.push_str(&format!(
            "<{}>\n{}\n</{}>\n",
            role_label, msg.content, role_label
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

    let summary_msg = AgentMessage::user(&prompt);
    let system_prompt = "You are a precise summarizer. Summarize the conversation branch above.";

    let summary = yo_bridge::summarize_text(api_key, model, system_prompt, &[summary_msg]).await?;

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
    read_files.retain(|f| !modified_files.contains(f));

    Some(serde_json::json!({
        "readFiles": read_files,
        "modifiedFiles": modified_files,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::session::{MessageEntry, SessionManager};
    use crate::agent::types::AgentMessage;
    use tempfile::TempDir;

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

    fn make_session_with_linear_history(count: usize) -> (TempDir, SessionManager) {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
        for i in 0..count {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            sm.append_message(&make_msg(role, &format!("msg {}", i)));
        }
        (tmp, sm)
    }

    #[test]
    fn test_collect_entries_same_path() {
        let (_tmp, sm) = make_session_with_linear_history(4);
        let entries = sm.entries();
        // All entries are sequential, navigating from leaf to leaf = nothing to summarize
        let (collected, ancestor) = collect_entries_for_branch_summary(
            &sm,
            Some(entries.last().unwrap().id()),
            entries.last().unwrap().id(),
        );
        assert!(collected.is_empty());
        assert_eq!(ancestor, Some(entries.last().unwrap().id().to_string()));
    }

    #[test]
    fn test_collect_entries_branch_path() {
        let (_tmp, mut sm) = make_session_with_linear_history(4);
        let entries_before = sm.entries();
        // entries have ids like 'm0', 'm1', 'm2', 'm3' (from MessageEntry)
        let branch_point_id = entries_before[2].id().to_string(); // index 2 = 3rd entry (user msg)

        sm.set_branch(&branch_point_id).unwrap();

        // Append new messages on the branch
        sm.append_message(&make_msg(Role::Assistant, "branch response 1"));
        sm.append_message(&make_msg(Role::User, "branch user 2"));

        let entries = sm.entries();
        // Should have 4 original + 2 new = 6 entries
        assert_eq!(entries.len(), 6);

        // Old leaf is the last entry before branching = entries[3]
        let old_leaf_id = entries[3].id().to_string();
        // Target is the current leaf = entries[5]
        let target_id = entries[5].id().to_string();

        let (collected, ancestor) =
            collect_entries_for_branch_summary(&sm, Some(&old_leaf_id), &target_id);

        // Collected should have entries from index 3 (the abandoned leaf)
        // going back to but not including branch_point_id (index 2)
        // The abandoned path is just entry[3].
        assert!(
            !collected.is_empty(),
            "should have collected abandoned entries"
        );
        assert_eq!(
            collected.len(),
            1,
            "only the one abandoned entry after branching"
        );
        assert_eq!(
            collected[0].id(),
            entries[3].id(),
            "the abandoned entry should be the one after the branch point"
        );
        assert_eq!(
            ancestor,
            Some(branch_point_id.clone()),
            "common ancestor should be the branch point"
        );
    }

    #[test]
    fn test_prepare_branch_entries_empty() {
        let (messages, tokens) = prepare_branch_entries(&[], 1000);
        assert!(messages.is_empty());
        assert_eq!(tokens, 0);
    }

    #[test]
    fn test_prepare_branch_entries_basic() {
        let entries: Vec<SessionEntry> = (0..4)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                SessionEntry::Message(MessageEntry {
                    id: format!("m{}", i),
                    parent_id: None,
                    timestamp: String::new(),
                    message: make_msg(role, &format!("content {}", i)),
                })
            })
            .collect();

        let (messages, tokens) = prepare_branch_entries(&entries, 0);
        assert_eq!(messages.len(), 4);
        assert!(tokens > 0);
    }

    #[test]
    fn test_prepare_branch_entries_skips_tool_results() {
        let entries: Vec<SessionEntry> = vec![
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
                message: make_msg(Role::Assistant, "tool call"),
            }),
            SessionEntry::Message(MessageEntry {
                id: "t1".to_string(),
                parent_id: Some("a1".to_string()),
                timestamp: String::new(),
                message: make_msg(Role::ToolResult, "result"),
            }),
        ];

        let (messages, _) = prepare_branch_entries(&entries, 0);
        assert_eq!(messages.len(), 2); // user + assistant, no tool result
    }
}
