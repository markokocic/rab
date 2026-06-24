use crate::agent::ui::theme::ThemeKey;
use crate::tui::Theme;

/// Render a unified diff string with colored lines and intra-line change highlighting.
/// Matches pi's `renderDiff()` in `diff.ts`.
///
/// Input format (from edit tool's compute_diff):
/// `--- a/path` / `+++ b/path` / `@@ -1,5 +1,6 @@` / ` context` / `-removed` / `+added`
///
/// Output: ANSI-styled lines with:
/// - `-` lines: `toolDiffRemoved` (red), with inverse on changed tokens for single-line changes
/// - `+` lines: `toolDiffAdded` (green), with inverse on changed tokens for single-line changes
/// - ` ` lines: `toolDiffContext` (gray)
///
/// Multi-line changes show all removed lines first, then all added lines (no intra-line diff).
/// Single-line changes (1 removed + 1 added) render intra-line word-diff with inverse.
///
/// Takes a `&dyn Theme` parameter to avoid calling `current_theme()` which
/// would deadlock if the theme lock is already held by a caller.
pub fn render_diff(diff_text: &str, theme: &dyn Theme) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let diff_lines: Vec<&str> = diff_text.lines().collect();
    let mut i = 0;

    while i < diff_lines.len() {
        let line = diff_lines[i];

        // Skip unified diff headers
        if line.starts_with("---") || line.starts_with("+++") || line.starts_with("@@") {
            i += 1;
            continue;
        }

        if line.is_empty() {
            i += 1;
            continue;
        }

        let first = line.chars().next().unwrap_or(' ');

        if first == '-' {
            // Collect consecutive removed lines
            let mut removed: Vec<&str> = Vec::new();
            while i < diff_lines.len() {
                let l = diff_lines[i];
                if let Some(stripped) = l.strip_prefix('-') {
                    removed.push(stripped);
                    i += 1;
                } else if l.starts_with("---") {
                    // Skip hunk header prefix that could match
                    i += 1;
                } else {
                    break;
                }
            }

            // Collect consecutive added lines
            let mut added: Vec<&str> = Vec::new();
            while i < diff_lines.len() {
                let l = diff_lines[i];
                if let Some(stripped) = l.strip_prefix('+') {
                    added.push(stripped);
                    i += 1;
                } else {
                    break;
                }
            }

            // Single-line change: intra-line word diff
            if removed.len() == 1 && added.len() == 1 {
                render_intra_line_diff(
                    &replace_tabs(removed[0]),
                    &replace_tabs(added[0]),
                    &mut lines,
                    theme,
                );
            } else {
                // Multi-line change: show all removed, then all added
                for r in &removed {
                    let content = replace_tabs(r);
                    lines.push(theme.fg_key(ThemeKey::ToolDiffRemoved, &format!("-{}", content)));
                }
                for a in &added {
                    let content = replace_tabs(a);
                    lines.push(theme.fg_key(ThemeKey::ToolDiffAdded, &format!("+{}", content)));
                }
            }
        } else if first == '+' {
            // Standalone added line (no preceding removal)
            let content = replace_tabs(&line[1..]);
            lines.push(theme.fg_key(ThemeKey::ToolDiffAdded, &format!("+{}", content)));
            i += 1;
        } else {
            // Context line
            let content = replace_tabs(&line[1..]);
            lines.push(theme.fg_key(ThemeKey::ToolDiffContext, &format!(" {}", content)));
            i += 1;
        }
    }

    lines
}

/// Replace tabs with spaces for consistent rendering (matching pi's `replaceTabs`).
fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

/// Render intra-line diff for a single-line change (one removed, one added).
/// Uses word-level diff and applies inverse (reverse video) on changed parts.
///
/// Matches pi's `renderIntraLineDiff()` which uses `diffWords` to find changed
/// tokens and applies `theme.inverse()` on them.
/// Strips leading whitespace from inverse to avoid highlighting indentation.
fn render_intra_line_diff(old: &str, new: &str, output: &mut Vec<String>, theme: &dyn Theme) {
    let changes = compute_word_diff(old, new);

    let mut removed_line = String::new();
    let mut added_line = String::new();

    for change in &changes {
        match change {
            Change::Equal(text) => {
                removed_line.push_str(text);
                added_line.push_str(text);
            }
            Change::Removed(text) => {
                // Strip leading whitespace (matching pi's behavior)
                let trimmed = text.trim_start();
                if trimmed.len() < text.len() {
                    let ws = &text[..text.len() - trimmed.len()];
                    removed_line.push_str(ws);
                }
                if !trimmed.is_empty() {
                    removed_line.push_str(&theme.inverse(trimmed));
                }
            }
            Change::Added(text) => {
                // Strip leading whitespace
                let trimmed = text.trim_start();
                if trimmed.len() < text.len() {
                    let ws = &text[..text.len() - trimmed.len()];
                    added_line.push_str(ws);
                }
                if !trimmed.is_empty() {
                    added_line.push_str(&theme.inverse(trimmed));
                }
            }
        }
    }

    output.push(theme.fg_key(ThemeKey::ToolDiffRemoved, &format!("-{}", removed_line)));
    output.push(theme.fg_key(ThemeKey::ToolDiffAdded, &format!("+{}", added_line)));
}

/// A change in a diff: equal, removed, or added.
#[derive(Debug)]
enum Change {
    Equal(String),
    Removed(String),
    Added(String),
}

