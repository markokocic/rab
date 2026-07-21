use crate::agent::extension::ToolDefinition;
use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::builtin;
use crate::tui::Style;
use crate::tui::components::StyledSegment;
use crate::tui::{Component, Theme, ThemeKey};

use std::path::Path;
use std::sync::Arc;

/// Normalize and coerce write tool arguments.
/// Ensures `path` and `content` are strings, coercing numbers/bools/null as needed.
pub fn prepare_write_args(mut args: serde_json::Value) -> Result<serde_json::Value, String> {
    // Coerce path to string if possible
    if let Some(val) = args.get("path")
        && !val.is_string()
    {
        if val.is_number() || val.is_boolean() {
            args["path"] = serde_json::Value::String(match val {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => unreachable!(),
            });
        } else if val.is_null() {
            return Err("Missing 'path' argument".to_string());
        }
    }

    // Coerce content to string if possible
    if let Some(val) = args.get("content")
        && !val.is_string()
    {
        if val.is_number() || val.is_boolean() || val.is_null() {
            args["content"] = serde_json::Value::String(match val {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Null => String::new(),
                _ => unreachable!(),
            });
        } else if val.is_array() || val.is_object() {
            // Stringify arrays/objects when schema expects string
            args["content"] =
                serde_json::Value::String(serde_json::to_string(val).unwrap_or_default());
        }
    }

    Ok(args)
}

/// Number of preview lines when collapsed (matching pi's PREVIEW_LINES).
const PREVIEW_LINES: usize = 10;

/// Number of lines at the start to re-highlight with full multi-line context
/// when content grows incrementally (matching pi's WRITE_PARTIAL_FULL_HIGHLIGHT_LINES).
const PARTIAL_FULL_HIGHLIGHT_LINES: usize = 50;

// ── WriteOperations (pluggable) ───────────────────────────────────

/// Pluggable operations for the write tool (matching pi's WriteOperations).
/// Override these to delegate file writing to remote systems (for example SSH).
pub trait WriteOperations: Send + Sync {
    /// Write content to a file.
    fn write_file(&self, absolute_path: &Path, content: &str) -> anyhow::Result<()>;
    /// Create directory recursively.
    fn mkdir(&self, dir: &Path) -> anyhow::Result<()>;
}

impl<F1, F2> WriteOperations for (F1, F2)
where
    F1: Send + Sync + Fn(&Path, &str) -> anyhow::Result<()>,
    F2: Send + Sync + Fn(&Path) -> anyhow::Result<()>,
{
    fn write_file(&self, absolute_path: &Path, content: &str) -> anyhow::Result<()> {
        (self.0)(absolute_path, content)
    }
    fn mkdir(&self, dir: &Path) -> anyhow::Result<()> {
        (self.1)(dir)
    }
}

pub(crate) struct DefaultWriteOperations;

impl WriteOperations for DefaultWriteOperations {
    fn write_file(&self, absolute_path: &Path, content: &str) -> anyhow::Result<()> {
        Ok(std::fs::write(absolute_path, content)?)
    }
    fn mkdir(&self, dir: &Path) -> anyhow::Result<()> {
        Ok(std::fs::create_dir_all(dir)?)
    }
}

// ── Extension ─────────────────────────────────────────────────────

/// Create a ToolDefinition for the write tool.
pub(crate) fn make_write_tool(
    cwd: std::path::PathBuf,
    operations: Arc<dyn WriteOperations>,
) -> ToolDefinition {
    ToolDefinition {
        tool: Box::new(WriteTool { cwd, operations }),
        snippet: "Create or overwrite files",
        guidelines: &["Use write only for new files or complete rewrites."],
        prepare_arguments: Some(prepare_write_args),
        before_tool_call: None,
        after_tool_call: None,
        renderer: Some(std::sync::Arc::new(WriteRenderer::new())),
    }
}

// ── Tool ─────────────────────────────────────────────────────────

struct WriteTool {
    cwd: std::path::PathBuf,
    operations: Arc<dyn WriteOperations>,
}

// ── Incremental highlight cache ──────────────────────────────────

/// Cached highlighted lines for a write tool call, matching pi's WriteHighlightCache.
/// Supports incremental updates when new content extends old content.
struct WriteHighlightCache {
    raw_path: Option<String>,
    lang: String,
    raw_content: String,
    normalized_lines: Vec<String>,
    highlighted_lines: Vec<String>,
}

