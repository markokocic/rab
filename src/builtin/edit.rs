use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use async_trait::async_trait;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;
use unicode_normalization::UnicodeNormalization;

// ── EditOperations (pluggable) ─────────────────────────────────────

/// Pluggable operations for the edit tool (matching pi's EditOperations).
/// Override these to delegate file editing to remote systems (for example SSH).
#[async_trait]
pub trait EditOperations: Send + Sync {
    /// Read file contents as a String.
    async fn read_file(&self, absolute_path: &Path) -> anyhow::Result<String>;
    /// Write content to a file.
    async fn write_file(&self, absolute_path: &Path, content: &str) -> anyhow::Result<()>;
    /// Check if file is readable and writable (throw if not).
    async fn access(&self, absolute_path: &Path) -> anyhow::Result<()>;
}

struct DefaultEditOperations;

#[async_trait]
impl EditOperations for DefaultEditOperations {
    async fn read_file(&self, absolute_path: &Path) -> anyhow::Result<String> {
        Ok(std::fs::read_to_string(absolute_path)?)
    }

    async fn write_file(&self, absolute_path: &Path, content: &str) -> anyhow::Result<()> {
        Ok(std::fs::write(absolute_path, content)?)
    }

    async fn access(&self, absolute_path: &Path) -> anyhow::Result<()> {
        if !absolute_path.exists() {
            anyhow::bail!("File not found: {}", absolute_path.display());
        }
        if !absolute_path.is_file() {
            anyhow::bail!("Not a file: {}", absolute_path.display());
        }
        Ok(())
    }
}

pub struct EditExtension {
    cwd: PathBuf,
    operations: Arc<dyn EditOperations>,
}

impl EditExtension {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            operations: Arc::new(DefaultEditOperations),
        }
    }

    /// Set custom edit operations (e.g. for SSH targets).
    pub fn with_operations(mut self, operations: Arc<dyn EditOperations>) -> Self {
        self.operations = operations;
        self
    }
}

impl Extension for EditExtension {
    fn name(&self) -> Cow<'static, str> {
        "edit".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(EditTool {
            cwd: self.cwd.clone(),
            renderer: EditRenderer::new(),
            operations: self.operations.clone(),
        })]
    }
}

struct EditTool {
    cwd: PathBuf,
    renderer: EditRenderer,
    operations: Arc<dyn EditOperations>,
}

#[derive(serde::Deserialize, Clone)]
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