/// Compute a word-level diff between two strings.
/// Splits text into word tokens (alphanumeric sequences) and computes LCS.
/// Groups consecutive same-type changes for compact output.
/// Matches pi's `diffWords` behavior.
fn compute_word_diff(old: &str, new: &str) -> Vec<Change> {
    let old_tokens = split_words(old);
    let new_tokens = split_words(new);
    let n = old_tokens.len();
    let m = new_tokens.len();

    // Build LCS table (O(n*m), fine for short intra-line comparisons)
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            if old_tokens[i - 1] == new_tokens[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to extract diff
    let mut temp = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old_tokens[i - 1] == new_tokens[j - 1] {
            temp.push(Change::Equal(old_tokens[i - 1].clone()));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            temp.push(Change::Added(new_tokens[j - 1].clone()));
            j -= 1;
        } else {
            temp.push(Change::Removed(old_tokens[i - 1].clone()));
            i -= 1;
        }
    }
    temp.reverse();

    // Merge consecutive same-type changes
    let mut merged: Vec<Change> = Vec::new();
    for change in temp {
        let should_merge = merged.last().is_some_and(|last| {
            matches!(
                (last, &change),
                (Change::Equal(_), Change::Equal(_))
                    | (Change::Removed(_), Change::Removed(_))
                    | (Change::Added(_), Change::Added(_))
            )
        });

        if should_merge {
            if let Some(last) = merged.last_mut() {
                let text = match change {
                    Change::Equal(t) | Change::Removed(t) | Change::Added(t) => t,
                };
                match last {
                    Change::Equal(t) => t.push_str(&text),
                    Change::Removed(t) => t.push_str(&text),
                    Change::Added(t) => t.push_str(&text),
                }
            }
        } else {
            merged.push(change);
        }
    }

    merged
}

/// Split text into word tokens for diffing.
/// Alphanumeric sequences (including `_`) are kept as whole words;
/// everything else (whitespace, punctuation) becomes individual character tokens.
/// Matches pi's `diff.diffWords` behavior.
fn split_words(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            tokens.push(ch.to_string());
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> crate::agent::ui::theme::RabTheme {
        crate::agent::ui::theme::current_theme().clone()
    }

    #[test]
    fn test_empty_diff() {
        let theme = test_theme();
        let result = render_diff("", &theme);
        assert!(result.is_empty());
    }

    #[test]
    fn test_skips_headers() {
        let theme = test_theme();
        let diff = "--- a/file.rs\n+++ b/file.rs\n@@ -1,3 +1,4 @@\n";
        let result = render_diff(diff, &theme);
        assert!(result.is_empty(), "should skip all headers");
    }

    #[test]
    fn test_context_lines() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = test_theme();
        let diff = " line1\n line2\n";
        let result = render_diff(diff, &theme);
        assert_eq!(result.len(), 2);
        assert!(result[0].contains("line1"));
        assert!(result[0].starts_with("\x1b")); // has ANSI color
        assert!(result[0].contains("\x1b[39m")); // has reset
    }

    #[test]
    fn test_removed_line() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = test_theme();
        let diff = "-old_line\n";
        let result = render_diff(diff, &theme);
        assert_eq!(result.len(), 1);
        // Prefix should be preserved
        assert!(result[0].contains('-'));
        assert!(result[0].contains("old_line"));
    }

    #[test]
    fn test_added_line() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = test_theme();
        let diff = "+new_line\n";
        let result = render_diff(diff, &theme);
        assert_eq!(result.len(), 1);
        assert!(result[0].contains('+'));
        assert!(result[0].contains("new_line"));
    }

    #[test]
    fn test_single_line_modification() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = test_theme();
        let diff = "-foo\n+bar\n";
        let result = render_diff(diff, &theme);
        assert_eq!(result.len(), 2);
        assert!(result[0].contains('-'));
        assert!(result[1].contains('+'));
        // Intra-line diff should have inverse markers
        assert!(
            result[0].contains("\x1b[7m"),
            "should have inverse on removed"
        );
        assert!(
            result[1].contains("\x1b[7m"),
            "should have inverse on added"
        );
    }

    #[test]
    fn test_multi_line_removes() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = test_theme();
        let diff = "-a\n-b\n+c\n";
        let result = render_diff(diff, &theme);
        // Two removed lines, then one added
        assert_eq!(result.len(), 3);
        assert!(result[0].contains("-a"));
        assert!(result[1].contains("-b"));
        assert!(result[2].contains("+c"));
    }

    #[test]
    fn test_multi_line_removes_no_intra_diff() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = test_theme();
        let diff = "-aaa\n-bbb\n+ccc\n+ddd\n";
        let result = render_diff(diff, &theme);
        assert_eq!(result.len(), 4);
        // No intra-line diff for multi-line changes - no inverse markers
        assert!(
            !result[0].contains("\x1b[7m"),
            "no inverse on multi-line remove"
        );
    }

    #[test]
    fn test_compute_word_diff_basic() {
        let changes = compute_word_diff("abc", "abd");
        assert!(!changes.is_empty());
    }

    #[test]
    fn test_compute_word_diff_identical() {
        let changes = compute_word_diff("hello", "hello");
        assert_eq!(changes.len(), 1);
        assert!(matches!(changes[0], Change::Equal(_)));
    }

    #[test]
    fn test_tabs_replaced() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = test_theme();
        let diff = "-\tindented\n";
        let result = render_diff(diff, &theme);
        assert_eq!(result.len(), 1);
        assert!(!result[0].contains('\t'), "tabs should be replaced");
    }

    #[test]
    fn test_context_line_format() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = test_theme();
        let diff = " context\n";
        let result = render_diff(diff, &theme);
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("context"));
        assert!(result[0].starts_with("\x1b"));
    }
}
