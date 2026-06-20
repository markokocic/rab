use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use anyhow::Context;
use async_trait::async_trait;
use std::borrow::Cow;

pub struct EditExtension {
    cwd: std::path::PathBuf,
}

impl EditExtension {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for EditExtension {
    fn name(&self) -> Cow<'static, str> {
        "edit".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(EditTool {
            cwd: self.cwd.clone(),
        })]
    }
}

struct EditTool {
    cwd: std::path::PathBuf,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Edit {
    old_text: String,
    new_text: String,
}

// ── BOM handling ──────────────────────────────────────────────────

/// Strip UTF-8 BOM if present. Returns (bom, content_without_bom).
fn strip_bom(content: &str) -> (&str, &str) {
    if content.starts_with('\u{FEFF}') {
        ("\u{FEFF}", &content['\u{FEFF}'.len_utf8()..])
    } else {
        ("", content)
    }
}

// ── Line ending handling ─────────────────────────────────────────

fn detect_line_ending(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn normalize_to_lf(content: &str) -> String {
    content.replace("\r\n", "\n")
}

fn restore_line_endings(content: &str, ending: &str) -> String {
    if ending == "\r\n" {
        content.replace('\n', "\r\n")
    } else {
        content.to_string()
    }
}

// ── Fuzzy matching ───────────────────────────────────────────────

/// Normalize text for fuzzy matching:
/// - Strip trailing whitespace from each line
/// - Normalize Unicode smart quotes → ASCII quotes
/// - Normalize Unicode dashes/hyphens → ASCII hyphen
/// - Normalize special Unicode spaces → regular space
fn normalize_for_fuzzy_match(text: &str) -> String {
    // First pass: strip trailing whitespace per line
    let mut intermediate = String::with_capacity(text.len());
    for line in text.lines() {
        if !intermediate.is_empty() {
            intermediate.push('\n');
        }
        intermediate.push_str(line.trim_end());
    }
    // Handle trailing newline: lines() strips final newline, re-add if present
    if text.ends_with('\n') {
        intermediate.push('\n');
    }

    // Second pass: normalize Unicode characters to ASCII equivalents
    let mut result = String::with_capacity(intermediate.len());
    for ch in intermediate.chars() {
        match ch {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => result.push('\''),
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => result.push('"'),
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}' | '\u{2212}' => {
                result.push('-');
            }
            '\u{00A0}'
            | '\u{2002}'
            | '\u{2003}'
            | '\u{2004}'
            | '\u{2005}'
            | '\u{2006}'
            | '\u{2007}'
            | '\u{2008}'
            | '\u{2009}'
            | '\u{200A}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}' => {
                result.push(' ');
            }
            other => result.push(other),
        }
    }

    result
}

// ── Input normalization ──────────────────────────────────────────

/// Normalize tool arguments: handle `edits` as JSON string, legacy `oldText`/`newText`.
fn prepare_edit_arguments(args: &serde_json::Value) -> Result<(String, Vec<Edit>), String> {
    let path = args["path"]
        .as_str()
        .ok_or_else(|| "Missing 'path' argument".to_string())?;

    let edits = if let Some(edits_val) = args.get("edits") {
        if let Some(s) = edits_val.as_str() {
            // Some models send edits as a JSON string instead of an array
            serde_json::from_str::<Vec<Edit>>(s)
                .map_err(|e| format!("Invalid edits JSON string: {}", e))?
        } else {
            serde_json::from_value::<Vec<Edit>>(edits_val.clone())
                .map_err(|e| format!("Invalid edits array: {}", e))?
        }
    } else if let (Some(old), Some(new)) = (args.get("oldText"), args.get("newText")) {
        // Legacy: oldText + newText at top level
        let old_text = old
            .as_str()
            .ok_or_else(|| "Invalid 'oldText' argument: expected string".to_string())?;
        let new_text = new
            .as_str()
            .ok_or_else(|| "Invalid 'newText' argument: expected string".to_string())?;
        vec![Edit {
            old_text: old_text.to_string(),
            new_text: new_text.to_string(),
        }]
    } else {
        return Err(
            "Missing 'edits' array (or 'oldText'/'newText' for legacy format)".to_string(),
        );
    };

    if edits.is_empty() {
        return Err("At least one edit is required".to_string());
    }

    Ok((path.to_string(), edits))
}

