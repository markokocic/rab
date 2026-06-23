use crate::agent::ui::theme::ThemeKey;
use crate::tui::Theme;

/// Render a unified diff string with colored lines and intra-line change highlighting.
/// Matches pi's `renderDiff()` in `diff.ts`.
///
/// Input format (from edit tool's compute_diff):
/// `--- a/path` / `+++ b/path` / `@@ -1,5 +1,6 @@` / ` context` / `-removed` / `+added`
///
/// Output: ANSI-styled lines with:
/// - `-` lines: `toolDiffRemoved` (red)
/// - `+` lines: `toolDiffAdded` (green)
/// - ` ` lines: `toolDiffContext` (gray)
/// - Single-line changes: intra-line diff with inverse highlighting
///
/// Takes a `&dyn Theme` parameter to avoid calling `current_theme()` which
/// would deadlock if the theme lock is already held by a caller.
pub fn render_diff(diff_text: &str, theme: &dyn Theme) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut prev_removed: Option<String> = None;

    for line in diff_text.lines() {
        // Skip unified diff headers
        if line.starts_with("---") || line.starts_with("+++") || line.starts_with("@@") {
            prev_removed = None;
            continue;
        }

        if line.is_empty() {
            prev_removed = None;
            continue;
        }

        let (prefix, content) = line.split_at(1);
        let content = content.trim_end_matches('\r');

        match prefix {
            "-" => {
                if prev_removed.is_some() {
                    // Multiple removed lines - push previous and start new
                    if let Some(prev) = prev_removed.take() {
                        let styled = color_line(&prev, "toolDiffRemoved", theme);
                        lines.push(styled);
                    }
                }
                prev_removed = Some(line.to_string());
            }
            "+" => {
                if let Some(ref removed_full) = prev_removed.take() {
                    let removed_content = &removed_full[1..]; // strip '-'
                    // Intra-line diff: single removed + single added
                    render_intra_line_diff(removed_content, content, &mut lines, theme);
                } else {
                    // Standalone added line (multiple adds after removes)
                    let styled = color_line(line, "toolDiffAdded", theme);
                    lines.push(styled);
                }
            }
            _ => {
                prev_removed = None;
                let styled = color_line(line, "toolDiffContext", theme);
                lines.push(styled);
            }
        }
    }

    // Flush remaining removed line
    if let Some(prev) = prev_removed.take() {
        let styled = color_line(&prev, "toolDiffRemoved", theme);
        lines.push(styled);
    }

    lines
}

/// Color a single diff line with the given theme color.
fn color_line(line: &str, color: &str, theme: &dyn Theme) -> String {
    let ansi = theme.fg_ansi(color).to_string();
    format!("{}{}\x1b[39m", ansi, line)
}

/// Render intra-line diff for a single-line change (one removed, one added).
/// Uses character-level diff and applies inverse (reverse video) on changed parts.
///
/// Matches pi's `renderIntraLineDiff()` which uses `diffWords` to find changed
/// tokens and applies `theme.inverse()` on them.
fn render_intra_line_diff(old: &str, new: &str, output: &mut Vec<String>, theme: &dyn Theme) {
    let changes: Vec<Change> = compute_word_diff(old, new);
    let added_ansi = theme.fg_ansi_key(ThemeKey::ToolDiffAdded).to_string();
    let removed_ansi = theme.fg_ansi_key(ThemeKey::ToolDiffRemoved).to_string();
    let inverse_on = "\x1b[7m"; // reverse video
    let inverse_off = "\x1b[27m"; // reverse video off
    let reset = "\x1b[39m";

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
                removed_line.push_str(&format!("{}{}{}", inverse_on, trimmed, inverse_off));
            }
            Change::Added(text) => {
                // Strip leading whitespace
                let trimmed = text.trim_start();
                if trimmed.len() < text.len() {
                    let ws = &text[..text.len() - trimmed.len()];
                    added_line.push_str(ws);
                }
                added_line.push_str(&format!("{}{}{}", inverse_on, trimmed, inverse_off));
            }
        }
    }

    output.push(format!("-{}{}{}", removed_ansi, removed_line, reset));
    output.push(format!("+{}{}{}", added_ansi, added_line, reset));
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
        assert!(result[0].contains('-')); // prefix preserved
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
        assert!(result.len() >= 2);
        // The first two should be - lines
        assert!(result[0].contains("-a") || result[0].contains("-a"));
        // The last should be a + line or intra-line diff
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
}
