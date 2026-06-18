use crate::extension::{AgentTool, Extension};
use anyhow::Context;
use async_trait::async_trait;
use std::borrow::Cow;

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

    fn label(&self) -> &str {
        "Create or overwrite files"
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
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'content' argument"))?;

        let abs_path = {
            let p = std::path::Path::new(path);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                self.cwd.join(p)
            }
        };

        // Create parent directories
        if let Some(parent) = abs_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }

        // Write to temp file, then atomic rename
        let tmp_path = abs_path.with_extension(format!("tmp{}", uuid::Uuid::new_v4()));
        std::fs::write(&tmp_path, content)
            .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &abs_path).with_context(|| {
            format!(
                "Failed to rename {} → {}",
                tmp_path.display(),
                abs_path.display()
            )
        })?;

        Ok(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            path
        ))
    }
}