// ── Diff computation ─────────────────────────────────────────────

/// Compute a simple unified diff between original and modified content.
fn compute_diff(original: &str, modified: &str, path: &str) -> String {
    let orig_lines: Vec<&str> = original.lines().collect();
    let mod_lines: Vec<&str> = modified.lines().collect();

    let mut diff = String::new();
    diff.push_str("--- a/");
    diff.push_str(path);
    diff.push('\n');
    diff.push_str("+++ b/");
    diff.push_str(path);
    diff.push('\n');

    let mut i = 0;
    let mut j = 0;
    let mut hunk: Vec<(char, &str)> = Vec::new();
    let mut hunk_start_orig = 0;
    let mut hunk_start_mod = 0;

    while i < orig_lines.len() || j < mod_lines.len() {
        let same = i < orig_lines.len() && j < mod_lines.len() && orig_lines[i] == mod_lines[j];

        if same {
            if !hunk.is_empty() && hunk.len() >= 3 {
                // Emit context line within hunk
                hunk.push((' ', orig_lines[i]));
            } else {
                // Flush current hunk
                if !hunk.is_empty() {
                    flush_hunk(&mut diff, &mut hunk, hunk_start_orig, hunk_start_mod);
                }
                hunk_start_orig = i + 1;
                hunk_start_mod = j + 1;
            }
            i += 1;
            j += 1;
        } else {
            if hunk.is_empty() {
                hunk_start_orig = i;
                hunk_start_mod = j;
            }
            if i < orig_lines.len() {
                hunk.push(('-', orig_lines[i]));
                i += 1;
            }
            if j < mod_lines.len() {
                hunk.push(('+', mod_lines[j]));
                j += 1;
            }
        }
    }

    if !hunk.is_empty() {
        flush_hunk(&mut diff, &mut hunk, hunk_start_orig, hunk_start_mod);
    }

    diff
}

fn flush_hunk(diff: &mut String, hunk: &mut Vec<(char, &str)>, orig_start: usize, mod_start: usize) {
    let orig_count = hunk.iter().filter(|(c, _)| *c == '-' || *c == ' ').count();
    let mod_count = hunk.iter().filter(|(c, _)| *c == '+' || *c == ' ').count();
    use std::fmt::Write;
    let _ = writeln!(
        diff,
        "@@ -{},{} +{},{} @@",
        orig_start + 1,
        orig_count,
        mod_start + 1,
        mod_count
    );
    for (c, line) in hunk.drain(..) {
        let _ = writeln!(diff, "{}{}", c, line);
    }
}

// ── AgentTool implementation ─────────────────────────────────────

