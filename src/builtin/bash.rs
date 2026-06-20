use crate::agent::extension::{AgentTool, Cancel, Extension, ToolOutput};
use async_trait::async_trait;
use std::borrow::Cow;

pub struct BashExtension {
    cwd: std::path::PathBuf,
}

impl BashExtension {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for BashExtension {
    fn name(&self) -> Cow<'static, str> {
        "bash".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(BashTool {
            cwd: self.cwd.clone(),
        })]
    }
}

struct BashTool {
    cwd: std::path::PathBuf,
}

#[async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command in the current working directory. Returns stdout and stderr. \
         Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, \
         full output is saved to a temp file. Optionally provide a timeout in seconds."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash command to execute"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (optional, no default timeout)"
                }
            }
        })
    }

    fn label(&self) -> &str {
        "Execute bash commands"
    }

    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
        _cancel: Cancel,
    ) -> anyhow::Result<ToolOutput> {
        let _ = tool_call_id;
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;
        let timeout = args["timeout"].as_u64();

        let output_future = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output();

        let output = if let Some(secs) = timeout {
            tokio::time::timeout(std::time::Duration::from_secs(secs), output_future)
                .await
                .map_err(|_| anyhow::anyhow!("Command timed out after {} seconds", secs))??
        } else {
            output_future.await?
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        let total_lines = combined.lines().count();
        const MAX_LINES: usize = 2000;
        const MAX_BYTES: usize = 50 * 1024;

        // Truncate from the end (pi-style): keep last N lines, last N bytes
        let mut result = combined.clone();
        if result.len() > MAX_BYTES {
            // Find the nearest newline before MAX_BYTES from the end
            let byte_start = result.len().saturating_sub(MAX_BYTES);
            if let Some(newline_pos) = result[byte_start..].find('\n') {
                result = result[(byte_start + newline_pos + 1)..].to_string();
            } else {
                result = result[byte_start..].to_string();
            }
        }

        let lines: Vec<&str> = result.lines().collect();
        let shown_lines = lines.len();

        if lines.len() > MAX_LINES {
            let start = lines.len() - MAX_LINES;
            result = lines[start..].join("\n");
        }

        // Check if we truncated and add continuation notice
        if total_lines > shown_lines || combined.len() > MAX_BYTES {
            let start_line = total_lines - shown_lines + 1;
            result.push_str(&format!(
                "\n\n[Showing lines {}-{} of {}.]",
                start_line, total_lines, total_lines,
            ));
        }

        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            if result.is_empty() {
                Ok(ToolOutput::ok(format!("Command exited with code {}", code)))
            } else {
                Ok(ToolOutput::ok(format!(
                    "{}\n\n[Command exited with code {}]",
                    result, code
                )))
            }
        } else if result.is_empty() {
            Ok(ToolOutput::ok("(no output)"))
        } else {
            Ok(ToolOutput::ok(result))
        }
    }
}
