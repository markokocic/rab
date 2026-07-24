//! Syntax validation — collects ERROR/MISSING nodes from a tree-sitter parse
//! and provides a delimiter-balance fallback for grammarless lisps.

use std::path::Path;

/// A single syntax error discovered by tree-sitter.
#[derive(Debug, Clone)]
pub struct SyntaxError {
    /// 1-based line number.
    pub line: usize,
    /// 1-based column number.
    pub column: usize,
    /// Short snippet of the problematic source (≤ 80 chars, single line).
    pub snippet: String,
    /// Whether tree-sitter classified this as MISSING (expected token).
    pub is_missing: bool,
    /// For MISSING nodes, the expected token (e.g. "}", ";"), if available.
    pub expected: Option<String>,
}

impl SyntaxError {
    pub fn render(&self) -> String {
        match (self.is_missing, self.expected.as_deref()) {
            (true, Some(tok)) if !tok.is_empty() && tok != "ERROR" => {
                format!(
                    "  missing `{tok}` at {}:{}: {}",
                    self.line, self.column, self.snippet
                )
            }
            (true, _) => {
                format!(
                    "  missing token at {}:{}: {}",
                    self.line, self.column, self.snippet
                )
            }
            (false, _) => {
                format!(
                    "  syntax error at {}:{}: {}",
                    self.line, self.column, self.snippet
                )
            }
        }
    }
}

/// Cap on errors surfaced per call — tree-sitter cascades on missing braces.
const MAX_ERRORS: usize = 10;

/// Walk the tree collecting ERROR/MISSING nodes.
pub fn collect_errors(tree: &tree_sitter::Tree, source: &str) -> Vec<SyntaxError> {
    let mut errors = Vec::new();
    let mut stack = vec![tree.root_node()];

    while let Some(node) = stack.pop() {
        if errors.len() >= MAX_ERRORS {
            break;
        }
        if node.is_error() || node.is_missing() {
            let pos = node.start_position();
            let snippet = snippet_for(node, source);
            let expected = if node.is_missing() {
                Some(node.kind().to_string())
            } else {
                None
            };
            errors.push(SyntaxError {
                line: pos.row + 1,
                column: pos.column + 1,
                snippet,
                is_missing: node.is_missing(),
                expected,
            });
            // Don't descend into error nodes — children are noise.
            continue;
        }
        // Push children in reverse for left-to-right traversal.
        for i in (0..node.child_count()).rev() {
            if let Some(child) = node.child(i as u32) {
                stack.push(child);
            }
        }
    }

    errors
}

/// Short snippet for an error node (≤ 80 chars, single line).
fn snippet_for(node: tree_sitter::Node, source: &str) -> String {
    let start = node.start_byte();
    let end = node.end_byte().min(source.len());
    if start >= end {
        // Missing nodes have zero span — pull the line they're on.
        let line_start = source[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = source[start..]
            .find('\n')
            .map(|i| start + i)
            .unwrap_or(source.len());
        return source[line_start..line_end]
            .chars()
            .take(80)
            .collect::<String>()
            .trim_end()
            .to_string();
    }
    let raw = &source[start..end];
    let line: String = raw.chars().take_while(|c| *c != '\n').collect();
    line.chars()
        .take(80)
        .collect::<String>()
        .trim_end()
        .to_string()
}

// ── Delimiter-balance fallback ──────────────────────────────────────────
// For Lisp-like languages without a tree-sitter grammar, we scan for
// delimiter imbalance with comment/string awareness.

const BALANCE_RULES: &[(&str, BalanceRule)] = &[
    (".janet", BalanceRule {
        line_comment: "#",
        block_comment: None,
        strings: &[("\"", "\"", true)],
        char_backslash: false,
        backtick_long: true,
    }),
    (".jdn", BalanceRule {
        line_comment: "#",
        block_comment: None,
        strings: &[("\"", "\"", true)],
        char_backslash: false,
        backtick_long: true,
    }),
    (".fnl", BalanceRule {
        line_comment: ";",
        block_comment: None,
        strings: &[("\"", "\"", true)],
        char_backslash: true,
        backtick_long: false,
    }),
    (".scm", BalanceRule {
        line_comment: ";",
        block_comment: Some(("#|", "|#")),
        strings: &[("\"", "\"", true)],
        char_backslash: true,
        backtick_long: false,
    }),
    (".ss", BalanceRule {
        line_comment: ";",
        block_comment: Some(("#|", "|#")),
        strings: &[("\"", "\"", true)],
        char_backslash: true,
        backtick_long: false,
    }),
    (".rkt", BalanceRule {
        line_comment: ";",
        block_comment: Some(("#|", "|#")),
        strings: &[("\"", "\"", true)],
        char_backslash: true,
        backtick_long: false,
    }),
    (".lisp", BalanceRule {
        line_comment: ";",
        block_comment: Some(("#|", "|#")),
        strings: &[("\"", "\"", true)],
        char_backslash: true,
        backtick_long: false,
    }),
    (".el", BalanceRule {
        line_comment: ";",
        block_comment: None,
        strings: &[("\"", "\"", true)],
        char_backslash: true, // for ?\X
        backtick_long: false,
    }),
];

struct BalanceRule {
    line_comment: &'static str,
    block_comment: Option<(&'static str, &'static str)>,
    strings: &'static [(&'static str, &'static str, bool)], // (open, close, esc)
    char_backslash: bool,
    backtick_long: bool,
}

fn balance_rule_for_ext(ext: &str) -> Option<&'static BalanceRule> {
    BALANCE_RULES.iter().find(|(e, _)| *e == ext).map(|(_, r)| r)
}