#[async_trait]
impl AgentTool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a single file using exact text replacement. Every edits[].oldText must match a \
         unique, non-overlapping region of the original file. If two changes affect the same \
         block or nearby lines, merge them into one edit instead of emitting overlapping edits. \
         Do not include large unchanged regions just to connect distant changes."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "edits"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative or absolute)"
                },
                "edits": {
                    "type": "array",
                    "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.",
                    "items": {
                        "type": "object",
                        "required": ["oldText", "newText"],
                        "properties": {
                            "oldText": {
                                "type": "string",
                                "description": "Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."
                            },
                            "newText": {
                                "type": "string",
                                "description": "Replacement text for this targeted edit."
                            }
                        }
                    }
                }
            }
        })
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        vec![
            "Use edit for precise changes (edits[].oldText must match exactly)".into(),
            "When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls".into(),
            "Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.".into(),
            "Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.".into(),
        ]
    }

    fn label(&self) -> &str {
        "Make precise file edits"
    }

    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
        _cancel: Cancel,
    ) -> anyhow::Result<ToolOutput> {
        let _ = tool_call_id;
        let (path_str, edits) =
            prepare_edit_arguments(&args).map_err(|e| anyhow::anyhow!("{}", e))?;

        let abs_path = {
            let p = std::path::Path::new(&path_str);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                self.cwd.join(p)
            }
        };

        // Read file
        let raw_content = std::fs::read_to_string(&abs_path)
            .with_context(|| format!("Failed to read {}", abs_path.display()))?;

        // ── 1. BOM handling ──
        let (bom, content) = strip_bom(&raw_content);

        // ── 2. Line ending handling ──
        let original_ending = detect_line_ending(content);
        let normalized = normalize_to_lf(content);

        // ── 3. Work in fuzzy-normalized space ──
        // This strips trailing whitespace and normalizes Unicode quotes/dashes,
        // so we work in a space where fuzzy matching doesn't cause coordinate issues.
        let work_content = normalize_for_fuzzy_match(&normalized);

        // ── 4. Validate and find each edit ──
        let mut matched_edits: Vec<(usize, usize, &Edit)> = Vec::new();

        for (i, edit) in edits.iter().enumerate() {
            if edit.old_text.is_empty() {
                return if edits.len() == 1 {
                    Err(anyhow::anyhow!("oldText must not be empty in {}.", path_str))
                } else {
                    Err(anyhow::anyhow!(
                        "edits[{}].oldText must not be empty in {}.",
                        i,
                        path_str
                    ))
                };
            }

            // Normalize oldText to the same fuzzy space for counting
            let fuzzy_old = normalize_for_fuzzy_match(&edit.old_text);
            let count = work_content.matches(&fuzzy_old).count();

            if count == 0 {
                return if edits.len() == 1 {
                    Err(anyhow::anyhow!(
                        "Could not find the exact text in {}. \
                         The old text must match exactly including all whitespace and newlines.",
                        path_str
                    ))
                } else {
                    Err(anyhow::anyhow!(
                        "Could not find edits[{}] in {}. \
                         The oldText must match exactly including all whitespace and newlines.",
                        i,
                        path_str
                    ))
                };
            }

            if count > 1 {
                return if edits.len() == 1 {
                    Err(anyhow::anyhow!(
                        "Found {} occurrences of the text in {}. \
                         The text must be unique. Please provide more context to make it unique.",
                        count,
                        path_str
                    ))
                } else {
                    Err(anyhow::anyhow!(
                        "Found {} occurrences of edits[{}] in {}. \
                         Each oldText must be unique. Please provide more context to make it unique.",
                        count,
                        i,
                        path_str
                    ))
                };
            }

            // Unique match found — locate it
            let pos = work_content.find(&fuzzy_old).unwrap();
            matched_edits.push((pos, pos + fuzzy_old.len(), edit));
        }

        // ── 5. Check for overlapping edits ──
        for (idx_i, &(pos_i, end_i, _)) in matched_edits.iter().enumerate() {
            for (idx_j, &(pos_j, end_j, _)) in matched_edits.iter().enumerate().skip(idx_i + 1) {
                if pos_i < end_j && pos_j < end_i {
                    return Err(anyhow::anyhow!(
                        "edits[{}] and edits[{}] overlap. Merge them into one edit.",
                        idx_i,
                        idx_j
                    ));
                }
            }
        }

        // ── 6. Apply edits (sorted left-to-right) ──
        matched_edits.sort_by_key(|(pos, _, _)| *pos);

        let mut result = String::new();
        let mut cursor = 0;
        for (start, end, edit) in &matched_edits {
            result.push_str(&work_content[cursor..*start]);
            result.push_str(&edit.new_text);
            cursor = *end;
        }
        result.push_str(&work_content[cursor..]);

        // ── 7. Compute diff ──
        let diff = compute_diff(&normalized, &result, &path_str);

        // ── 8. Write back with original line endings and BOM ──
        let final_content =
            bom.to_string() + &restore_line_endings(&result, original_ending);
        std::fs::write(&abs_path, &final_content)
            .with_context(|| format!("Failed to write {}", abs_path.display()))?;

        // ── 9. Return result ──
        let noun = if edits.len() == 1 { "block" } else { "blocks" };
        Ok(ToolOutput::ok(format!(
            "Successfully replaced {} {} in {}.\n```diff\n{}```",
            edits.len(),
            noun,
            path_str,
            diff.trim_end()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::extension::Cancel;

    fn tmp_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rab-edit-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_tool() -> (EditTool, std::path::PathBuf) {
        let tmp = tmp_dir();
        let tool = EditTool { cwd: tmp.clone() };
        (tool, tmp)
    }

    async fn exec_ok(tool: &EditTool, args: serde_json::Value) -> String {
        tool.execute("id".into(), args, Cancel::new())
            .await
            .unwrap()
            .content
    }

    async fn exec_err(tool: &EditTool, args: serde_json::Value) -> String {
        tool.execute("id".into(), args, Cancel::new())
            .await
            .unwrap_err()
            .to_string()
    }

    async fn is_err(tool: &EditTool, args: serde_json::Value) -> bool {
        tool.execute("id".into(), args, Cancel::new())
            .await
            .is_err()
    }

    #[tokio::test]
    async fn single_edit_replaces_text() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("file.txt");
        std::fs::write(&path, "hello world\nfoo bar\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "foo bar", "newText": "baz qux"}]
            }),
        )
        .await;

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "hello world\nbaz qux\n"
        );
    }

    #[tokio::test]
    async fn multiple_edits_replaces_all() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("file.txt");
        std::fs::write(&path, "aaa\nbbb\nccc\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [
                    {"oldText": "aaa", "newText": "111"},
                    {"oldText": "ccc", "newText": "333"}
                ]
            }),
        )
        .await;

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "111\nbbb\n333\n");
    }

    #[tokio::test]
    async fn non_unique_oldtext_errors() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("file.txt");
        std::fs::write(&path, "dup\ndup\n").unwrap();

        assert!(is_err(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "dup", "newText": "x"}]
            }),
        )
        .await);
    }

    #[tokio::test]
    async fn missing_oldtext_errors() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("file.txt");
        std::fs::write(&path, "content\n").unwrap();

        let err = exec_err(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "not found", "newText": "x"}]
            }),
        )
        .await;
        assert!(err.contains("Could not find"));
    }

    #[tokio::test]
    async fn overlapping_edits_error() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("file.txt");
        std::fs::write(&path, "abcdef\n").unwrap();

        assert!(is_err(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [
                    {"oldText": "abc", "newText": "1"},
                    {"oldText": "bcd", "newText": "2"}
                ]
            }),
        )
        .await);
    }

    #[tokio::test]
    async fn empty_edits_errors() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("file.txt");
        std::fs::write(&path, "content\n").unwrap();

        assert!(is_err(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "edits": []}),
        )
        .await);
    }

    // ── BOM handling ─────────────────────────────────────────

    #[tokio::test]
    async fn handles_bom() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("bom.txt");
        std::fs::write(&path, "\u{FEFF}hello world\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "hello world", "newText": "goodbye"}]
            }),
        )
        .await;

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with('\u{FEFF}'));
        assert!(content.contains("goodbye"));
    }

    #[tokio::test]
    async fn preserves_bom_when_no_edit_at_start() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("bom2.txt");
        std::fs::write(&path, "\u{FEFF}line1\nline2\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "line2", "newText": "modified"}]
            }),
        )
        .await;

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with('\u{FEFF}'));
        assert!(content.contains("modified"));
    }

    // ── CRLF handling ────────────────────────────────────────

    #[tokio::test]
    async fn preserves_crlf() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("crlf.txt");
        std::fs::write(&path, "hello\r\nworld\r\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "world", "newText": "universe"}]
            }),
        )
        .await;

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello\r\nuniverse\r\n");
    }

    #[tokio::test]
    async fn handles_mixed_line_endings() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("mixed.txt");
        std::fs::write(&path, "line1\r\nline2\nline3\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "line2", "newText": "modified"}]
            }),
        )
        .await;

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "line1\r\nmodified\r\nline3\r\n");
    }

    #[tokio::test]
    async fn lf_only_stays_lf() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("lf.txt");
        std::fs::write(&path, "hello\nworld\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "world", "newText": "universe"}]
            }),
        )
        .await;

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello\nuniverse\n");
    }

    // ── Fuzzy matching ───────────────────────────────────────

    #[tokio::test]
    async fn fuzzy_match_trailing_whitespace() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("trailing.txt");
        std::fs::write(&path, "hello world  \nnext line\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "hello world", "newText": "hi there"}]
            }),
        )
        .await;

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hi there\nnext line\n");
    }

    #[tokio::test]
    async fn fuzzy_match_smart_quotes() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("quotes.txt");
        std::fs::write(&path, "he said \u{201C}hello\u{201D}\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "he said \"hello\"", "newText": "she said \"hi\""}]
            }),
        )
        .await;

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "she said \"hi\"\n");
    }

    #[tokio::test]
    async fn fuzzy_match_dashes() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("dashes.txt");
        std::fs::write(&path, "foo \u{2014} bar\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "foo - bar", "newText": "baz"}]
            }),
        )
        .await;

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "baz\n");
    }

    // ── Input normalization ──────────────────────────────────

    #[tokio::test]
    async fn legacy_oldtext_newtext() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("legacy.txt");
        std::fs::write(&path, "hello world\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "oldText": "hello world",
                "newText": "goodbye"
            }),
        )
        .await;

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "goodbye\n");
    }

    #[tokio::test]
    async fn edits_as_json_string() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("jsonstr.txt");
        std::fs::write(&path, "aaa\nbbb\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": r#"[{"oldText": "bbb", "newText": "xxx"}]"#
            }),
        )
        .await;

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "aaa\nxxx\n");
    }

    // ── Diff output ──────────────────────────────────────────

    #[tokio::test]
    async fn result_contains_diff() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("diff_test.txt");
        std::fs::write(&path, "aaa\nbbb\nccc\n").unwrap();

        let result = exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "bbb", "newText": "xxx"}]
            }),
        )
        .await;

        assert!(result.contains("```diff"));
        assert!(result.contains("-bbb"));
        assert!(result.contains("+xxx"));
        assert!(result.contains("Successfully replaced 1 block"));
    }

    // ── Empty oldText ────────────────────────────────────────

    #[tokio::test]
    async fn empty_oldtext_errors() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("empty.txt");
        std::fs::write(&path, "content\n").unwrap();

        let err = exec_err(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "", "newText": "x"}]
            }),
        )
        .await;
        assert!(err.contains("empty"));
    }

    // ── Relative paths ───────────────────────────────────────

    #[tokio::test]
    async fn relative_path_resolves_to_cwd() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("relative.txt");
        std::fs::write(&path, "hello\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": "relative.txt",
                "edits": [{"oldText": "hello", "newText": "hi"}]
            }),
        )
        .await;

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi\n");
    }
}

