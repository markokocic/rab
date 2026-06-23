use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use crate::agent::extension::{ToolRenderContext, ToolRenderer};
use crate::tui::Theme;
use crate::tui::ThemeKey;
use anyhow::Context;
use async_trait::async_trait;
use std::borrow::Cow;
use tokio::sync::mpsc::UnboundedSender;

pub struct WriteExtension {
    cwd: std::path::PathBuf,
}

impl WriteExtension {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for WriteExtension {
    fn name(&self) -> Cow<'static, str> {
        "write".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(WriteTool {
            cwd: self.cwd.clone(),
        })]
    }
}

struct WriteTool {
    cwd: std::path::PathBuf,
}

#[async_trait]
impl AgentTool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. \
         Automatically creates parent directories."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write (relative or absolute)"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            }
        })
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        vec!["Use write only for new files or complete rewrites.".into()]
    }

    fn label(&self) -> &str {
        "Create or overwrite files"
    }

    fn renderer(&self) -> Option<Box<dyn ToolRenderer>> {
        Some(Box::new(WriteRenderer::new()))
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
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'content' argument"))?;

        cancel.check()?;

        let cwd = self.cwd.clone();
        let path_for_queue = path.to_owned();
        let cwd_for_closure = cwd.clone();
        let path_for_closure = path.to_owned();
        let content_owned = content.to_owned();

        let result = crate::builtin::file_mutation_queue::with_file_mutation_queue(
            &path_for_queue,
            &cwd,
            || async move {
                let abs_path = {
                    let p = std::path::Path::new(&path_for_closure);
                    if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        cwd_for_closure.join(p)
                    }
                };

                // Create parent directories
                if let Some(parent) = abs_path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("Failed to create directory {}", parent.display())
                    })?;
                }

                // Write to temp file, then atomic rename
                let tmp_path = abs_path.with_extension(format!("tmp{}", uuid::Uuid::new_v4()));
                std::fs::write(&tmp_path, &content_owned)
                    .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
                std::fs::rename(&tmp_path, &abs_path).with_context(|| {
                    format!(
                        "Failed to rename {} → {}",
                        tmp_path.display(),
                        abs_path.display()
                    )
                })?;

                Ok::<_, anyhow::Error>(format!(
                    "Successfully wrote {} bytes to {}",
                    content_owned.len(),
                    path_for_closure
                ))
            },
        )
        .await?;

        Ok(ToolOutput::ok(result))
    }
}

/// Tool renderer for the `write` tool.
/// Shows the file path and a content preview in the call, empty result on success.
/// Includes incremental caching for syntax-highlighted content.
struct WriteRenderer {
    /// Cache state using RwLock for thread safety.
    cache: std::sync::RwLock<WriteCache>,
}

struct WriteCache {
    /// Cache key: (content_hash, expanded, preview_lines_count)
    key: Option<(u64, bool, usize)>,
    /// Cached highlighted lines (without the leading \n prefix)
    lines: Vec<String>,
    /// Cached remaining count
    remaining: usize,
}

impl WriteRenderer {
    fn new() -> Self {
        Self {
            cache: std::sync::RwLock::new(WriteCache {
                key: None,
                lines: Vec::new(),
                remaining: 0,
            }),
        }
    }

    /// Compute a hash of the content for cache invalidation.
    fn content_hash(content: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }

    /// Get or compute highlighted lines, using cache when possible.
    fn get_highlighted_lines(
        &self,
        content: &str,
        path: &str,
        expanded: bool,
    ) -> (Vec<String>, usize) {
        let hash = Self::content_hash(content);
        let max_preview = if expanded { usize::MAX } else { 5 };
        let content_lines: Vec<&str> = content.lines().collect();
        let preview_count = content_lines.len().min(max_preview);
        let remaining = content_lines.len().saturating_sub(preview_count);

        let key = (hash, expanded, preview_count);

        // Check cache (read lock)
        {
            let cache = self.cache.read().unwrap();
            if let Some(ref cached_key) = cache.key
                && *cached_key == key
                && !cache.lines.is_empty()
            {
                return (cache.lines.clone(), cache.remaining);
            }
        }

        // Compute highlighted lines
        let display: Vec<&str> = content_lines.iter().copied().take(preview_count).collect();
        let lang = if !path.is_empty() {
            crate::tui::components::path_to_language(path)
        } else {
            None
        };

        let mut highlighted = Vec::new();

        #[cfg(feature = "syntect")]
        if let Some(lang) = lang {
            let text = display.join("\n");
            let hl = crate::tui::components::highlight_code(&text, Some(lang));
            if !hl.is_empty() {
                highlighted = hl;
            }
        }

        // Fallback: no highlighting
        if highlighted.is_empty() {
            highlighted = display.iter().map(|l| l.to_string()).collect();
        }

        // Update cache (write lock)
        {
            let mut cache = self.cache.write().unwrap();
            cache.key = Some(key);
            cache.lines = highlighted.clone();
            cache.remaining = remaining;
        }

        (highlighted, remaining)
    }
}

impl ToolRenderer for WriteRenderer {
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
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

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
            theme.fg_key(ThemeKey::ToolTitle, &theme.bold("write")),
            path_disp
        );

        let mut lines = vec![header];

        // Show content preview (first few lines) when not expanded
        if !content.is_empty() {
            let (display, remaining) = self.get_highlighted_lines(content, path, ctx.expanded);

            for line in &display {
                lines.push(format!("\n{}", theme.fg_key(ThemeKey::ToolOutput, line)));
            }

            if remaining > 0 {
                lines.push(theme.fg(
                    "muted",
                    &format!(
                        "... ({} more lines, {} total, {} to expand)",
                        remaining,
                        content.lines().count(),
                        ctx.expand_key
                    ),
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
        ctx: &ToolRenderContext,
    ) -> Vec<String> {
        // On success, pi shows no result output (just the background color transition).
        // On error, show the error text.
        if !ctx.is_error || content.is_empty() {
            return vec![];
        }
        vec![theme.fg_key(ThemeKey::Error, content)]
    }
}
