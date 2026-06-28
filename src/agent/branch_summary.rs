use crate::agent::compaction;
use crate::agent::compaction::{CompactionSettings, estimate_tokens};
use crate::agent::session::{Session, SessionEntry};
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
    session: &Session,
    old_leaf_id: Option<&str>,
    target_id: &str,
) -> (Vec<SessionEntry>, Option<String>) {
    let Some(old_leaf) = old_leaf_id else {
        return (vec![], None);
    };

    // Build set of ids on the path from old leaf to root
    let old_path: HashSet<String> = session
        .get_branch(Some(old_leaf))
        .unwrap_or_default()
        .iter()
        .map(|e| e.id().to_string())
        .collect();

    // Walk target path from root to leaf, find deepest common ancestor
    let target_path = session.get_branch(Some(target_id)).unwrap_or_default();
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
        if let Some(entry) = session.get_entry(cur_id) {
            let parent = entry.parent_id().map(|s| s.to_string());
            entries.push(entry);
            current = parent;
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
    session: &mut Session,
    entries: &[SessionEntry],
    target_id: &str,
    api_key: &str,
    model: &str,
    thinking_level: yoagent::types::ThinkingLevel,
    model_config: Option<yoagent::provider::model::ModelConfig>,
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

    let summary = compaction::summarize_text(
        api_key,
        model,
        system_prompt,
        &[summary_msg],
        thinking_level,
        model_config,
    )
    .await?;

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
mod tests {
    use super::*;
    use crate::agent::session::{BranchSummaryEntry, MessageEntry, SessionEntry, SessionManager};
    use crate::agent::types::{assistant_message, user_message};
    use std::path::Path;

    /// Push an entry into the SessionManager via the storage layer.
    fn push_entry(sm: &mut SessionManager, entry: SessionEntry) -> String {
        let id = entry.id().to_string();
        sm.session_mut()
            .get_storage_mut()
            .append_entry(entry)
            .unwrap();
        id
    }

    /// Build a minimal message entry (user message).
    fn msg_entry(parent_id: Option<&str>) -> SessionEntry {
        SessionEntry::Message(MessageEntry {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            timestamp: String::new(),
            message: user_message("test"),
        })
    }

    fn asst_entry(parent_id: Option<&str>) -> SessionEntry {
        SessionEntry::Message(MessageEntry {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            timestamp: String::new(),
            message: assistant_message("response"),
        })
    }

    fn branch_summary_entry(parent_id: Option<&str>, from_id: &str) -> SessionEntry {
        SessionEntry::BranchSummary(BranchSummaryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            timestamp: String::new(),
            from_id: from_id.to_string(),
            summary: "branch summary".into(),
            details: None,
            from_hook: None,
        })
    }

    /// Build a SessionManager with a simple linear chain of entries.
    fn linear_chain(n: usize) -> (SessionManager, Vec<String>) {
        let mut sm = SessionManager::in_memory(Path::new("/tmp/test"));
        let mut ids = Vec::new();
        for i in 0..n {
            let entry = msg_entry(ids.last().map(|s: &String| s.as_str()));
            let id = push_entry(&mut sm, entry);
            ids.push(id);
            if i % 2 == 1 && i + 1 < n {
                let asst = asst_entry(ids.last().map(|s: &String| s.as_str()));
                let asst_id = push_entry(&mut sm, asst);
                ids.push(asst_id);
            }
        }
        (sm, ids)
    }

    #[test]
    fn test_collect_entries_no_old_leaf() {
        let (sm, _ids) = linear_chain(3);
        let target_id = sm.entries().last().unwrap().id().to_string();
        let (entries, ancestor) =
            collect_entries_for_branch_summary(sm.session(), None, &target_id);
        assert!(entries.is_empty());
        assert!(ancestor.is_none());
    }

    #[test]
    fn test_collect_entries_same_branch() {
        let (sm, ids) = linear_chain(5);
        let old_leaf = ids.last().unwrap();
        let target_id = ids.last().unwrap();
        let (entries, ancestor) =
            collect_entries_for_branch_summary(sm.session(), Some(old_leaf), target_id);
        assert!(entries.is_empty());
        assert!(ancestor.is_some());
    }

    #[test]
    fn test_collect_entries_different_branches() {
        let mut sm = SessionManager::in_memory(Path::new("/tmp/test"));

        // Root: entry A
        let entry_a = msg_entry(None);
        let id_a = push_entry(&mut sm, entry_a);

        // Branch 1: A -> B -> C
        let entry_b = msg_entry(Some(&id_a));
        let id_b = push_entry(&mut sm, entry_b);
        let entry_c = msg_entry(Some(&id_b));
        let id_c = push_entry(&mut sm, entry_c);

        // Branch 2: A -> D -> E
        let entry_d = msg_entry(Some(&id_a));
        let id_d = push_entry(&mut sm, entry_d);
        let entry_e = msg_entry(Some(&id_d));
        let id_e = push_entry(&mut sm, entry_e);

        let (entries, ancestor) =
            collect_entries_for_branch_summary(sm.session(), Some(&id_e), &id_c);

        assert_eq!(ancestor.as_deref(), Some(id_a.as_str()));
        assert_eq!(entries.len(), 2, "should collect E and D");
        assert_eq!(entries[0].id(), id_d.as_str());
        assert_eq!(entries[1].id(), id_e.as_str());
    }

    #[test]
    fn test_collect_entries_uses_branch_summary_as_ancestor() {
        let mut sm = SessionManager::in_memory(Path::new("/tmp/test"));

        // Root: A
        let entry_a = msg_entry(None);
        let id_a = push_entry(&mut sm, entry_a);

        // Branch summary from A
        let bs = branch_summary_entry(Some(&id_a), &id_a);
        let id_bs = push_entry(&mut sm, bs);

        // Branch 1: bs -> B
        let entry_b = msg_entry(Some(&id_bs));
        let id_b = push_entry(&mut sm, entry_b);

        // Branch 2: bs -> C
        let entry_c = msg_entry(Some(&id_bs));
        let id_c = push_entry(&mut sm, entry_c);

        let (entries, ancestor) =
            collect_entries_for_branch_summary(sm.session(), Some(&id_c), &id_b);

        assert_eq!(ancestor.as_deref(), Some(id_bs.as_str()));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id(), id_c.as_str());
    }

    #[test]
    fn test_prepare_branch_entries_empty_collected() {
        let result = prepare_branch_entries(&[], 1000);
        assert!(result.0.is_empty());
    }

    #[test]
    fn test_prepare_branch_entries_with_entries() {
        let (sm, _ids) = linear_chain(3);
        let entries: Vec<SessionEntry> = sm.entries().to_vec();
        let result = prepare_branch_entries(&entries, 1000);
        assert!(!result.0.is_empty());
        assert!(result.1 > 0);
    }
}
