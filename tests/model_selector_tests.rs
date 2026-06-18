// Tests for model selector behavior: filtering, navigation, and slash command resolution.
// These test the pure logic extracted from tui.rs.

/// Pure logic for filtering a list of model names by a case-insensitive query.
fn filter_models<'a>(models: &'a [&'a str], query: &str) -> Vec<&'a str> {
    if query.is_empty() {
        return models.to_vec();
    }
    let lower = query.to_lowercase();
    models
        .iter()
        .filter(|m| m.to_lowercase().contains(&lower))
        .copied()
        .collect()
}

/// Pure logic for navigating the model list with wrap-around.
/// Returns the new selection index given the current one and direction.
fn navigate_selection(current: usize, count: usize, direction: isize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let max = count.saturating_sub(1);
    let next = match direction {
        -1 => {
            if current == 0 {
                max
            } else {
                current - 1
            }
        }
        1 => {
            if current >= max {
                0
            } else {
                current + 1
            }
        }
        _ => current,
    };
    Some(next)
}

/// Pure logic for updating the search string on a character input.
/// Returns (new_search, new_selection).
fn search_add_char(search: &str, c: char) -> String {
    let mut s = search.to_string();
    s.push(c);
    s
}

/// Pure logic for updating the search string on backspace.
fn search_backspace(search: &str) -> String {
    let mut s = search.to_string();
    s.pop();
    s
}

/// Resolve a slash command and determine if it should open the model selector.
/// Returns (resolved_command_name, should_open_selector) or None if unresolved.
fn resolve_slash_command(typed: &str, available_commands: &[&str]) -> Option<(String, bool)> {
    let (cmd_part, args) = match typed.trim().split_once(' ') {
        Some((cmd, rest)) => (cmd.trim_start_matches('/'), rest),
        None => (typed.trim().trim_start_matches('/'), ""),
    };
    let lower = cmd_part.to_lowercase();

    // Exact match
    if available_commands.contains(&cmd_part) {
        let is_model_empty = cmd_part == "model" && args.is_empty();
        return Some((cmd_part.to_string(), is_model_empty));
    }

    // Prefix match
    let matches: Vec<&&str> = available_commands
        .iter()
        .filter(|c| c.to_lowercase().starts_with(&lower))
        .collect();

    if matches.len() == 1 {
        let name = (*matches[0]).to_string();
        let is_model_empty = name == "model" && args.is_empty();
        Some((name, is_model_empty))
    } else {
        None
    }
}

// ── Filter tests ──────────────────────────────────────────────────

#[test]
fn filter_empty_query_returns_all() {
    let models = &["deepseek-v4-flash", "deepseek-v4-pro", "claude-3-opus"];
    let result = filter_models(models, "");
    assert_eq!(result.len(), 3);
    assert_eq!(
        result,
        vec!["deepseek-v4-flash", "deepseek-v4-pro", "claude-3-opus"]
    );
}

#[test]
fn filter_empty_list_returns_empty() {
    let models: &[&str] = &[];
    let result = filter_models(models, "deepseek");
    assert!(result.is_empty());
}

#[test]
fn filter_substring_match() {
    let models = &[
        "deepseek-v4-flash",
        "deepseek-v4-pro",
        "claude-3-opus",
        "gpt-4",
    ];
    let result = filter_models(models, "deepseek");
    assert_eq!(result.len(), 2);
    assert!(result.contains(&"deepseek-v4-flash"));
    assert!(result.contains(&"deepseek-v4-pro"));
}

#[test]
fn filter_case_insensitive() {
    let models = &["DeepSeek-V4-Flash", "claude-3-opus"];
    let result = filter_models(models, "deepseek");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "DeepSeek-V4-Flash");
}

#[test]
fn filter_partial_substring() {
    let models = &["deepseek-v4-flash", "deepseek-v4-pro", "claude-3-opus"];
    let result = filter_models(models, "pro");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "deepseek-v4-pro");
}

#[test]
fn filter_no_match_returns_empty() {
    let models = &["deepseek-v4-flash", "deepseek-v4-pro"];
    let result = filter_models(models, "nonexistent");
    assert!(result.is_empty());
}

#[test]
fn filter_single_char_query() {
    let models = &["deepseek-v4-flash", "deepseek-v4-pro", "claude-3-opus"];
    let result = filter_models(models, "c");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "claude-3-opus");
}

#[test]
fn filter_match_at_end_of_string() {
    let models = &["model-flash", "model-pro", "other"];
    let result = filter_models(models, "flash");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "model-flash");
}