/// Highlight a single line (uses full highlight on the single line, returns first result).
fn highlight_single_line(line: &str, lang: &str) -> String {
    #[cfg(feature = "syntect")]
    {
        let hl = crate::tui::components::highlight_code(line, Some(lang));
        if !hl.is_empty() {
            return hl[0].clone();
        }
    }
    line.to_string()
}

/// Re-highlight the first PARTIAL_FULL_HIGHLIGHT_LINES with full multi-line context.
/// Lines beyond that only get single-line highlight (performance optimization).
fn refresh_highlight_prefix(cache: &mut WriteHighlightCache) {
    let prefix_count = PARTIAL_FULL_HIGHLIGHT_LINES.min(cache.normalized_lines.len());
    if prefix_count == 0 {
        return;
    }
    let prefix_source: Vec<&str> = cache.normalized_lines[..prefix_count]
        .iter()
        .map(|s| s.as_str())
        .collect();
    let prefix_text = prefix_source.join("\n");
    #[cfg(feature = "syntect")]
    {
        let prefix_highlighted =
            crate::tui::components::highlight_code(&prefix_text, Some(&cache.lang));
        for i in 0..prefix_count {
            cache.highlighted_lines[i] = prefix_highlighted
                .get(i)
                .cloned()
                .unwrap_or_else(|| highlight_single_line(&cache.normalized_lines[i], &cache.lang));
        }
    }
    #[cfg(not(feature = "syntect"))]
    {
        let _ = prefix_text;
        for i in 0..prefix_count {
            cache.highlighted_lines[i] = cache.normalized_lines[i].clone();
        }
    }
}

/// Rebuild the highlight cache from scratch (full recompute).
fn rebuild_highlight_cache(
    raw_path: Option<&str>,
    file_content: &str,
) -> Option<WriteHighlightCache> {
    let lang = raw_path
        .and_then(crate::tui::components::path_to_language)
        .map(|s| s.to_string());
    let lang = lang?;

    let display_content = file_content.replace('\r', "");
    let normalized = display_content.replace('\t', "   ");
    let normalized_lines: Vec<String> = normalized.lines().map(|l| l.to_string()).collect();

    #[cfg(feature = "syntect")]
    let highlighted_lines = crate::tui::components::highlight_code(&normalized, Some(&lang));
    #[cfg(not(feature = "syntect"))]
    let highlighted_lines = normalized_lines.clone();

    Some(WriteHighlightCache {
        raw_path: raw_path.map(|s| s.to_string()),
        lang,
        raw_content: file_content.to_string(),
        normalized_lines,
        highlighted_lines,
    })
}

/// Incrementally update the highlight cache when new content extends old.
/// Matching pi's `updateWriteHighlightCacheIncremental`.
fn update_highlight_cache_incremental(
    cache: Option<WriteHighlightCache>,
    raw_path: Option<&str>,
    file_content: &str,
) -> Option<WriteHighlightCache> {
    let lang = raw_path
        .and_then(crate::tui::components::path_to_language)
        .map(|s| s.to_string());
    let lang = lang?;

    let mut cache = match cache {
        Some(c) => c,
        None => return rebuild_highlight_cache(raw_path, file_content),
    };

    // If lang or path changed, rebuild from scratch
    if cache.lang != lang || cache.raw_path.as_deref() != raw_path {
        return rebuild_highlight_cache(raw_path, file_content);
    }

    // If new content doesn't start with old content, rebuild
    if !file_content.starts_with(&cache.raw_content) {
        return rebuild_highlight_cache(raw_path, file_content);
    }

    // If content length is the same, no update needed
    if file_content.len() == cache.raw_content.len() {
        return Some(cache);
    }

    // Incremental: append delta
    let delta_raw = &file_content[cache.raw_content.len()..];
    let delta_display = delta_raw.replace('\r', "");
    let delta_normalized = delta_display.replace('\t', "   ");

    cache.raw_content = file_content.to_string();

    if cache.normalized_lines.is_empty() {
        cache.normalized_lines.push(String::new());
        cache.highlighted_lines.push(String::new());
    }

    let segments: Vec<&str> = delta_normalized.split('\n').collect();
    if segments.is_empty() {
        return Some(cache);
    }

    // First segment appends to the last existing line (delta may start mid-line)
    let last_idx = cache.normalized_lines.len() - 1;
    cache.normalized_lines[last_idx].push_str(segments[0]);
    cache.highlighted_lines[last_idx] =
        highlight_single_line(&cache.normalized_lines[last_idx], &cache.lang);

    // Subsequent segments become new lines
    for &seg in &segments[1..] {
        cache.normalized_lines.push(seg.to_string());
        cache
            .highlighted_lines
            .push(highlight_single_line(seg, &cache.lang));
    }

    // Re-highlight the prefix with full multi-line context
    refresh_highlight_prefix(&mut cache);

    Some(cache)
}

