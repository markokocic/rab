use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use anyhow::Context;
use async_trait::async_trait;
use std::borrow::Cow;
use tokio::sync::mpsc::UnboundedSender;

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
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => {
                result.push('-');
            }
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
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
        return Err("Missing 'edits' array (or 'oldText'/'newText' for legacy format)".to_string());
    };

    if edits.is_empty() {
        return Err("At least one edit is required".to_string());
    }

    Ok((path.to_string(), edits))
}

// ── Diff computation ─────────────────────────────────────────────

/// Replace tabs with 3 spaces for consistent rendering.
fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

/// Compute a display-oriented diff string with line numbers and context.
/// Produces pi-compatible format:
/// `+{lineNum} {content}` / `-{lineNum} {content}` / ` {lineNum} {content}` / `  ...`
/// With line numbers padded to the width of the max line number.
fn compute_diff(original: &str, modified: &str, _path: &str) -> String {
    let orig_lines: Vec<&str> = original.lines().collect();
    let mod_lines: Vec<&str> = modified.lines().collect();

    let max_line_num = orig_lines.len().max(mod_lines.len());
    let line_num_width = max_line_num.to_string().len();

    let mut output: Vec<String> = Vec::new();

    // Use LCS to find the diff
    let n = orig_lines.len();
    let m = mod_lines.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            if orig_lines[i - 1] == mod_lines[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to build sequence of changes
    let mut changes: Vec<(char, &str)> = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && orig_lines[i - 1] == mod_lines[j - 1] {
            changes.push((' ', orig_lines[i - 1]));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            changes.push(('+', mod_lines[j - 1]));
            j -= 1;
        } else {
            changes.push(('-', orig_lines[i - 1]));
            i -= 1;
        }
    }
    changes.reverse();

    // Group into hunks with context boundaries
    const CONTEXT_LINES: usize = 4;
    let mut old_line_num: usize = 1;
    let mut new_line_num: usize = 1;
    let mut _last_was_change = false;

    let pad = |num: usize| -> String { format!("{:width$}", num, width = line_num_width) };

    let mut k = 0;
    while k < changes.len() {
        let (tag, _text) = changes[k];

        if tag == ' ' {
            // Context line
            let mut ctx_buffer: Vec<&str> = Vec::new();
            let ctx_start = k;
            while k < changes.len() && changes[k].0 == ' ' {
                ctx_buffer.push(changes[k].1);
                k += 1;
            }
            let ctx_end = k;
            let has_leading_change = ctx_start > 0 && changes[ctx_start - 1].0 != ' ';
            let has_trailing_change = ctx_end < changes.len() - 1;

            if has_leading_change || has_trailing_change {
                // Show context around changes (pi-style)
                let total_ctx = ctx_buffer.len();

                if has_leading_change && has_trailing_change {
                    if total_ctx <= CONTEXT_LINES * 2 {
                        // Show all
                        for &line in &ctx_buffer {
                            output.push(format!(" {} {}", pad(old_line_num), replace_tabs(line)));
                            old_line_num += 1;
                            new_line_num += 1;
                        }
                    } else {
                        let leading = &ctx_buffer[..CONTEXT_LINES];
                        let trailing = &ctx_buffer[total_ctx - CONTEXT_LINES..];
                        let skipped = total_ctx - leading.len() - trailing.len();

                        for &line in leading {
                            output.push(format!(" {} {}", pad(old_line_num), replace_tabs(line)));
                            old_line_num += 1;
                            new_line_num += 1;
                        }

                        output.push(format!(" {} ...", " ".repeat(line_num_width)));
                        old_line_num += skipped;
                        new_line_num += skipped;

                        for &line in trailing {
                            output.push(format!(" {} {}", pad(old_line_num), replace_tabs(line)));
                            old_line_num += 1;
                            new_line_num += 1;
                        }
                    }
                } else if has_leading_change {
                    // Context after a change: show CONTEXT_LINES trailing
                    let shown = ctx_buffer.len().min(CONTEXT_LINES);
                    let skipped = ctx_buffer.len() - shown;

                    if skipped > 0 {
                        output.push(format!(" {} ...", " ".repeat(line_num_width)));
                        old_line_num += skipped;
                        new_line_num += skipped;
                    }

                    for &line in &ctx_buffer[ctx_buffer.len() - shown..] {
                        output.push(format!(" {} {}", pad(old_line_num), replace_tabs(line)));
                        old_line_num += 1;
                        new_line_num += 1;
                    }
                } else if has_trailing_change {
                    // Context before a change: show CONTEXT_LINES leading
                    let shown = ctx_buffer.len().min(CONTEXT_LINES);
                    let skipped = ctx_buffer.len() - shown;

                    if skipped > 0 {
                        output.push(format!(" {} ...", " ".repeat(line_num_width)));
                        old_line_num += skipped;
                        new_line_num += skipped;
                    }

                    for &line in &ctx_buffer[..shown] {
                        output.push(format!(" {} {}", pad(old_line_num), replace_tabs(line)));
                        old_line_num += 1;
                        new_line_num += 1;
                    }
                }
            } else {
                // No surrounding changes - skip entirely
                old_line_num += ctx_buffer.len();
                new_line_num += ctx_buffer.len();
            }

            _last_was_change = false;
        } else {
            // Change (removed or added)
            let mut removed: Vec<&str> = Vec::new();
            while k < changes.len() && changes[k].0 == '-' {
                removed.push(changes[k].1);
                k += 1;
            }
            let mut added: Vec<&str> = Vec::new();
            while k < changes.len() && changes[k].0 == '+' {
                added.push(changes[k].1);
                k += 1;
            }

            // Show all removed lines first
            for &line in &removed {
                output.push(format!("-{} {}", pad(old_line_num), replace_tabs(line)));
                old_line_num += 1;
            }
            // Then all added lines
            for &line in &added {
                output.push(format!("+{} {}", pad(new_line_num), replace_tabs(line)));
                new_line_num += 1;
            }

            _last_was_change = true;
        }
    }

    output.join("\n")
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

    fn renderer(&self) -> Option<Box<dyn ToolRenderer>> {
        Some(Box::new(EditRenderer))
    }

    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
        cancel: Cancel,
        _on_update: Option<UnboundedSender<ToolOutput>>,
    ) -> anyhow::Result<ToolOutput> {
        let _ = tool_call_id;
        let (path_str, edits) =
            prepare_edit_arguments(&args).map_err(|e| anyhow::anyhow!("{}", e))?;

        cancel.check()?;

        let cwd = self.cwd.clone();
        let path_for_queue = path_str.clone();
        let cwd_for_closure = cwd.clone();

        // Wrap the entire read-edit-write in a per-file mutation queue so
        // concurrent edits to the same file are serialized (pi-style).
        let output = crate::builtin::file_mutation_queue::with_file_mutation_queue(
            &path_for_queue,
            &cwd,
            || async move {
                let abs_path = {
                    let p = std::path::Path::new(&path_str);
                    if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        cwd_for_closure.join(p)
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
                let work_content = normalize_for_fuzzy_match(&normalized);

                // ── 4. Validate and find each edit ──
                let mut matched_indices: Vec<(usize, usize)> = Vec::new();

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

                    let pos = work_content.find(&fuzzy_old).unwrap();
                    matched_indices.push((pos, pos + fuzzy_old.len()));
                }

                // ── 5. Check for overlapping edits ──
                for (idx_i, &(pos_i, end_i)) in matched_indices.iter().enumerate() {
                    for (idx_j, &(pos_j, end_j)) in matched_indices.iter().enumerate().skip(idx_i + 1) {
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
                let mut sorted: Vec<(usize, usize, &Edit)> = matched_indices
                    .into_iter()
                    .zip(edits.iter())
                    .map(|((start, end), edit)| (start, end, edit))
                    .collect();
                sorted.sort_by_key(|(pos, _, _)| *pos);

                let mut modified = String::new();
                let mut cursor = 0;
                for (start, end, edit) in &sorted {
                    modified.push_str(&work_content[cursor..*start]);
                    modified.push_str(&edit.new_text);
                    cursor = *end;
                }
                modified.push_str(&work_content[cursor..]);

                // ── 7. Compute diff ──
                let diff = compute_diff(&normalized, &modified, &path_str);

                // ── 8. Write back with original line endings and BOM ──
                let final_content =
                    bom.to_string() + &restore_line_endings(&modified, original_ending);
                std::fs::write(&abs_path, &final_content)
                    .with_context(|| format!("Failed to write {}", abs_path.display()))?;

                // ── 9. Return result ──
                let noun = if edits.len() == 1 { "block" } else { "blocks" };
                Ok(format!(
                    "Successfully replaced {} {} in {}.\n```diff\n{}```",
                    edits.len(),
                    noun,
                    path_str,
                    diff.trim_end()
                ))
            },
        )
        .await?;

        Ok(ToolOutput::ok(output))
    }
}

/// Tool renderer for the `edit` tool.
/// Uses `renderShell: "self"` - renders its own framing without colored box.
/// Shows a preview of what will change in the call header.
struct EditRenderer;

impl ToolRenderer for EditRenderer {
    fn render_self(&self) -> bool {
        true
    }

    fn render_call(
        &self,
        args: &serde_json::Value,
        width: usize,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Vec<String> {
        let path = args
            .get("file_path")
            .or_else(|| args.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let short = if let Ok(home) = std::env::var("HOME") {
            path.replacen(&home, "~", 1)
        } else {
            path.to_string()
        };
        let path_disp = if short.is_empty() {
            String::new()
        } else {
            theme.fg_key(ThemeKey::Accent, &short)
        };

        let mut lines = vec![format!(
            "{} {}",
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold("edit")),
            path_disp
        )];

        // Show edit preview (compact summary of changes).
        // Matches Pi's buildEditCallComponent which always includes the preview
        // when available, regardless of expanded state.
        if let Some(edits) = args.get("edits") {
            let edits_arr = if let Some(arr) = edits.as_array() {
                arr.as_slice()
            } else {
                static EMPTY: [serde_json::Value; 0] = [];
                &EMPTY // Can't parse here, skip preview
            };

            for edit in edits_arr.iter().take(3) {
                if let (Some(old), new) = (edit.get("oldText"), edit.get("newText"))
                    && let (Some(old_str), Some(new_str)) =
                        (old.as_str(), new.and_then(|v| v.as_str()))
                {
                    let preview = format_edit_preview(old_str, new_str, width, theme);
                    lines.extend(preview);
                }
            }

            if edits_arr.len() > 3 {
                lines.push(theme.fg(
                    "muted",
                    &format!("... and {} more edits", edits_arr.len() - 3),
                ));
            }
        }

        lines
    }

    fn render_result(
        &self,
        content: &str,
        _width: usize,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Vec<String> {
        // Extract diff from ```diff ... ``` block in the result
        if let Some(start) = content.find("```diff\n") {
            let after = &content[start + 8..];
            if let Some(end) = after.find("```") {
                let diff_text = &after[..end];
                let has_diff = diff_text
                    .lines()
                    .any(|l| l.starts_with('-') || l.starts_with('+') || l.starts_with(' '));
                if has_diff {
                    let rendered = crate::tui::components::diff::render_diff(diff_text, theme);
                    return rendered;
                }
            }
        }
        // Fallback: show content as-is
        if content.is_empty() {
            return vec![];
        }
        vec![theme.fg_key(ThemeKey::ToolOutput, content)]
    }
}

/// Format a compact preview of a single edit operation.
/// Shows first N chars of oldText → first N chars of newText as separate lines.
fn format_edit_preview(old: &str, new: &str, _width: usize, theme: &dyn Theme) -> Vec<String> {
    let max_preview = 30;
    let old_first_line = old.lines().next().unwrap_or("");
    let new_first_line = new.lines().next().unwrap_or("");

    let old_preview = truncate_simple(old_first_line, max_preview);
    let new_preview = truncate_simple(new_first_line, max_preview);

    let old_styled = theme.fg_key(ThemeKey::ToolDiffRemoved, &format!("-{}", old_preview));
    let new_styled = theme.fg_key(ThemeKey::ToolDiffAdded, &format!("+{}", new_preview));
    vec![format!("  {}", old_styled), format!("  {}", new_styled)]
}

/// Truncate a string to max_chars (characters), adding "..." if truncated.
fn truncate_simple(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else if max_chars > 3 {
        let truncated: String = s.chars().take(max_chars - 3).collect();
        format!("{}...", truncated)
    } else {
        s.chars().take(max_chars).collect()
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
        tool.execute("id".into(), args, Cancel::new(), None)
            .await
            .unwrap()
            .content
    }

    async fn exec_err(tool: &EditTool, args: serde_json::Value) -> String {
        tool.execute("id".into(), args, Cancel::new(), None)
            .await
            .unwrap_err()
            .to_string()
    }

    async fn is_err(tool: &EditTool, args: serde_json::Value) -> bool {
        tool.execute("id".into(), args, Cancel::new(), None)
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

        assert!(
            is_err(
                &tool,
                serde_json::json!({
                    "path": path.to_str().unwrap(),
                    "edits": [{"oldText": "dup", "newText": "x"}]
                }),
            )
            .await
        );
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

        assert!(
            is_err(
                &tool,
                serde_json::json!({
                    "path": path.to_str().unwrap(),
                    "edits": [
                        {"oldText": "abc", "newText": "1"},
                        {"oldText": "bcd", "newText": "2"}
                    ]
                }),
            )
            .await
        );
    }

    #[tokio::test]
    async fn empty_edits_errors() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("file.txt");
        std::fs::write(&path, "content\n").unwrap();

        assert!(
            is_err(
                &tool,
                serde_json::json!({"path": path.to_str().unwrap(), "edits": []}),
            )
            .await
        );
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
        // Pi-style format with line numbers: "-2 bbb" and "+2 xxx"
        // (bbb/xxx are on line 2 of the original/modified content)
        assert!(
            result.contains("-2 bbb"),
            "expected '-2 bbb' in diff but got: {}",
            result
        );
        assert!(
            result.contains("+2 xxx"),
            "expected '+2 xxx' in diff but got: {}",
            result
        );
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
        assert_eq!(
            normalize_for_fuzzy_match("hello   \nworld  "),
            "hello\nworld"
        );
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
        assert_eq!(
            normalize_for_fuzzy_match("hello\nworld\n"),
            "hello\nworld\n"
        );
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
        assert!(
            diff.contains("-2 bbb"),
            "diff should contain -2 bbb but got: {}",
            diff
        );
        assert!(
            diff.contains("+2 xxx"),
            "diff should contain +2 xxx but got: {}",
            diff
        );
    }

    #[test]
    fn test_no_changes() {
        let text = "hello\nworld\n";
        let diff = compute_diff(text, text, "f.txt");
        assert!(diff.is_empty(), "no changes should produce empty diff");
    }

    #[test]
    fn test_multiple_hunks() {
        let orig = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let modified = "a\nX\nc\nd\ne\nY\ng\nh\n";
        let diff = compute_diff(orig, modified, "f.txt");
        assert!(
            diff.contains("-2 b"),
            "should contain -2 b but got: {}",
            diff
        );
        assert!(
            diff.contains("+2 X"),
            "should contain +2 X but got: {}",
            diff
        );
        assert!(
            diff.contains("-6 f"),
            "should contain -6 f but got: {}",
            diff
        );
        assert!(
            diff.contains("+6 Y"),
            "should contain +6 Y but got: {}",
            diff
        );
    }
}
