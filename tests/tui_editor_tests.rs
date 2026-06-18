// Tests for editor behavior: history recall, slash completion, common prefix.
// These test the pure logic extracted from tui.rs.

/// Pure logic for history recall (extracted from tui.rs).
/// Given user messages (newest last) and current history index (None = at end),
/// returns the new index and text for the given direction (-1 = older, 1 = newer).
fn recall_history(
    user_messages: &[&str],
    current_index: Option<usize>,
    direction: isize,
) -> (Option<usize>, Option<String>) {
    if user_messages.is_empty() {
        return (None, None);
    }

    let len = user_messages.len();
    let current = current_index.unwrap_or(len);

    let new_index = if direction < 0 {
        if current == 0 {
            return (current_index, None);
        }
        current.saturating_sub(1)
    } else {
        if current >= len {
            return (current_index, None);
        }
        current + 1
    };

    if new_index >= len {
        (None, None)
    } else {
        (Some(new_index), Some(user_messages[new_index].to_string()))
    }
}

/// Pure logic for slash command resolution (extracted from tui.rs submit_message).
/// Returns the resolved command name if exactly one matches, or None.
fn resolve_slash_command(typed: &str, available_commands: &[&str]) -> Option<String> {
    let (cmd_part, _args) = match typed.trim().split_once(' ') {
        Some((cmd, rest)) => (cmd.trim_start_matches('/'), rest),
        None => (typed.trim().trim_start_matches('/'), ""),
    };
    let lower = cmd_part.to_lowercase();

    // Exact match
    if available_commands.contains(&cmd_part) {
        return Some(cmd_part.to_string());
    }

    // Prefix match
    let matches: Vec<&&str> = available_commands
        .iter()
        .filter(|c| c.to_lowercase().starts_with(&lower))
        .collect();

    if matches.len() == 1 {
        Some((**matches[0]).to_string())
    } else {
        None
    }
}

/// Find common prefix of a list of strings (extracted from tui.rs).
fn common_prefix(strings: &[&str]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    let first = strings[0];
    let mut end = first.len();
    for s in &strings[1..] {
        end = end.min(
            first
                .chars()
                .zip(s.chars())
                .take(end)
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    first[..end].to_string()
}

// ── History recall tests ──────────────────────────────────────────

#[test]
fn history_empty_returns_none() {
    let result = recall_history(&[], None, -1);
    assert_eq!(result, (None, None));
}

#[test]
fn history_first_recall_goes_to_newest() {
    let msgs = &["hello", "what is rust?"];
    let result = recall_history(msgs, None, -1);
    assert_eq!(result, (Some(1), Some("what is rust?".into())));
}

#[test]
fn history_second_recall_goes_to_older() {
    let msgs = &["hello", "what is rust?"];
    let result = recall_history(msgs, Some(1), -1);
    assert_eq!(result, (Some(0), Some("hello".into())));
}

#[test]
fn history_at_oldest_stays_put() {
    let msgs = &["hello"];
    let result = recall_history(msgs, Some(0), -1);
    assert_eq!(result, (Some(0), None));
}

#[test]
fn history_forward_from_oldest() {
    let msgs = &["hello", "what is rust?"];
    let result = recall_history(msgs, Some(0), 1);
    assert_eq!(result, (Some(1), Some("what is rust?".into())));
}

#[test]
fn history_forward_past_newest_clears() {
    let msgs = &["hello"];
    let (idx, text) = recall_history(msgs, None, 1);
    assert_eq!(idx, None);
    assert_eq!(text, None);
}

#[test]
fn history_forward_past_end_clears() {
    let msgs = &["hello", "what is rust?"];
    let (idx, text) = recall_history(msgs, Some(1), 1);
    assert_eq!(idx, None);
    assert_eq!(text, None);
}

// ── Slash command resolution tests ────────────────────────────────

#[test]
fn slash_exact_match() {
    let cmds = &["quit", "model"];
    assert_eq!(resolve_slash_command("/quit", cmds), Some("quit".into()));
    assert_eq!(resolve_slash_command("/model", cmds), Some("model".into()));
}

#[test]
fn slash_prefix_unique_match() {
    let cmds = &["quit", "model"];
    assert_eq!(resolve_slash_command("/q", cmds), Some("quit".into()));
    assert_eq!(resolve_slash_command("/mo", cmds), Some("model".into()));
}

#[test]
fn slash_prefix_case_insensitive() {
    let cmds = &["quit", "model"];
    assert_eq!(resolve_slash_command("/Q", cmds), Some("quit".into()));
    assert_eq!(resolve_slash_command("/MODEL", cmds), Some("model".into()));
}

#[test]
fn slash_prefix_ambiguous_returns_none() {
    // /m matches both "model" and "mycommand" — should not resolve
    let cmds = &["model", "mycommand"];
    assert_eq!(resolve_slash_command("/m", cmds), None);
}

#[test]
fn slash_unknown_command_returns_none() {
    let cmds = &["quit", "model"];
    assert_eq!(resolve_slash_command("/unknown", cmds), None);
}

#[test]
fn slash_with_args_preserves_args() {
    // Only the command part is resolved; args are separate
    let cmds = &["quit", "model"];
    assert_eq!(
        resolve_slash_command("/model deepseek-v4-flash", cmds),
        Some("model".into())
    );
}

#[test]
fn slash_only_slash_returns_none() {
    let cmds = &["quit", "model"];
    // "/" prefix is "" — multiple prefix matches, should return None
    assert_eq!(resolve_slash_command("/", cmds), None);
}

// ── Common prefix tests ───────────────────────────────────────────

#[test]
fn common_prefix_empty() {
    assert_eq!(common_prefix(&[]), "");
}

#[test]
fn common_prefix_single() {
    assert_eq!(common_prefix(&["hello"]), "hello");
}

#[test]
fn common_prefix_full_match() {
    assert_eq!(common_prefix(&["hello", "hello world"]), "hello");
}

#[test]
fn common_prefix_partial() {
    assert_eq!(
        common_prefix(&["deepseek-v4-flash", "deepseek-v4-pro"]),
        "deepseek-v4-"
    );
}

#[test]
fn common_prefix_no_match() {
    assert_eq!(common_prefix(&["abc", "xyz"]), "");
}