/// Trim trailing empty lines from a slice.
fn trim_trailing_empty_lines(lines: &[String]) -> &[String] {
    let mut end = lines.len();
    while end > 0 && lines[end - 1].is_empty() {
        end -= 1;
    }
    &lines[..end]
}

#[async_trait::async_trait]
impl yoagent::types::AgentTool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }
    fn label(&self) -> &str {
        "write"
    }
    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. \
         Automatically creates parent directories."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            }
        })
    }
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: yoagent::types::ToolContext,
    ) -> std::result::Result<yoagent::types::ToolResult, yoagent::types::ToolError> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| {
                yoagent::types::ToolError::InvalidArgs("Missing 'path' argument".into())
            })?
            .to_string();
        let content = params["content"]
            .as_str()
            .ok_or_else(|| {
                yoagent::types::ToolError::InvalidArgs("Missing 'content' argument".into())
            })?
            .to_string();

        if ctx.cancel.is_cancelled() {
            return Err(yoagent::types::ToolError::Cancelled);
        }

        let cwd = self.cwd.clone();
        let cancel = ctx.cancel.clone();
        let ops = self.operations.clone();
        let path_for_queue = path.clone();
        let cwd_for_closure = cwd.clone();
        let content_for_closure = content.clone();

        let result = crate::builtin::file_mutation_queue::with_file_mutation_queue(
            &path_for_queue,
            &cwd,
            || async move {
                let abs_path = builtin::resolve_path(&path, &cwd_for_closure);

                // Create parent directories
                if let Some(parent) = abs_path.parent() {
                    ops.mkdir(parent).map_err(|e| {
                        anyhow::anyhow!("Failed to create dir {}: {}", parent.display(), e)
                    })?;
                }

                if cancel.is_cancelled() {
                    anyhow::bail!("Operation cancelled");
                }

                // Write file using pluggable operations
                ops.write_file(&abs_path, &content_for_closure)
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to write {}: {}", abs_path.display(), e)
                    })?;

                Ok::<_, anyhow::Error>(format!(
                    "Successfully wrote {} bytes to {}",
                    content_for_closure.len(),
                    path
                ))
            },
        )
        .await
        .map_err(|e| yoagent::types::ToolError::Failed(e.to_string()))?;

        Ok(yoagent::types::ToolResult {
            content: vec![yoagent::types::Content::Text { text: result }],
            details: serde_json::Value::Null,
        })
    }
}

// ── Renderer ─────────────────────────────────────────────────────

/// Tool renderer for the `write` tool.
/// Shows the file path (with hyperlink) and a content preview in the call,
/// empty result on success. Includes incremental streaming highlight cache.
struct WriteRenderer {
    cache: std::sync::Mutex<Option<WriteHighlightCache>>,
}

impl WriteRenderer {
    fn new() -> Self {
        Self {
            cache: std::sync::Mutex::new(None),
        }
    }
}

