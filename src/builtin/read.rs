use crate::extension::{AgentTool, Extension};
use anyhow::Context;
use async_trait::async_trait;
use std::borrow::Cow;

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

    fn label(&self) -> &str {
        "Read file contents"
    }

    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
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

        let content = std::fs::read_to_string(&abs_path)
            .with_context(|| format!("Failed to read {}", abs_path.display()))?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Apply offset (1-indexed → 0-indexed)
        let start = if offset > 0 {
            (offset - 1).min(total_lines)
        } else {
            0
        };

        // Apply limit
        let end = if let Some(lim) = limit {
            (start + lim).min(total_lines)
        } else {
            total_lines
        };

        let selected: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}: {}", start + i + 1, line))
            .collect();

        let mut output = selected.join("\n");

        // Truncate to ~50KB
        const MAX_BYTES: usize = 50 * 1024;
        if output.len() > MAX_BYTES {
            output.truncate(MAX_BYTES);
            output.push_str("\n\n[Output truncated at 50KB]");
        }

        // Truncate to 2000 lines
        const MAX_LINES: usize = 2000;
        if selected.len() > MAX_LINES {
            let truncated: Vec<&String> = selected.iter().take(MAX_LINES).collect();
            output = truncated
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            output.push_str(&format!(
                "\n\n[Truncated to {} lines. Use offset={} to continue.]",
                MAX_LINES,
                start + MAX_LINES + 1
            ));
        }

        // If more lines remain, suggest offset
        if limit.is_none() && end < total_lines {
            output.push_str(&format!(
                "\n\n[{} more lines in file. Use offset={} to continue.]",
                total_lines - end,
                end + 1
            ));
        }

        Ok(output)
    }
}