/// Normalize text for fuzzy matching (pi-compatible).
/// Applies progressive transformations:
/// - NFKC normalization (handles composed/decomposed Unicode)
/// - Strip trailing whitespace from each line
/// - Normalize Unicode smart quotes → ASCII quotes
/// - Normalize Unicode dashes/hyphens → ASCII hyphen
/// - Normalize special Unicode spaces → regular space
fn normalize_for_fuzzy_match(text: &str) -> String {
    // First: NFKC normalization (pi calls .normalize("NFKC"))
    let nfkc = text.nfkc().collect::<String>();

    // Second: strip trailing whitespace per line
    let mut intermediate = String::with_capacity(nfkc.len());
    for line in nfkc.lines() {
        if !intermediate.is_empty() {
            intermediate.push('\n');
        }
        intermediate.push_str(line.trim_end());
    }
    // Handle trailing newline: lines() strips final newline, re-add if present
    if nfkc.ends_with('\n') {
        intermediate.push('\n');
    }

    // Third: normalize Unicode characters to ASCII equivalents
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

// ── Line-span tracking for fuzzy mapping ────────────────────────

/// A line span tracking the byte offsets of a line in the content.
/// Matches pi's `LineSpan` struct.
#[derive(Debug, Clone, Copy)]
struct LineSpan {
    start: usize,
    end: usize,
}

/// Split content into lines, preserving each line's ending.
/// Returns Vec<&str> where each element includes its line ending if present.
fn split_lines_with_endings(content: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut remaining = content;
    while let Some(pos) = remaining.find('\n') {
        result.push(&remaining[..=pos]);
        remaining = &remaining[pos + 1..];
    }
    if !remaining.is_empty() {
        result.push(remaining);
    }
    result
}

/// Get line spans for the content.
fn get_line_spans(content: &str) -> Vec<LineSpan> {
    let mut offset = 0;
    split_lines_with_endings(content)
        .iter()
        .map(|line| {
            let span = LineSpan {
                start: offset,
                end: offset + line.len(),
            };
            offset = span.end;
            span
        })
        .collect()
}

/// Get the line range that a replacement touches.
fn get_replacement_line_range(
    lines: &[LineSpan],
    match_index: usize,
    match_length: usize,
) -> (usize, usize) {
    let replacement_end = match_index + match_length;

    let mut start_line = 0;
    for (i, line) in lines.iter().enumerate() {
        if match_index >= line.start && match_index < line.end {
            start_line = i;
            break;
        }
    }

    let mut end_line = start_line;
    while end_line < lines.len() && lines[end_line].end < replacement_end {
        end_line += 1;
    }
    if end_line >= lines.len() {
        end_line = lines.len() - 1;
    }

    (start_line, end_line + 1)
}

/// Apply replacements to content (applied in reverse order to keep offsets stable).
/// Each replacement is (matchIndex, matchLength, newText).
fn apply_replacements(
    content: &str,
    replacements: &[(usize, usize, &str)],
    offset: usize,
) -> String {
    let mut result = content.to_string();
    for (start, length, new_text) in replacements.iter().rev() {
        let adj_start = start - offset;
        let adj_end = adj_start + length;
        result.replace_range(adj_start..adj_end, new_text);
    }
    result
}

/// Map changes made in fuzzy-normalized space back to the original (LF-normalized)
/// content, preserving the original bytes of unchanged lines (pi-compatible).
///
/// Uses line-span tracking and groups overlapping replacements, matching pi's
/// `applyReplacementsPreservingUnchangedLines`.
fn apply_replacements_preserving_unchanged_lines(
    original_content: &str,
    base_content: &str,
    replacements: &[(usize, usize, &str)], // (matchIndex, matchLength, newText) sorted by matchIndex
) -> String {
    let original_lines = split_lines_with_endings(original_content);
    let base_lines = get_line_spans(base_content);

    if original_lines.len() != base_lines.len() {
        // Line count mismatch — fall back to simple application
        let mut result = base_content.to_string();
        for (start, end, new_text) in replacements.iter().rev() {
            result.replace_range(*start..*end, new_text);
        }
        return result;
    }

    // Build groups of overlapping replacements
    struct Group {
        start_line: usize,
        end_line: usize,
        replacements: Vec<(usize, usize, String)>, // (matchIndex, matchLength, newText)
    }

    let mut groups: Vec<Group> = Vec::new();
    for &(start, end, new_text) in replacements {
        let (sl, el) = get_replacement_line_range(&base_lines, start, end);
        if let Some(last) = groups.last_mut()
            && sl < last.end_line
        {
            last.end_line = last.end_line.max(el);
            last.replacements.push((start, end, new_text.to_string()));
            continue;
        }
        groups.push(Group {
            start_line: sl,
            end_line: el,
            replacements: vec![(start, end, new_text.to_string())],
        });
    }

    let mut original_line_index = 0;
    let mut result = String::new();

    for group in &groups {
        // Copy unchanged original lines
        result.push_str(&original_lines[original_line_index..group.start_line].concat());

        // Apply replacements to the base content slice for this group
        let group_start_offset = base_lines[group.start_line].start;
        let group_end_offset = base_lines[group.end_line - 1].end;
        let group_slice = &base_content[group_start_offset..group_end_offset];
        let adjusted_replacements: Vec<(usize, usize, &str)> = group
            .replacements
            .iter()
            .map(|(s, e, t)| (*s - group_start_offset, *e, t.as_str()))
            .collect();
        result.push_str(&apply_replacements(group_slice, &adjusted_replacements, 0));

        original_line_index = group.end_line;
    }

    // Copy remaining original lines
    result.push_str(&original_lines[original_line_index..].concat());

    result
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
                    // Context after a change (change before context): show CONTEXT_LINES leading
                    let shown = ctx_buffer.len().min(CONTEXT_LINES);
                    let skipped = ctx_buffer.len() - shown;

                    for &line in &ctx_buffer[..shown] {
                        output.push(format!(" {} {}", pad(old_line_num), replace_tabs(line)));
                        old_line_num += 1;
                        new_line_num += 1;
                    }

                    if skipped > 0 {
                        output.push(format!(" {} ...", " ".repeat(line_num_width)));
                        old_line_num += skipped;
                        new_line_num += skipped;
                    }
                } else if has_trailing_change {
                    // Context before a change (change after context): show CONTEXT_LINES trailing
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
                }
            } else {
                // No surrounding changes - skip entirely
                old_line_num += ctx_buffer.len();
                new_line_num += ctx_buffer.len();
            }
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
        }
    }

    output.join("\n")
}