// ── Navigation tests ──────────────────────────────────────────────

#[test]
fn navigate_empty_list() {
    assert_eq!(navigate_selection(0, 0, -1), None);
    assert_eq!(navigate_selection(0, 0, 1), None);
}

#[test]
fn navigate_single_item() {
    assert_eq!(navigate_selection(0, 1, -1), Some(0)); // wraps to same
    assert_eq!(navigate_selection(0, 1, 1), Some(0)); // wraps to same
}

#[test]
fn navigate_down_wraps_to_top() {
    let models = &["a", "b", "c"];
    let result = navigate_selection(2, models.len(), 1);
    assert_eq!(result, Some(0));
}

#[test]
fn navigate_up_wraps_to_bottom() {
    let models = &["a", "b", "c"];
    let result = navigate_selection(0, models.len(), -1);
    assert_eq!(result, Some(2));
}

#[test]
fn navigate_down_mid_list() {
    let models = &["a", "b", "c"];
    assert_eq!(navigate_selection(0, models.len(), 1), Some(1));
    assert_eq!(navigate_selection(1, models.len(), 1), Some(2));
}

#[test]
fn navigate_up_mid_list() {
    let models = &["a", "b", "c"];
    assert_eq!(navigate_selection(2, models.len(), -1), Some(1));
    assert_eq!(navigate_selection(1, models.len(), -1), Some(0));
}

#[test]
fn navigate_selection_at_zero_with_wrap() {
    let models = &["a", "b", "c"];
    assert_eq!(navigate_selection(0, models.len(), -1), Some(2));
}

#[test]
fn navigate_selection_at_end_with_wrap() {
    let models = &["a", "b", "c"];
    assert_eq!(navigate_selection(2, models.len(), 1), Some(0));
}

// ── Search string tests ───────────────────────────────────────────

#[test]
fn search_add_char_empty() {
    assert_eq!(search_add_char("", 'd'), "d");
}

#[test]
fn search_add_char_appends() {
    assert_eq!(search_add_char("deep", 's'), "deeps");
}

#[test]
fn search_backspace_empty() {
    assert_eq!(search_backspace(""), "");
}

#[test]
fn search_backspace_removes_last() {
    assert_eq!(search_backspace("deep"), "dee");
}

#[test]
fn search_backspace_single() {
    assert_eq!(search_backspace("d"), "");
}

// ── Slash command resolution with model selector detection ───────

#[test]
fn slash_model_exact_no_args_opens_selector() {
    let cmds = &["quit", "model"];
    let result = resolve_slash_command("/model", cmds);
    assert_eq!(result, Some(("model".into(), true)));
}

#[test]
fn slash_model_exact_with_args_does_not_open_selector() {
    let cmds = &["quit", "model"];
    let result = resolve_slash_command("/model deepseek-v4-flash", cmds);
    assert_eq!(result, Some(("model".into(), false)));
}

#[test]
fn slash_m_prefix_no_args_opens_selector() {
    let cmds = &["quit", "model"];
    let result = resolve_slash_command("/m", cmds);
    assert_eq!(result, Some(("model".into(), true)));
}

#[test]
fn slash_m_prefix_with_args_does_not_open_selector() {
    let cmds = &["quit", "model"];
    let result = resolve_slash_command("/m deepseek-v4-pro", cmds);
    assert_eq!(result, Some(("model".into(), false)));
}

#[test]
fn slash_quit_no_args_does_not_open_selector() {
    let cmds = &["quit", "model"];
    let result = resolve_slash_command("/quit", cmds);
    assert_eq!(result, Some(("quit".into(), false)));
}

#[test]
fn slash_ambiguous_prefix_resolves_to_none() {
    let cmds = &["model", "mycommand"];
    let result = resolve_slash_command("/m", cmds);
    assert_eq!(result, None);
}

#[test]
fn slash_unknown_command_resolves_to_none() {
    let cmds = &["quit", "model"];
    let result = resolve_slash_command("/unknown", cmds);
    assert_eq!(result, None);
}

#[test]
fn slash_model_case_insensitive_no_args_opens_selector() {
    let cmds = &["quit", "model"];
    let result = resolve_slash_command("/MODEL", cmds);
    assert_eq!(result, Some(("model".into(), true)));
}

#[test]
fn slash_m_case_insensitive_no_args_opens_selector() {
    let cmds = &["quit", "model"];
    let result = resolve_slash_command("/M", cmds);
    assert_eq!(result, Some(("model".into(), true)));
}
