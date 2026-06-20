use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use anyhow::Context;
use async_trait::async_trait;
use std::borrow::Cow;
use std::path::Path;
use tokio::sync::mpsc::UnboundedSender;

pub struct ReadExtension {
    cwd: std::path::PathBuf,
}

impl ReadExtension {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for ReadExtension {
    fn name(&self) -> Cow<'static, str> {
        "read".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(ReadTool {
            cwd: self.cwd.clone(),
        })]
    }
}

struct ReadTool {
    cwd: std::path::PathBuf,
}

// ── Constants ────────────────────────────────────────────────────

const DEFAULT_MAX_LINES: usize = 2000;
const DEFAULT_MAX_BYTES: usize = 50 * 1024; // 50KB

// ── Helpers ──────────────────────────────────────────────────────

/// Format bytes as a human-readable size string, matching pi's format.
fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Trim trailing empty lines from a slice of lines.
fn trim_trailing_empty_lines<'a>(lines: &'a [&'a str]) -> &'a [&'a str] {
    let mut end = lines.len();
    while end > 0 && lines[end - 1].is_empty() {
        end -= 1;
    }
    &lines[..end]
}

/// Build a compact label for the read tool output, matching pi's compact mode.
/// Returns `None` for regular files.
fn compact_read_label(path: &str, cwd: &Path, start_line: usize) -> Option<String> {
    let abs_path = if Path::new(path).is_absolute() {
        Path::new(path).to_path_buf()
    } else {
        cwd.join(path)
    };

    let file_name = abs_path.file_name()?.to_str()?;

    // AGENTS.md / CLAUDE.md → "read resource <path>"
    if file_name.eq_ignore_ascii_case("AGENTS.md") || file_name.eq_ignore_ascii_case("CLAUDE.md") {
        let display = abs_path
            .strip_prefix(cwd)
            .unwrap_or(&abs_path)
            .to_string_lossy()
            .to_string();
        let range = if start_line > 0 {
            format!(":{}", start_line + 1)
        } else {
            String::new()
        };
        return Some(format!("read resource {}{}", display, range));
    }

    // SKILL.md → "read skill <dirname>"
    if file_name == "SKILL.md"
        && let Some(parent) = abs_path.parent()
        && let Some(dir_name) = parent.file_name()
    {
        let dir_name = dir_name.to_str().unwrap_or("unknown");
        let range = if start_line > 0 {
            format!(":{}", start_line + 1)
        } else {
            String::new()
        };
        return Some(format!("read skill {}{}", dir_name, range));
    }

    None
}

// ── Truncation ──────────────────────────────────────────────────

/// Truncation result, mirroring pi's `TruncationResult`.
#[allow(dead_code)]
struct TruncationResult {
    content: String,
    truncated: bool,
    truncated_by: &'static str, // "lines" | "bytes"
    total_lines: usize,
    output_lines: usize,
    first_line_exceeds_limit: bool,
}

/// Truncate content from the head, keeping complete lines that fit within limits.
/// Never returns partial lines. If first line exceeds the byte limit,
/// returns empty content with `first_line_exceeds_limit = true`.
fn truncate_head(content: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let total_bytes = content.len();
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Check if no truncation needed
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: "",
            total_lines,
            output_lines: total_lines,
            first_line_exceeds_limit: false,
        };
    }

    // Check if first line alone exceeds the byte limit
    if let Some(first) = lines.first()
        && first.len() > max_bytes
    {
        return TruncationResult {
            content: String::new(),
            truncated: true,
            truncated_by: "bytes",
            total_lines,
            output_lines: 0,
            first_line_exceeds_limit: true,
        };
    }

    // Accumulate complete lines that fit within both limits
    let mut output: Vec<&str> = Vec::new();
    let mut byte_count: usize = 0;
    let mut truncated_by = "lines";

    for line in lines.iter().take(max_lines) {
        let line_bytes = line.len();
        let with_newline = if output.is_empty() {
            line_bytes
        } else {
            line_bytes + 1 // +1 for the preceding newline
        };

        if byte_count + with_newline > max_bytes {
            truncated_by = "bytes";
            break;
        }

        output.push(line);
        byte_count += with_newline;
    }

    if output.len() >= max_lines && byte_count <= max_bytes {
        truncated_by = "lines";
    }

    TruncationResult {
        content: output.join("\n"),
        truncated: true,
        truncated_by,
        total_lines,
        output_lines: output.len(),
        first_line_exceeds_limit: false,
    }
}