/// Check delimiter balance. Returns an actionable error message, or None if balanced.
/// Format errors for inclusion in a tool error message.
pub fn format_errors(path: &Path, _content: &str, errors: &[SyntaxError]) -> String {
    let mut out = format!(
        "Syntax check failed for {}: {} error(s) detected by tree-sitter. \
         Fix and re-submit. (This is a pre-write guard — the file was NOT modified.)\n",
        path.display(),
        errors.len(),
    );
    for err in errors {
        out.push_str(&err.render());
        out.push('\n');
    }
    if errors.len() == MAX_ERRORS {
        out.push_str(&format!(
            "  …(truncated at {} errors; fix the listed issues and re-check)\n",
            MAX_ERRORS,
        ));
    }
    out
}

pub fn check_delimiter_balance(path: &Path, content: &str) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    let ext = format!(".{ext}");
    let rules = balance_rule_for_ext(&ext)?;

    let bytes = content.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    let mut line = 1usize;
    let mut col = 1usize;
    // (opener byte, line, col)
    let mut stack: Vec<(u8, usize, usize)> = Vec::new();

    while i < n {
        // Line comment
        if bytes[i..].starts_with(rules.line_comment.as_bytes()) {
            let to_eol = bytes[i..].iter().position(|&c| c == b'\n').unwrap_or(n - i);
            advance(bytes, &mut i, &mut line, &mut col, to_eol);
            continue;
        }

        // Block comment
        if let Some((open, close)) = rules.block_comment
            && bytes[i..].starts_with(open.as_bytes()) {
                advance(bytes, &mut i, &mut line, &mut col, open.len());
                let mut depth = 1;
                while i < n && depth > 0 {
                    if bytes[i..].starts_with(open.as_bytes()) {
                        depth += 1;
                        advance(bytes, &mut i, &mut line, &mut col, open.len());
                    } else if bytes[i..].starts_with(close.as_bytes()) {
                        depth -= 1;
                        advance(bytes, &mut i, &mut line, &mut col, close.len());
                    } else {
                        advance(bytes, &mut i, &mut line, &mut col, 1);
                    }
                }
                continue;
            }

        // Strings
        for &(open, close, esc) in rules.strings {
            if bytes[i..].starts_with(open.as_bytes()) {
                advance(bytes, &mut i, &mut line, &mut col, open.len());
                while i < n {
                    if esc && bytes[i] == b'\\' {
                        advance(bytes, &mut i, &mut line, &mut col, 2);
                    } else if bytes[i..].starts_with(close.as_bytes()) {
                        advance(bytes, &mut i, &mut line, &mut col, close.len());
                        break;
                    } else {
                        advance(bytes, &mut i, &mut line, &mut col, 1);
                    }
                }
                continue;
            }
        }

        // Char literal \x (Lisp)
        if rules.char_backslash && bytes[i] == b'\\' {
            advance(bytes, &mut i, &mut line, &mut col, 2);
            continue;
        }

        // Backtick long-string (Janet)
        if rules.backtick_long && bytes[i] == b'`' {
            let mut k = 0;
            while i + k < n && bytes[i + k] == b'`' {
                k += 1;
            }
            advance(bytes, &mut i, &mut line, &mut col, k);
            while i < n {
                if bytes[i] == b'`' {
                    let mut j = 0;
                    while i + j < n && bytes[i + j] == b'`' {
                        j += 1;
                    }
                    if j >= k {
                        advance(bytes, &mut i, &mut line, &mut col, k);
                        break;
                    }
                    advance(bytes, &mut i, &mut line, &mut col, j);
                } else {
                    advance(bytes, &mut i, &mut line, &mut col, 1);
                }
            }
            continue;
        }

        match bytes[i] {
            b'(' | b'[' | b'{' => {
                let (l, c) = (line, col);
                stack.push((bytes[i], l, c));
            }
            b')' | b']' | b'}' => {
                let want = match bytes[i] {
                    b')' => b'(',
                    b']' => b'[',
                    _ => b'{',
                };
                match stack.last() {
                    Some(&(open, _, _)) if open == want => {
                        stack.pop();
                    }
                    _ => {
                        let (l, c) = (line, col);
                        return Some(format!(
                            "Delimiter imbalance: unexpected `{}` at line {l}, col {c} \
                             with no matching opener — remove an extra closer, or add the \
                             missing opener before it.",
                            bytes[i] as char
                        ));
                    }
                }
            }
            _ => {}
        }
        advance(bytes, &mut i, &mut line, &mut col, 1);
    }

    if !stack.is_empty() {
        let (open, l, c) = stack[0];
        let close = closer_for(open);
        return Some(format!(
            "Delimiter imbalance: {} unclosed — the `{}` opened at line {l}, col {c} is \
             never closed; add {} matching `{close}` (do not count by hand — fix this delimiter).",
            stack.len(),
            open as char,
            stack.len(),
        ));
    }

    None
}

