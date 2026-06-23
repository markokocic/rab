use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::current_theme;

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
pub fn render_diff(diff_text: &str) -> Vec<String> {
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
                    // Multiple removed lines — push previous and start new
                    if let Some(prev) = prev_removed.take() {
                        let styled = color_line(&prev, "toolDiffRemoved");
                        lines.push(styled);
                    }
                }
                prev_removed = Some(line.to_string());
            }
            "+" => {
                if let Some(ref removed_full) = prev_removed.take() {
                    let removed_content = &removed_full[1..]; // strip '-'
                    // Intra-line diff: single removed + single added
                    render_intra_line_diff(removed_content, content, &mut lines);
                } else {
                    // Standalone added line (multiple adds after removes)
                    let styled = color_line(line, "toolDiffAdded");
                    lines.push(styled);
                }
            }
            _ => {
                prev_removed = None;
                let styled = color_line(line, "toolDiffContext");
                lines.push(styled);
            }
        }
    }

    // Flush remaining removed line
    if let Some(prev) = prev_removed.take() {
        let styled = color_line(&prev, "toolDiffRemoved");
        lines.push(styled);
    }

    lines
}

/// Color a single diff line with the given theme color.
fn color_line(line: &str, color: &str) -> String {
    let theme = current_theme();
    let ansi = theme.fg_ansi(color).to_string();
    drop(theme);
    format!("{}{}\x1b[39m", ansi, line)
}

/// Render intra-line diff for a single-line change (one removed, one added).
/// Uses character-level diff and applies inverse (reverse video) on changed parts.
///
/// Matches pi's `renderIntraLineDiff()` which uses `diffWords` to find changed
/// tokens and applies `theme.inverse()` on them.
fn render_intra_line_diff(old: &str, new: &str, output: &mut Vec<String>) {
    let changes: Vec<Change> = compute_word_diff(old, new);

    let theme = current_theme();
    let added_ansi = theme.fg_ansi_key(ThemeKey::ToolDiffAdded).to_string();
    let removed_ansi = theme.fg_ansi_key(ThemeKey::ToolDiffRemoved).to_string();
    let inverse_on = "\x1b[7m"; // reverse video
    let inverse_off = "\x1b[27m"; // reverse video off
    let reset = "\x1b[39m";
    drop(theme);

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

/// Compute a character-level diff between two strings.
/// Groups consecutive same-type changes into tokens (matching pi's diffWords style).
fn compute_word_diff(old: &str, new: &str) -> Vec<Change> {
    let changeset = diff::chars(old, new);

    let mut merged: Vec<Change> = Vec::new();
    for change in &changeset {
        let (tag, ch) = match change {
            diff::Result::Left(c) => ("-", *c),
            diff::Result::Right(c) => ("+", *c),
            diff::Result::Both(c, _) => ("=", *c),
        };

        // Try to merge with the last change
        if let Some(last) = merged.last_mut() {
            let last_tag = match last {
                Change::Equal(_) => "=",
                Change::Removed(_) => "-",
                Change::Added(_) => "+",
            };
            if last_tag == tag {
                // Merge character into last change
                match last {
                    Change::Equal(t) => t.push(ch),
                    Change::Removed(t) => t.push(ch),
                    Change::Added(t) => t.push(ch),
                }
                continue;
            }
        }

        // New change group
        let change = match tag {
            "=" => Change::Equal(ch.to_string()),
            "-" => Change::Removed(ch.to_string()),
            "+" => Change::Added(ch.to_string()),
            _ => unreachable!(),
        };
        merged.push(change);
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_empty_diff() {
        let result = render_diff("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_skips_headers() {
        let diff = "--- a/file.rs\n+++ b/file.rs\n@@ -1,3 +1,4 @@\n";
        let result = render_diff(diff);
        assert!(result.is_empty(), "should skip all headers");
    }

    #[test]
    fn test_context_lines() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let diff = " line1\n line2\n";
        let result = render_diff(diff);
        assert_eq!(result.len(), 2);
        assert!(result[0].contains("line1"));
        assert!(result[0].starts_with("\x1b")); // has ANSI color
        assert!(result[0].contains("\x1b[39m")); // has reset
    }

    #[test]
    fn test_removed_line() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let diff = "-old_line\n";
        let result = render_diff(diff);
        assert_eq!(result.len(), 1);
        assert!(result[0].contains('-')); // prefix preserved
        assert!(result[0].contains("old_line"));
    }

    #[test]
    fn test_added_line() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let diff = "+new_line\n";
        let result = render_diff(diff);
        assert_eq!(result.len(), 1);
        assert!(result[0].contains('+'));
        assert!(result[0].contains("new_line"));
    }

    #[test]
    fn test_single_line_modification() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let diff = "-foo\n+bar\n";
        let result = render_diff(diff);
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
        let diff = "-a\n-b\n+c\n";
        let result = render_diff(diff);
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