// ── AgentTool implementation ─────────────────────────────────────

#[async_trait]
impl AgentTool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Supports text files and images (jpg, png, gif, webp). \
         Images are sent as attachments. For text files, output is truncated to 2000 lines or \
         50KB (whichever is hit first). Use offset/limit for large files. When you need the \
         full file, continue with offset until complete."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (relative or absolute)"
                },
                "offset": {
                    "type": "number",
                    "description": "Line number to start reading from (1-indexed)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of lines to read"
                }
            }
        })
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        vec!["Use read to examine files instead of cat or sed.".into()]
    }

    fn label(&self) -> &str {
        "Read file contents"
    }

    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
        cancel: Cancel,
        _on_update: Option<UnboundedSender<ToolOutput>>,
    ) -> anyhow::Result<ToolOutput> {
        let _ = tool_call_id;
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;
        let offset = args["offset"].as_u64().map(|o| o as usize).unwrap_or(0);
        let limit = args["limit"].as_u64().map(|l| l as usize);

        let abs_path = {
            let p = std::path::Path::new(path);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                self.cwd.join(p)
            }
        };

        cancel.check()?;

        let content = std::fs::read_to_string(&abs_path)
            .with_context(|| format!("Failed to read {}", abs_path.display()))?;

        let all_lines: Vec<&str> = content.split('\n').collect();
        let total_file_lines = if content.ends_with('\n') {
            all_lines.len() - 1
        } else {
            all_lines.len()
        };

        // Apply offset (1-indexed → 0-indexed)
        let start_line = if offset > 0 { offset - 1 } else { 0 };
        if start_line >= total_file_lines {
            return Err(anyhow::anyhow!(
                "Offset {} is beyond end of file ({} lines total)",
                offset,
                total_file_lines
            ));
        }

        cancel.check()?;

        // Build the selected content based on offset/limit
        let selected_content: String;
        let user_limited_lines: Option<usize>;

        if let Some(lim) = limit {
            let end_line = (start_line + lim).min(total_file_lines);
            let selected_lines = &all_lines[start_line..end_line];
            selected_content = selected_lines.join("\n");
            user_limited_lines = Some(end_line - start_line);
        } else {
            let selected_lines = &all_lines[start_line..];
            selected_content = selected_lines.join("\n");
            user_limited_lines = None;
        }

        // Compute compact label before truncation (for UI rendering)
        let compact = compact_read_label(path, &self.cwd, start_line);

        // Apply truncation
        let trunc = truncate_head(&selected_content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);

        if trunc.first_line_exceeds_limit {
            let first_line_bytes = format_size(all_lines[start_line].len());
            let msg = format!(
                "[Line {} is {}, exceeds {} limit. Use bash: sed -n '{}p' {} | head -c {}]",
                start_line + 1,
                first_line_bytes,
                format_size(DEFAULT_MAX_BYTES),
                start_line + 1,
                path,
                DEFAULT_MAX_BYTES,
            );
            return Ok(ToolOutput::ok(msg));
        }

        let output: String;

        if trunc.truncated {
            let start_display = start_line + 1;
            let end_display = start_display + trunc.output_lines - 1;
            let next_offset = end_display + 1;

            if trunc.truncated_by == "lines" {
                output = format!(
                    "{}\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                    trunc.content, start_display, end_display, total_file_lines, next_offset,
                );
            } else {
                output = format!(
                    "{}\n\n[Showing lines {}-{} of {} ({} limit). Use offset={} to continue.]",
                    trunc.content,
                    start_display,
                    end_display,
                    total_file_lines,
                    format_size(DEFAULT_MAX_BYTES),
                    next_offset,
                );
            }
        } else if let Some(ul) = user_limited_lines {
            if start_line + ul < total_file_lines {
                let remaining = total_file_lines - (start_line + ul);
                let next_offset = start_line + ul + 1;
                output = format!(
                    "{}\n\n[{} more lines in file. Use offset={} to continue.]",
                    trunc.content, remaining, next_offset,
                );
            } else {
                let lines: Vec<&str> = trunc.content.lines().collect();
                let trimmed = trim_trailing_empty_lines(&lines);
                output = trimmed.join("\n");
            }
        } else {
            let lines: Vec<&str> = trunc.content.lines().collect();
            let trimmed = trim_trailing_empty_lines(&lines);
            output = trimmed.join("\n");
        }

        if let Some(label) = compact {
            Ok(ToolOutput::ok_with_compact(output, label))
        } else {
            Ok(ToolOutput::ok(output))
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rab-read-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_tool() -> (ReadTool, std::path::PathBuf) {
        let tmp = tmp_dir();
        (ReadTool { cwd: tmp.clone() }, tmp)
    }

    async fn exec_ok(tool: &ReadTool, args: serde_json::Value) -> String {
        tool.execute("id".into(), args, Cancel::new(), None)
            .await
            .unwrap()
            .content
    }

    async fn exec_full(tool: &ReadTool, args: serde_json::Value) -> ToolOutput {
        tool.execute("id".into(), args, Cancel::new(), None)
            .await
            .unwrap()
    }

    // ── Truncation unit tests ─────────────────────────────────

    #[test]
    fn test_no_truncation_needed() {
        let result = truncate_head("hello\nworld\n", 2000, 50000);
        assert!(!result.truncated);
        assert!(!result.first_line_exceeds_limit);
        assert_eq!(result.content, "hello\nworld\n");
    }

    #[test]
    fn test_truncates_by_lines() {
        let content: String = (1..=5000).map(|i| format!("line {}\n", i)).collect();
        let result = truncate_head(&content, 2000, 50000);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, "lines");
        assert_eq!(result.output_lines, 2000);
        assert!(result.content.ends_with("line 2000"));
    }

    #[test]
    fn test_truncates_by_bytes() {
        let content: String = (1..=100)
            .map(|i| format!("line {} {}\n", i, "x".repeat(1000)))
            .collect();
        let result = truncate_head(&content, 2000, 50000);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, "bytes");
        assert!(result.output_lines < 100);
    }

    #[test]
    fn test_first_line_exceeds_limit() {
        let content = format!("{}\nshort\n", "x".repeat(60000));
        let result = truncate_head(&content, 2000, 50000);
        assert!(result.truncated);
        assert!(result.first_line_exceeds_limit);
        assert!(result.content.is_empty());
    }

    #[test]
    fn test_empty_content() {
        let result = truncate_head("", 2000, 50000);
        assert!(!result.truncated);
        assert_eq!(result.content, "");
    }

    #[test]
    fn test_exact_fit() {
        let line = "a".repeat(50000);
        let result = truncate_head(&line, 2000, 50000);
        assert!(!result.truncated);
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500B");
        assert_eq!(format_size(1024), "1.0KB");
        assert_eq!(format_size(50 * 1024), "50.0KB");
        assert_eq!(format_size(1024 * 1024), "1.0MB");
    }

    #[test]
    fn test_trim_trailing_empty_lines() {
        let lines = vec!["a", "b", "", ""];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert_eq!(trimmed, &["a", "b"]);
    }

    #[test]
    fn test_trim_no_trailing_empty_lines() {
        let lines = vec!["a", "b"];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert_eq!(trimmed, &["a", "b"]);
    }

    #[test]
    fn test_trim_all_empty() {
        let lines: Vec<&str> = vec!["", "", ""];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert!(trimmed.is_empty());
    }

    #[test]
    fn test_trim_empty_input() {
        let lines: Vec<&str> = vec![];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert!(trimmed.is_empty());
    }

    // ── Compact label tests ──────────────────────────────────

    #[test]
    fn test_compact_label_agents_md() {
        let label = compact_read_label("path/to/AGENTS.md", Path::new("path"), 0);
        assert!(label.is_some());
        let l = label.unwrap();
        assert!(l.contains("read resource"));
        assert!(l.contains("to/AGENTS.md"));
    }

    #[test]
    fn test_compact_label_claude_md() {
        let label = compact_read_label("path/CLAUDE.md", Path::new("path"), 0);
        assert!(label.is_some());
        let l = label.unwrap();
        assert!(l.contains("read resource"));
    }

    #[test]
    fn test_compact_label_skill() {
        let label = compact_read_label("skills/my-skill/SKILL.md", Path::new("."), 0);
        assert!(label.is_some());
        let l = label.unwrap();
        assert!(l.contains("read skill"));
        assert!(l.contains("my-skill"));
    }

    #[test]
    fn test_compact_label_regular_file() {
        let label = compact_read_label("src/main.rs", Path::new("."), 0);
        assert!(label.is_none());
    }

    // ── Integration tests ────────────────────────────────────

    #[tokio::test]
    async fn reads_file_content() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("test.txt");
        std::fs::write(&path, "hello world\nline two\n").unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(result.contains("hello world"));
        assert!(result.contains("line two"));
    }

    #[tokio::test]
    async fn read_respects_offset() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("test.txt");
        let content: Vec<String> = (1..=10).map(|i| format!("line {}", i)).collect();
        std::fs::write(&path, content.join("\n")).unwrap();

        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "offset": 5}),
        )
        .await;

        assert!(result.contains("line 5"), "should contain line 5: {result}");
        assert!(
            !result.lines().any(|l| l == "line 1"),
            "should not contain line 1: {result}"
        );
    }

    #[tokio::test]
    async fn read_respects_limit() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("test.txt");
        let content: Vec<String> = (1..=10).map(|i| format!("line {}", i)).collect();
        std::fs::write(&path, content.join("\n")).unwrap();

        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "offset": 1, "limit": 3}),
        )
        .await;

        assert!(result.contains("line 1"));
        assert!(result.contains("line 3"));
        assert!(!result.contains("line 4"));
    }

    #[tokio::test]
    async fn read_nonexistent_file_errors() {
        let (tool, _tmp) = make_tool();

        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"path": "nonexistent.txt"}),
                Cancel::new(),
                None,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn offset_beyond_end_errors() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("short.txt");
        std::fs::write(&path, "only one line\n").unwrap();

        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"path": path.to_str().unwrap(), "offset": 100}),
                Cancel::new(),
                None,
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("beyond end of file"));
    }

    #[tokio::test]
    async fn large_file_truncation_by_lines() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("large.txt");
        let content: String = (1..=5000).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(&path, &content).unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(result.contains("Showing lines 1-"));
        assert!(result.contains("offset="));
        assert!(result.contains("of 5000."));
    }

    #[tokio::test]
    async fn large_file_truncation_by_bytes() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("wide.txt");
        let content: String = (1..=100)
            .map(|i| format!("line {} {}\n", i, "x".repeat(1190)))
            .collect();
        std::fs::write(&path, &content).unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(result.contains("KB limit"));
        assert!(result.contains("offset="));
    }

    #[tokio::test]
    async fn first_line_exceeds_limit_shows_bash_hint() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("huge_first_line.txt");
        let content = format!("{}\nshort line\n", "x".repeat(60000));
        std::fs::write(&path, &content).unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(result.contains("bash"));
        assert!(result.contains("sed"));
        assert!(result.contains("head -c"));
    }

    #[tokio::test]
    async fn limit_honored_without_truncation() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("limited.txt");
        let content: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(&path, &content).unwrap();

        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "limit": 5}),
        )
        .await;

        assert!(result.contains("line 1"));
        assert!(result.contains("line 5"));
        assert!(!result.contains("line 6"));
        assert!(result.contains("more lines"));
    }

    #[tokio::test]
    async fn limit_exactly_covers_file() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("exact.txt");
        let content: String = (1..=3).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(&path, &content).unwrap();

        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "limit": 3}),
        )
        .await;

        assert!(result.contains("line 1"));
        assert!(result.contains("line 2"));
        assert!(result.contains("line 3"));
        assert!(!result.contains("more lines"));
    }

    #[tokio::test]
    async fn trims_trailing_empty_lines() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("trailing_empties.txt");
        std::fs::write(&path, "hello\nworld\n\n\n").unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(result.contains("hello"));
        assert!(result.contains("world"));
        assert!(!result.ends_with("\n\n\n"));
    }

    #[tokio::test]
    async fn relative_path_resolves_to_cwd() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("relative.txt");
        std::fs::write(&path, "hello\n").unwrap();

        let result = exec_ok(&tool, serde_json::json!({"path": "relative.txt"})).await;

        assert!(result.contains("hello"));
    }

    #[tokio::test]
    async fn compact_label_for_agents_md() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("AGENTS.md");
        std::fs::write(&path, "some instructions\n").unwrap();

        let output = exec_full(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(output.compact.is_some());
        let label = output.compact.unwrap();
        assert!(label.contains("read resource"));
        assert!(label.contains("AGENTS.md"));
    }

    #[tokio::test]
    async fn no_compact_label_for_regular_file() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();

        let output = exec_full(&tool, serde_json::json!({"path": path.to_str().unwrap()})).await;

        assert!(output.compact.is_none());
    }

    #[tokio::test]
    async fn cancel_aborts_read() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("cancel_test.txt");
        std::fs::write(&path, "hello\n").unwrap();

        let cancel = Cancel::new();
        cancel.cancel();

        let result = tool
            .execute(
                "id".into(),
                serde_json::json!({"path": path.to_str().unwrap()}),
                cancel,
                None,
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cancell") || err.contains("Cancel"));
    }
}