impl ToolRenderer for WriteRenderer {
    fn render_call(
        &self,
        args: &serde_json::Value,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Box<dyn Component> {
        let raw_path = args
            .get("file_path")
            .or_else(|| args.get("path"))
            .and_then(|v| v.as_str());
        let content = args.get("content");

        let mut segments = Vec::new();

        // ── Header: "write" in toolTitle bold ──
        segments.push(StyledSegment {
            text: "write".to_string(),
            style: Some(
                Style::new()
                    .fg(theme.fg_ansi_key(ThemeKey::ToolTitle).to_string())
                    .bold(),
            ),
        });

        // ── Path display with hyperlink ──
        if let Some(p) = raw_path {
            let short = builtin::shorten_path(p);
            let cwd = if ctx.cwd.is_empty() {
                std::path::Path::new(".")
            } else {
                std::path::Path::new(&ctx.cwd)
            };
            let path_text = format!(" {}", short);
            let linked = builtin::link_path(&path_text, p, cwd);
            segments.push(StyledSegment {
                text: linked,
                style: Some(Style::new().fg(theme.fg_ansi_key(ThemeKey::Accent).to_string())),
            });
        }

        // Match pi's `str(value)` helper
        let content_str = match content {
            Some(content_val) => content_val.as_str(),
            None => Some(""),
        };

        match content_str {
            None => {
                segments.push(StyledSegment {
                    text: "\n".to_string(),
                    style: None,
                });
                segments.push(StyledSegment {
                    text: "[invalid content arg - expected string]".to_string(),
                    style: Some(Style::new().fg(theme.fg_ansi_key(ThemeKey::Error).to_string())),
                });
            }
            Some("") => {}
            Some(text) => {
                let mut cache_guard = self.cache.lock().unwrap();

                // Pi: when argsComplete, do full rebuild; otherwise incremental
                if ctx.args_complete {
                    *cache_guard = rebuild_highlight_cache(raw_path, text);
                } else {
                    *cache_guard =
                        update_highlight_cache_incremental(cache_guard.take(), raw_path, text);
                }

                let lang = raw_path.and_then(crate::tui::components::path_to_language);

                let rendered_lines: Vec<String> = if let Some(ref cache) = *cache_guard {
                    cache.highlighted_lines.clone()
                } else if lang.is_some() {
                    let normalized = text.replace('\r', "").replace('\t', "   ");
                    #[cfg(feature = "syntect")]
                    {
                        let hl = crate::tui::components::highlight_code(&normalized, lang);
                        if !hl.is_empty() {
                            hl
                        } else {
                            normalized.lines().map(|l| l.to_string()).collect()
                        }
                    }
                    #[cfg(not(feature = "syntect"))]
                    {
                        normalized.lines().map(|l| l.to_string()).collect()
                    }
                } else {
                    text.replace('\r', "")
                        .split('\n')
                        .map(|l| l.to_string())
                        .collect()
                };

                // Trim trailing empty lines (pi: trimTrailingEmptyLines)
                let trimmed = trim_trailing_empty_lines(&rendered_lines);
                let total_lines = trimmed.len();
                let max_lines = if ctx.expanded {
                    total_lines
                } else {
                    PREVIEW_LINES
                };
                let display_lines = if total_lines > max_lines {
                    &trimmed[..max_lines]
                } else {
                    trimmed
                };
                let remaining = total_lines.saturating_sub(max_lines);

                let has_highlighting = cache_guard.is_some();

                // Pi: blank line between header and content
                segments.push(StyledSegment {
                    text: "\n".to_string(),
                    style: None,
                });

                let output_style =
                    Style::new().fg(theme.fg_ansi_key(ThemeKey::ToolOutput).to_string());
                let muted_style = Style::new().fg(theme.fg_ansi_key(ThemeKey::Muted).to_string());
                let dim_style = Style::new().fg(theme.fg_ansi_key(ThemeKey::Dim).to_string());

                for line in display_lines {
                    let line_text = if has_highlighting {
                        format!("{}\n", line)
                    } else {
                        format!("{}\n", line.replace('\t', "   "))
                    };
                    segments.push(StyledSegment {
                        text: line_text,
                        style: if has_highlighting {
                            None
                        } else {
                            Some(output_style.clone())
                        },
                    });
                }

                if remaining > 0 {
                    let remaining_segments = vec![
                        StyledSegment {
                            text: format!("... ({} more lines, {} total, ", remaining, total_lines),
                            style: Some(muted_style.clone()),
                        },
                        StyledSegment {
                            text: ctx.expand_key.clone(),
                            style: Some(dim_style.clone()),
                        },
                        StyledSegment {
                            text: " to expand)".to_string(),
                            style: Some(muted_style.clone()),
                        },
                    ];
                    segments.extend(remaining_segments);
                }
            }
        }

        std::boxed::Box::new(crate::tui::components::Text::from_segments(
            segments, 0, 0, None,
        ))
    }

    fn render_result(
        &self,
        content: &str,
        theme: &dyn Theme,
        _ctx: &ToolRenderContext,
    ) -> Option<Box<dyn Component>> {
        // Pi: formatWriteResult prepends \n before error text
        if !_ctx.is_error || content.is_empty() {
            return None;
        }
        let error_style = Style::new().fg(theme.fg_ansi_key(ThemeKey::Error).to_string());
        Some(std::boxed::Box::new(crate::tui::components::Text::new(
            content.to_string(),
            0,
            0,
            Some(error_style),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoagent::AgentTool;
    use yoagent::types::ToolContext;

    fn tool_ctx() -> ToolContext {
        ToolContext {
            tool_call_id: "id".into(),
            tool_name: "write".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
            on_update: None,
            on_progress: None,
        }
    }

    fn tmp_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rab-write-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_tool() -> (WriteTool, std::path::PathBuf) {
        let tmp = tmp_dir();
        let tool = WriteTool {
            cwd: tmp.clone(),
            operations: Arc::new(DefaultWriteOperations),
        };
        (tool, tmp)
    }

    async fn exec_ok(tool: &WriteTool, args: serde_json::Value) -> String {
        let result = tool.execute(args, tool_ctx()).await.unwrap();
        yo_msg_text(&result.content)
    }

    fn yo_msg_text(content: &[yoagent::types::Content]) -> String {
        content
            .iter()
            .filter_map(|c| {
                if let yoagent::types::Content::Text { text } = c {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }

    // ── Tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn writes_file_content() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("test.txt");
        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "content": "hello world\n"}),
        )
        .await;

        assert!(result.contains("Successfully wrote"));
        assert!(result.contains("12 bytes"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world\n");
    }

    #[tokio::test]
    async fn creates_parent_directories() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("subdir/nested/file.txt");
        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "content": "nested\n"}),
        )
        .await;

        assert!(result.contains("Successfully wrote"));
        assert!(path.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested\n");
    }

    #[tokio::test]
    async fn missing_path_errors() {
        let (tool, _tmp) = make_tool();
        let result = tool
            .execute(serde_json::json!({"content": "hello"}), tool_ctx())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn missing_content_errors() {
        let (tool, tmp) = make_tool();
        let result = tool
            .execute(
                serde_json::json!({"path": tmp.join("test.txt").to_str().unwrap()}),
                tool_ctx(),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn handles_empty_content() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("empty.txt");
        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "content": ""}),
        )
        .await;

        assert!(result.contains("Successfully wrote"));
        assert!(result.contains("0 bytes"));
    }

    #[tokio::test]
    async fn cancel_aborts_write() {
        let (tool, tmp) = make_tool();
        let path = tmp.join("cancelled.txt");
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();

        let result = tool
            .execute(
                serde_json::json!({"path": path.to_str().unwrap(), "content": "hello"}),
                ToolContext {
                    tool_call_id: "id".into(),
                    tool_name: "write".into(),
                    cancel,
                    on_update: None,
                    on_progress: None,
                },
            )
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_highlight_single_line_empty() {
        let result = highlight_single_line("", "rust");
        assert_eq!(result, "");
    }

    #[test]
    fn test_trim_trailing_empty_lines() {
        let lines = vec![
            "a".to_string(),
            "b".to_string(),
            "".to_string(),
            "".to_string(),
        ];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert_eq!(trimmed, &["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn test_trim_no_trailing_empty_lines() {
        let lines = vec!["a".to_string(), "b".to_string()];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert_eq!(trimmed, &["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn test_trim_all_empty() {
        let lines = vec!["".to_string(), "".to_string()];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert!(trimmed.is_empty());
    }

    #[test]
    fn test_trim_empty_input() {
        let lines: Vec<String> = vec![];
        let trimmed = trim_trailing_empty_lines(&lines);
        assert!(trimmed.is_empty());
    }

    #[test]
    fn test_rebuild_cache_unknown_lang() {
        let result = rebuild_highlight_cache(Some("foo.unknown"), "hello");
        assert!(result.is_none());
    }

    #[test]
    fn test_rebuild_cache_known_lang() {
        let result = rebuild_highlight_cache(Some("foo.rs"), "fn main() {}");
        assert!(result.is_some());
        let cache = result.unwrap();
        assert_eq!(cache.lang, "rust");
        assert_eq!(cache.raw_content, "fn main() {}");
    }

    #[test]
    fn test_incremental_update_extends_content() {
        let cache = rebuild_highlight_cache(Some("foo.rs"), "fn main()");
        assert!(cache.is_some());
        let cache = cache.unwrap();
        assert_eq!(cache.normalized_lines.len(), 1);

        let updated =
            update_highlight_cache_incremental(Some(cache), Some("foo.rs"), "fn main() {}");
        assert!(updated.is_some());
        let updated = updated.unwrap();
        assert_eq!(updated.raw_content, "fn main() {}");
    }

    #[tokio::test]
    async fn relative_path_resolves_to_cwd() {
        let (tool, tmp) = make_tool();
        let result = exec_ok(
            &tool,
            serde_json::json!({"path": "relative.txt", "content": "hello\n"}),
        )
        .await;

        assert!(result.contains("Successfully wrote"));
        let abs_path = tmp.join("relative.txt");
        assert!(abs_path.exists());
    }

    #[tokio::test]
    async fn absolute_path_is_resolved_correctly() {
        let (tool, _tmp) = make_tool();
        let tmp2 = tmp_dir();
        let path = tmp2.join("abs.txt");
        let result = exec_ok(
            &tool,
            serde_json::json!({"path": path.to_str().unwrap(), "content": "absolute\n"}),
        )
        .await;

        assert!(result.contains("Successfully wrote"));
        assert!(path.exists());
    }
}
