use crate::tui::util::visible_width;

/// Count how many visual (wrapped) lines a single logical line occupies
/// at the given terminal width. Accounts for zero-width edge case.
pub fn visual_line_count(line: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    let vis = visible_width(line);
    if vis == 0 {
        return 1;
    }
    vis.div_ceil(width)
}

/// Select the last `max_visual_lines` visual lines from a list of logical lines.
///
/// Walks backwards from the end, counting how many visual rows each logical
/// line occupies. Stops when the budget is exhausted or the first line is reached.
///
/// Returns `(selected_logical_lines, hidden_logical_line_count)`.
///
/// This matches pi's `truncateToVisualLines` which accounts for terminal wrapping.
pub fn truncate_to_visual_lines<'a>(
    lines: &'a [&'a str],
    width: usize,
    max_visual_lines: usize,
) -> (Vec<&'a str>, usize) {
    if lines.is_empty() || max_visual_lines == 0 {
        return (vec![], 0);
    }

    // Compute visual line count per logical line
    let visual_counts: Vec<usize> = lines.iter().map(|l| visual_line_count(l, width)).collect();

    let total_visual: usize = visual_counts.iter().sum();

    // Everything fits — no truncation
    if total_visual <= max_visual_lines {
        return (lines.to_vec(), 0);
    }

    // Walk backwards from the end, consuming visual lines
    let mut budget = max_visual_lines;
    let mut start = lines.len();

    for (i, &vc) in visual_counts.iter().enumerate().rev() {
        if vc > budget {
            // This line alone exceeds the remaining budget — stop here.
            // We can't split a logical line, so this line is excluded.
            break;
        }
        budget -= vc;
        start = i;
    }

    (lines[start..].to_vec(), start)
}

/// Format a hint about hidden lines for display (matching pi's format).
/// Returns e.g. `"... (12 earlier lines, ctrl+o to expand)"`.
pub fn format_hidden_hint(hidden: usize, expand_key: &str) -> String {
    if expand_key.is_empty() {
        format!("... {} earlier lines", hidden)
    } else {
        format!("... ({} earlier lines, {} to expand)", hidden, expand_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visual_line_count_ascii() {
        assert_eq!(visual_line_count("hello", 80), 1);
        assert_eq!(visual_line_count("", 80), 1);
    }

    #[test]
    fn test_visual_line_count_wrapping() {
        assert_eq!(visual_line_count(&"a".repeat(100), 80), 2);
        assert_eq!(visual_line_count(&"a".repeat(160), 80), 2);
        assert_eq!(visual_line_count(&"a".repeat(161), 80), 3);
    }

    #[test]
    fn test_visual_line_count_zero_width() {
        assert_eq!(visual_line_count("hello", 0), 1);
    }

    #[test]
    fn test_truncate_to_visual_lines_no_truncation() {
        let lines = vec!["short", "also short"];
        let (selected, hidden) = truncate_to_visual_lines(&lines, 80, 10);
        assert_eq!(selected.len(), 2);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn test_truncate_to_visual_lines_with_wrapping() {
        let line1 = "a".repeat(100);
        let line2 = "b".repeat(100);
        let line3 = "c".repeat(100);
        let lines = vec![line1.as_str(), line2.as_str(), line3.as_str()];

        // 3 lines × 2 visual = 6 total. Request 4 → show last 2 logical lines.
        let (selected, hidden) = truncate_to_visual_lines(&lines, 80, 4);
        assert_eq!(selected.len(), 2);
        assert_eq!(hidden, 1);
        assert_eq!(selected[0], line2.as_str());
        assert_eq!(selected[1], line3.as_str());
    }

    #[test]
    fn test_truncate_to_visual_lines_exact_fit() {
        let line1 = "a".repeat(100);
        let line2 = "b".repeat(100);
        let lines = vec![line1.as_str(), line2.as_str()];
        let (selected, hidden) = truncate_to_visual_lines(&lines, 80, 4);
        assert_eq!(selected.len(), 2);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn test_truncate_to_visual_lines_empty() {
        let lines: Vec<&str> = vec![];
        let (selected, hidden) = truncate_to_visual_lines(&lines, 80, 5);
        assert!(selected.is_empty());
        assert_eq!(hidden, 0);
    }

    #[test]
    fn test_truncate_to_visual_lines_mixed_widths() {
        let short1 = "short";
        let long = "x".repeat(100);
        let short2 = "also short";
        let lines = vec![short1, long.as_str(), short2];

        let (selected, hidden) = truncate_to_visual_lines(&lines, 80, 3);
        assert_eq!(selected.len(), 2);
        assert_eq!(hidden, 1);
        assert_eq!(selected[0], long.as_str());
        assert_eq!(selected[1], short2);
    }

    #[test]
    fn test_format_hidden_hint() {
        let hint = format_hidden_hint(12, "C-O");
        assert!(hint.contains("12"));
        assert!(hint.contains("C-O"));

        let hint = format_hidden_hint(5, "");
        assert!(hint.contains("5"));
        assert!(!hint.contains("to expand"));
    }
}