#[cfg(test)]
mod fuzzy_tests {
    use super::*;

    #[test]
    fn test_strip_trailing_whitespace() {
        assert_eq!(normalize_for_fuzzy_match("hello   \nworld  "), "hello\nworld");
    }

    #[test]
    fn test_smart_quotes() {
        assert_eq!(
            normalize_for_fuzzy_match("\u{2018}hello\u{2019} \u{201C}world\u{201D}"),
            "'hello' \"world\""
        );
    }

    #[test]
    fn test_dashes() {
        assert_eq!(normalize_for_fuzzy_match("a\u{2014}b"), "a-b");
        assert_eq!(normalize_for_fuzzy_match("a\u{2013}b"), "a-b");
    }

    #[test]
    fn test_nbsp() {
        assert_eq!(normalize_for_fuzzy_match("a\u{00A0}b"), "a b");
    }

    #[test]
    fn test_preserves_trailing_newline() {
        assert_eq!(normalize_for_fuzzy_match("hello\n"), "hello\n");
        assert_eq!(normalize_for_fuzzy_match("hello\nworld\n"), "hello\nworld\n");
    }
}

#[cfg(test)]
mod diff_tests {
    use super::*;

    #[test]
    fn test_simple_diff() {
        let orig = "aaa\nbbb\nccc\n";
        let modified = "aaa\nxxx\nccc\n";
        let diff = compute_diff(orig, modified, "test.txt");
        assert!(diff.contains("--- a/test.txt"));
        assert!(diff.contains("+++ b/test.txt"));
        assert!(diff.contains("-bbb"));
        assert!(diff.contains("+xxx"));
    }

    #[test]
    fn test_no_changes() {
        let text = "hello\nworld\n";
        let diff = compute_diff(text, text, "f.txt");
        assert!(diff.contains("--- a/f.txt"));
        assert!(diff.contains("+++ b/f.txt"));
        assert!(!diff.contains("@@"));
    }

    #[test]
    fn test_multiple_hunks() {
        let orig = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let modified = "a\nX\nc\nd\ne\nY\ng\nh\n";
        let diff = compute_diff(orig, modified, "f.txt");
        assert!(diff.contains("-b"));
        assert!(diff.contains("+X"));
        assert!(diff.contains("-f"));
        assert!(diff.contains("+Y"));
    }
}