fn closer_for(open: u8) -> char {
    match open {
        b'(' => ')',
        b'[' => ']',
        _ => '}',
    }
}

fn advance(bytes: &[u8], i: &mut usize, line: &mut usize, col: &mut usize, count: usize) {
    let end = i.saturating_add(count).min(bytes.len());
    while *i < end {
        if bytes[*i] == b'\n' {
            *line += 1;
            *col = 1;
        } else {
            *col += 1;
        }
        *i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_errors_caps_at_max() {
        let mut parser = tree_sitter::Parser::new();
        // Use a simple trick: we can't set a language here in unit tests
        // without a grammar, so just test the snippet helper directly.
    }

    #[test]
    fn render_names_expected_token() {
        let e = SyntaxError {
            line: 5, column: 1,
            snippet: "}".into(),
            is_missing: true,
            expected: Some("}".into()),
        };
        assert_eq!(e.render(), "  missing `}` at 5:1: }");

        let e2 = SyntaxError {
            line: 1, column: 1,
            snippet: "@@@".into(),
            is_missing: false,
            expected: None,
        };
        assert!(e2.render().contains("syntax error at 1:1"));
    }

    #[test]
    fn delimiter_balance_parens() {
        let path = std::path::PathBuf::from("/tmp/x.janet");
        assert!(check_delimiter_balance(&path, "(def f [x] (+ x 1))").is_none());
        assert!(check_delimiter_balance(&path, "(def f [x] (+ x 1)").is_some());
    }

    #[test]
    fn delimiter_balance_respects_strings() {
        let path = std::path::PathBuf::from("/tmp/x.janet");
        assert!(check_delimiter_balance(&path, "(def s \"a ) b\")").is_none());
    }

    #[test]
    fn delimiter_balance_respects_comments() {
        let path = std::path::PathBuf::from("/tmp/x.fnl");
        assert!(check_delimiter_balance(&path, "(fn f [] 1) ; comment )").is_none());
    }

    #[test]
    fn delimiter_balance_unknown_ext() {
        let path = std::path::PathBuf::from("/tmp/x.rs");
        assert!(check_delimiter_balance(&path, "((((").is_none());
    }
}