/// Parse path and edits from args without validation errors — returns None if
/// arguments are not yet complete (for preview computation).
fn parse_path_edits(args: &serde_json::Value) -> Option<(String, Vec<Edit>)> {
    let path = args.get("path").and_then(|v| v.as_str())?;
    let edits: Vec<Edit> = if let Some(edits_val) = args.get("edits") {
        if let Some(s) = edits_val.as_str() {
            serde_json::from_str(s).ok()?
        } else {
            serde_json::from_value(edits_val.clone()).ok()?
        }
    } else if let (Some(old), Some(new)) = (args.get("oldText"), args.get("newText")) {
        let old_text = old.as_str()?;
        let new_text = new.as_str()?;
        vec![Edit {
            old_text: old_text.to_string(),
            new_text: new_text.to_string(),
        }]
    } else {
        return None;
    };

    if edits.is_empty() {
        return None;
    }

    Some((path.to_string(), edits))
}

/// Apply edits to normalized content and return (normalized, base_content, new_content, diff).
/// This is the core edit logic extracted for reuse by both execute and preview.
///
/// Returns Ok((normalized, base_content, new_content, diff_string)) on success,
/// or Err(error_message) if edits can't be applied.
fn apply_edits_and_compute_diff(
    normalized: &str,
    edits: &[Edit],
    path_str: &str,
) -> Result<(String, String, String), String> {
    // Determine if fuzzy matching is needed
    let mut needs_fuzzy = false;
    for edit in edits {
        let old_lf = normalize_to_lf(&edit.old_text);
        if !normalized.contains(&old_lf) {
            needs_fuzzy = true;
            break;
        }
    }

    // Build work content: exact or fuzzy-normalized
    let fuzzy_owned;
    let (work_content, is_fuzzy_space) = if needs_fuzzy {
        fuzzy_owned = normalize_for_fuzzy_match(normalized);
        (fuzzy_owned.as_str(), true)
    } else {
        (normalized, false)
    };

    let mut matched_indices: Vec<(usize, usize)> = Vec::new();

    for (i, edit) in edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return if edits.len() == 1 {
                Err(format!("oldText must not be empty in {}.", path_str))
            } else {
                Err(format!(
                    "edits[{}].oldText must not be empty in {}.",
                    i, path_str
                ))
            };
        }

        let search_text = if is_fuzzy_space {
            normalize_for_fuzzy_match(&normalize_to_lf(&edit.old_text))
        } else {
            normalize_to_lf(&edit.old_text)
        };
        let count = work_content.matches(&search_text).count();

        if count == 0 {
            return if edits.len() == 1 {
                Err(format!(
                    "Could not find the exact text in {}. \
                     The old text must match exactly including all whitespace and newlines.",
                    path_str
                ))
            } else {
                Err(format!(
                    "Could not find edits[{}] in {}. \
                     The oldText must match exactly including all whitespace and newlines.",
                    i, path_str
                ))
            };
        }

        if count > 1 {
            return if edits.len() == 1 {
                Err(format!(
                    "Found {} occurrences of the text in {}. \
                     The text must be unique. Please provide more context to make it unique.",
                    count, path_str
                ))
            } else {
                Err(format!(
                    "Found {} occurrences of edits[{}] in {}. \
                     Each oldText must be unique. Please provide more context to make it unique.",
                    count, i, path_str
                ))
            };
        }

        let pos = work_content.find(&search_text).unwrap();
        matched_indices.push((pos, pos + search_text.len()));
    }

    // Check for overlapping edits
    for (idx_i, &(pos_i, end_i)) in matched_indices.iter().enumerate() {
        for (idx_j, &(pos_j, end_j)) in matched_indices.iter().enumerate().skip(idx_i + 1) {
            if pos_i < end_j && pos_j < end_i {
                return Err(format!(
                    "edits[{}] and edits[{}] overlap in {}. Merge them into one edit or target disjoint regions.",
                    idx_i, idx_j, path_str
                ));
            }
        }
    }

    // Apply edits (sorted left-to-right)
    let mut sorted: Vec<(usize, usize, &Edit)> = matched_indices
        .into_iter()
        .zip(edits.iter())
        .map(|((start, end), edit)| (start, end, edit))
        .collect();
    sorted.sort_by_key(|(pos, _, _)| *pos);

    let (base_content, new_content) = if is_fuzzy_space {
        // Build replacement tuples for the preserving function
        let mapped_refs: Vec<(usize, usize, &str)> = sorted
            .iter()
            .map(|(start, end, edit)| (*start, *end - *start, &edit.new_text[..]))
            .collect();

        let new_content =
            apply_replacements_preserving_unchanged_lines(normalized, work_content, &mapped_refs);

        (normalized.to_string(), new_content)
    } else {
        let mut modified = String::new();
        let mut cursor = 0usize;
        for (start, end, edit) in &sorted {
            modified.push_str(&normalized[cursor..*start]);
            modified.push_str(&normalize_to_lf(&edit.new_text));
            cursor = *end;
        }
        modified.push_str(&normalized[cursor..]);
        (normalized.to_string(), modified)
    };

    // No-change detection
    if base_content == new_content {
        return if edits.len() == 1 {
            Err(format!(
                "No changes made to {}. The replacement produced identical content. \
                 This might indicate an issue with special characters or the text not \
                 existing as expected.",
                path_str
            ))
        } else {
            Err(format!(
                "No changes made to {}. The replacements produced identical content.",
                path_str
            ))
        };
    }

    let diff = compute_diff(&base_content, &new_content, path_str);

    Ok((base_content, new_content, diff))
}

