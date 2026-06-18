use crate::extension::{AgentTool, Extension};
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
                "timeout_secs": {
                    "type": "number",
                    "description": "Timeout in seconds (optional, default 120)"
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
    ) -> anyhow::Result<String> {
        let _ = tool_call_id;
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;
        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(120);

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output();

        let output = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), output)
            .await
            .map_err(|_| anyhow::anyhow!("Command timed out after {} seconds", timeout_secs))??;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        let mut result = combined.clone();
        let lines: Vec<&str> = result.lines().collect();

        // Truncate to last 2000 lines
        const MAX_LINES: usize = 2000;
        if lines.len() > MAX_LINES {
            let start = lines.len() - MAX_LINES;
            result = format!(
                "[Truncated: showing last {} of {} lines]\n{}",
                MAX_LINES,
                lines.len(),
                lines[start..].join("\n")
            );
        }

        // Truncate to ~50KB
        const MAX_BYTES: usize = 50 * 1024;
        if result.len() > MAX_BYTES {
            result.truncate(MAX_BYTES);
            result.push_str("\n\n[Output truncated at 50KB]");
        }

        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            if result.is_empty() {
                Ok(format!("Command exited with code {}", code))
            } else {
                Ok(format!("{}\n\n[Command exited with code {}]", result, code))
            }
        } else if result.is_empty() {
            Ok("(no output)".into())
        } else {
            Ok(result)
        }
    }
}