/// Read a file and compute what the diff would look like if edits were applied.
/// This is used for the preview rendering (matching pi's computeEditsDiff).
fn compute_edits_diff(
    path_str: &str,
    edits: &[Edit],
    cwd: &std::path::Path,
) -> Result<String, String> {
    let abs_path = {
        let p = std::path::Path::new(path_str);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        }
    };

    let raw_content =
        std::fs::read_to_string(&abs_path).map_err(|e| format!("Could not read file: {}", e))?;

    let (_bom, content) = strip_bom(&raw_content);
    let normalized = normalize_to_lf(content);

    let (_, _, diff) = apply_edits_and_compute_diff(&normalized, edits, path_str)?;

    Ok(diff)
}

// ── AgentTool implementation ─────────────────────────────────────

#[async_trait]
impl AgentTool for EditTool {
    fn clone_boxed(&self) -> Box<dyn AgentTool> {
        Box::new(Self {
            cwd: self.cwd.clone(),
            renderer: self.renderer.clone(),
            operations: self.operations.clone(),
        })
    }

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

    fn prompt_snippet(&self) -> Option<Cow<'static, str>> {
        Some("Make precise file edits with exact text replacement, including multiple disjoint edits in one call".into())
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        vec![
            "Use edit for precise changes (edits[].oldText must match exactly)".into(),
            "When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls".into(),
            "Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.".into(),
            "Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.".into(),
        ]
    }

    fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
        let (path_str, edits) = match prepare_edit_arguments(&args) {
            Ok(result) => result,
            Err(_) => return args,
        };
        serde_json::json!({
            "path": path_str,
            "edits": edits.iter().map(|e| serde_json::json!({
                "oldText": e.old_text,
                "newText": e.new_text
            })).collect::<Vec<_>>()
        })
    }

    fn renderer(&self) -> Option<Box<dyn ToolRenderer>> {
        Some(Box::new(self.renderer.clone()))
    }

    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
        cancel: Cancel,
        _on_update: Option<UnboundedSender<ToolOutput>>,
    ) -> anyhow::Result<ToolOutput> {
        let _ = tool_call_id;
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?
            .to_string();
        let edits: Vec<Edit> = serde_json::from_value(args["edits"].clone())
            .map_err(|e| anyhow::anyhow!("Invalid edits: {}", e))?;

        cancel.check()?;

        let cwd = self.cwd.clone();
        let path_for_queue = path_str.clone();
        let cwd_for_closure = cwd.clone();
        let edits_for_closure = edits.clone();
        let ops = self.operations.clone();

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

                cancel.check()?;

                // Check file accessibility using operations
                ops.access(&abs_path).await?;

                cancel.check()?;

                // Read file using operations
                let raw_content = ops.read_file(&abs_path).await?;

                cancel.check()?;

                // ── 1. BOM handling ──
                let (bom, content) = strip_bom(&raw_content);

                // ── 2. Line ending handling ──
                let original_ending = detect_line_ending(content);
                let normalized = normalize_to_lf(content);

                // ── 3-8. Apply edits and compute diff ──
                let (_base_content, new_content, diff) =
                    apply_edits_and_compute_diff(&normalized, &edits_for_closure, &path_str)
                        .map_err(|e| anyhow::anyhow!("{}", e))?;

                cancel.check()?;

                // ── 9. Write back with original line endings and BOM ──
                let final_content =
                    bom.to_string() + &restore_line_endings(&new_content, original_ending);
                ops.write_file(&abs_path, &final_content).await?;

                cancel.check()?;

                // ── 10. Compute firstChangedLine and patch ──
                let first_changed_line = extract_first_changed_line(&diff);
                let patch = generate_unified_patch(&path_str, &_base_content, &new_content);

                // ── 11. Return result ──
                let noun = if edits.len() == 1 { "block" } else { "blocks" };
                let msg = format!(
                    "Successfully replaced {} {} in {}.",
                    edits.len(),
                    noun,
                    path_str
                );
                let details = serde_json::json!({
                    "diff": diff.trim_end(),
                    "path": path_str,
                    "patch": patch,
                    "firstChangedLine": first_changed_line,
                });
                Ok::<_, anyhow::Error>((msg, details))
            },
        )
        .await?;

        let (msg, details) = output;
        Ok(ToolOutput::ok_with_details(msg, details))
    }
}

// ── Edit tool renderer (stateful, with preview) ─────────────────

/// Cached preview of what the edit will look like.
#[derive(Debug, Clone)]
struct EditPreview {
    diff: String,
    error: Option<String>,
}

/// Tool renderer for the `edit` tool.
/// Uses `renderShell: "self"` - renders its own framing without colored box.
/// Shows a preview of what will change in the call header (matching pi behavior).
#[derive(Clone)]
struct EditRenderer {
    /// Cached diff preview, computed from file system during render_call.
    /// Protected by Mutex for interior mutability in a Sync trait impl.
    preview: std::sync::Arc<Mutex<Option<EditPreview>>>,
}

impl EditRenderer {
    fn new() -> Self {
        Self {
            preview: std::sync::Arc::new(Mutex::new(None)),
        }
    }
}

impl ToolRenderer for EditRenderer {
    fn render_self(&self) -> bool {
        true
    }

    fn render_bg_key(&self) -> Option<&'static str> {
        // Match pi's edit tool background management:
        // - If preview exists and has an error → toolErrorBg
        // - If preview exists and is valid → toolSuccessBg (preview succeeded)
        // - If settled error (post-exec) → toolErrorBg
        // - Otherwise → toolPendingBg (no preview yet)
        if let Ok(p) = self.preview.lock()
            && let Some(ref preview) = *p
        {
            if preview.error.is_some() {
                return Some("toolErrorBg");
            }
            return Some("toolSuccessBg");
        }
        None // Let compute_bg_key use default (toolPendingBg)
    }

    fn render_call(
        &self,
        args: &serde_json::Value,
        _width: usize,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
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

        let header = format!(
            "{} {}",
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold("edit")),
            path_disp
        );

        let mut lines = vec![header];

        // Decide what diff to show:
        // 1. If execution completed and details are available, use actual diff from details
        // 2. Otherwise, if args are complete and we have a cached preview, show that
        let actual_diff = ctx
            .details
            .as_ref()
            .and_then(|d| d.get("diff"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let diff_to_show = if let Some(ref d) = actual_diff {
            Some(d.clone())
        } else if ctx.args_complete && !ctx.is_partial {
            // After execution, if details aren't in context (unlikely), fallback to preview
            self.preview.lock().ok().and_then(|p| {
                p.as_ref().map(|preview| {
                    if let Some(ref err) = preview.error {
                        format!("error: {}", err)
                    } else {
                        preview.diff.clone()
                    }
                })
            })
        } else if ctx.args_complete && actual_diff.is_none() {
            // Pending state: try to use cached preview or spawn async computation
            let cached = self.preview.lock().ok().and_then(|p| p.clone());

            if let Some(preview) = cached {
                if let Some(ref err) = preview.error {
                    Some(format!("error: {}", err))
                } else {
                    Some(preview.diff.clone())
                }
            } else if let Some((path_str, edits)) = parse_path_edits(args) {
                // If no cached preview, check if preview is already being computed
                // (pending flag not started yet). If not, spawn async computation.
                // First, check a "pending" flag to avoid duplicate spawns.
                let mut preview_lock = self.preview.lock().unwrap();
                if preview_lock.is_some() {
                    // Preview was set between lock releases (race)
                    drop(preview_lock);
                    let cached = self.preview.lock().ok().and_then(|p| p.clone());
                    cached.map(|preview| {
                        if let Some(ref err) = preview.error {
                            format!("error: {}", err)
                        } else {
                            preview.diff.clone()
                        }
                    })
                } else {
                    // Mark as pending (store a place-holder)
                    *preview_lock = Some(EditPreview {
                        diff: String::new(),
                        error: Some("pending".to_string()),
                    });
                    drop(preview_lock);

                    // Spawn async computation (matching pi's computeEditsDiff called from renderCall)
                    let preview_arc = self.preview.clone();
                    let path_owned = path_str.clone();
                    let edits_owned = edits.clone();
                    let cwd_owned = ctx.cwd.clone();
                    let invalidate_tx = ctx.invalidate.clone();
                    tokio::spawn(async move {
                        let result = compute_edits_diff(
                            &path_owned,
                            &edits_owned,
                            std::path::Path::new(&cwd_owned),
                        );
                        let (diff, error) = match result {
                            Ok(d) => (d, None),
                            Err(e) => (String::new(), Some(e)),
                        };
                        if let Ok(mut p) = preview_arc.lock() {
                            *p = Some(EditPreview { diff, error });
                        }
                        // Notify UI to re-render
                        if let Some(ref tx) = invalidate_tx {
                            let _ = tx.send(());
                        }
                    });

                    // No diff to show yet (pending)
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some(ref diff) = diff_to_show {
            if diff.starts_with("error: ") {
                // Show error inline (dimmed, matching pi error display)
                lines.push(String::new());
                lines.push(theme.fg_key(ThemeKey::Muted, diff));
            } else if !diff.is_empty() {
                lines.push(String::new());
                let rendered_lines = crate::tui::components::diff::render_diff(diff, theme);
                lines.extend(rendered_lines);
            }
        }

        lines
    }

    fn render_result(
        &self,
        _content: &str,
        _width: usize,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Vec<String> {
        // Result is already shown in the call header (via render_call using ctx.details).
        // If there's an error message not already shown, return it.
        if ctx.is_error {
            // Error case: content has the error text
            if !_content.is_empty() {
                let msg = _content;
                // Check if this error is already shown as preview
                let preview_err = self
                    .preview
                    .lock()
                    .ok()
                    .and_then(|p| p.as_ref().and_then(|preview| preview.error.clone()));
                if preview_err.as_deref() != Some(msg) {
                    return vec![String::new(), theme.fg_key(ThemeKey::Error, msg)];
                }
            }
        }

        Vec::new()
    }
}

// ── Diff utility functions ───────────────────────────────────────

/// Extract the first changed line number from a diff string.
/// Scans for the first `+` or `-` prefixed line with a line number.
fn extract_first_changed_line(diff: &str) -> Option<usize> {
    for line in diff.lines() {
        let bytes = line.as_bytes();
        if bytes.is_empty() {
            continue;
        }
        let prefix = bytes[0] as char;
        if prefix != '+' && prefix != '-' {
            continue;
        }
        // Parse the line number from the rest
        let rest = &line[1..];
        let num_str: String = rest
            .chars()
            .take_while(|c| c.is_whitespace() || c.is_ascii_digit())
            .collect();
        if let Ok(num) = num_str.trim().parse::<usize>() {
            return Some(num);
        }
    }
    None
}

/// Generate a unified diff patch string from original and modified content.
/// Uses basic hunk structure matching pi's `generateUnifiedPatch`.
fn generate_unified_patch(path: &str, original: &str, modified: &str) -> String {
    let orig_lines: Vec<&str> = original.lines().collect();
    let mod_lines: Vec<&str> = modified.lines().collect();

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

    // Group into hunks
    const CTX: usize = 3;
    let mut hunks: Vec<String> = Vec::new();
    let mut pos = 0;

    while pos < changes.len() {
        while pos < changes.len() && changes[pos].0 == ' ' {
            pos += 1;
        }
        if pos >= changes.len() {
            break;
        }

        let hunk_start = pos.saturating_sub(CTX);
        let hunk_end = (pos + 3 * CTX).min(changes.len());

        // Compute old/new line ranges
        let mut old_line = 1usize;
        let mut new_line = 1usize;
        for (tag, _) in changes.iter().take(pos.saturating_sub(CTX)) {
            match tag {
                ' ' => {
                    old_line += 1;
                    new_line += 1;
                }
                '-' => old_line += 1,
                '+' => new_line += 1,
                _ => {}
            }
        }

        let old_start = old_line;
        let new_start = new_line;

        // Count hunk size
        let mut old_count = 0usize;
        let mut new_count = 0usize;
        for (tag, _) in changes[hunk_start..hunk_end].iter() {
            match tag {
                ' ' => {
                    old_count += 1;
                    new_count += 1;
                }
                '-' => old_count += 1,
                '+' => new_count += 1,
                _ => {}
            }
        }

        let mut hunk = format!(
            "@@ -{},{} +{},{} @@\n",
            old_start, old_count, new_start, new_count
        );

        for (tag, text) in changes[hunk_start..hunk_end].iter() {
            match tag {
                ' ' => hunk.push_str(&format!(" {}", text)),
                '-' => hunk.push_str(&format!("-{}", text)),
                '+' => hunk.push_str(&format!("+{}", text)),
                _ => {}
            }
            hunk.push('\n');
        }

        hunks.push(hunk);
        pos = hunk_end;
    }

    if hunks.is_empty() {
        return String::new();
    }

    let mut patch = format!("--- a/{}\n+++ b/{}\n", path, path);
    for hunk in &hunks {
        patch.push_str(hunk);
    }

    patch
}

// ═══════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════

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
        let tool = EditTool {
            cwd: tmp.clone(),
            renderer: EditRenderer::new(),
            operations: Arc::new(DefaultEditOperations),
        };
        (tool, tmp)
    }

    async fn exec_ok(tool: &EditTool, args: serde_json::Value) -> String {
        let args = tool.prepare_arguments(args);
        tool.execute("id".into(), args, Cancel::new(), None)
            .await
            .unwrap()
            .content
    }

    async fn exec_ok_details(
        tool: &EditTool,
        args: serde_json::Value,
    ) -> (String, Option<serde_json::Value>) {
        let args = tool.prepare_arguments(args);
        let result = tool
            .execute("id".into(), args, Cancel::new(), None)
            .await
            .unwrap();
        (result.content, result.details)
    }

    async fn exec_err(tool: &EditTool, args: serde_json::Value) -> String {
        let args = tool.prepare_arguments(args);
        tool.execute("id".into(), args, Cancel::new(), None)
            .await
            .unwrap_err()
            .to_string()
    }

    async fn is_err(tool: &EditTool, args: serde_json::Value) -> bool {
        let args = tool.prepare_arguments(args);
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
        // Pi behavior: exact match ("hello world" is a substring of "hello world  "),
        // so trailing whitespace on unchanged suffix is preserved.
        assert_eq!(content, "hi there  \nnext line\n");
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

    // ── No-change detection ──────────────────────────────────

    #[tokio::test]
    async fn no_change_identical_edit_errors() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("nochange.txt");
        std::fs::write(&path, "hello\nworld\n").unwrap();

        let err = exec_err(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "hello", "newText": "hello"}]
            }),
        )
        .await;
        assert!(
            err.contains("No changes made"),
            "expected no-change error but got: {}",
            err
        );
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

    // ── Structured details (diff no longer embedded in content) ──

    #[tokio::test]
    async fn result_content_has_no_diff_block() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("diff_test.txt");
        std::fs::write(&path, "aaa\nbbb\nccc\n").unwrap();

        let (content, details) = exec_ok_details(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "bbb", "newText": "xxx"}]
            }),
        )
        .await;

        // Content should NOT contain a ```diff block anymore
        assert!(
            !content.contains("```diff"),
            "content should not contain diff block, got: {}",
            content
        );
        assert!(content.contains("Successfully replaced 1 block"));

        // Diff should be in details
        let details_obj = details.expect("details should be present");
        let diff = details_obj
            .get("diff")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            diff.contains("-2 bbb"),
            "diff should contain '-2 bbb' but got: {}",
            diff
        );
        assert!(
            diff.contains("+2 xxx"),
            "diff should contain '+2 xxx' but got: {}",
            diff
        );
    }

    // ── Fuzzy matching preserves unchanged lines (using new line-span mapping) ──

    #[tokio::test]
    async fn fuzzy_preserves_unchanged_line_trailing_whitespace() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("fuzzy_preserve.txt");
        // First line has trailing spaces, second has smart quotes (forces fuzzy)
        std::fs::write(&path, "keep this line  \nchange \u{201C}this\u{201D}\n").unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "change \"this\"", "newText": "changed"}]
            }),
        )
        .await;

        let content = std::fs::read_to_string(&path).unwrap();
        // Unchanged first line preserves trailing spaces (pi behavior)
        assert!(
            content.starts_with("keep this line  "),
            "expected preserved trailing spaces but got: {:?}",
            content
        );
        assert!(content.contains("changed\n"), "got: {:?}", content);
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

    // ── NFKC normalization test ─────────────────────────────

    #[tokio::test]
    async fn fuzzy_match_nfkc_composed_vs_decomposed() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("nfkc.txt");
        // "café" in NFD (decomposed): cafe + combining acute accent
        let nfd: String = "cafe\u{0301}".chars().collect();
        std::fs::write(&path, format!("{} rest\n", nfd)).unwrap();

        exec_ok(
            &tool,
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "café", "newText": "changed"}]
            }),
        )
        .await;

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.starts_with("changed"),
            "expected 'changed' but got: {:?}",
            content
        );
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

    #[test]
    fn test_nfkc_normalization() {
        // é composed (NFC) vs decomposed (NFD) + NFKC
        let composed = "café";
        let decomposed: String = "cafe\u{0301}".chars().collect();
        assert_eq!(
            normalize_for_fuzzy_match(composed),
            normalize_for_fuzzy_match(&decomposed),
            "NFKC should make composed and decomposed café match"
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

    #[test]
    fn test_apply_replacements_preserving_unchanged_lines() {
        let original = "keep this  \nchange this\nkeep that  \n";
        let base = "keep this\nchange this\nkeep that\n";
        // matchIndex 10, matchLength 11 covers "change this" (bytes 10..21 in base)
        let replacements = vec![(10usize, 11usize, "modified")];
        let result = apply_replacements_preserving_unchanged_lines(original, base, &replacements);
        assert_eq!(result, "keep this  \nmodified\nkeep that  \n");
    }
}
