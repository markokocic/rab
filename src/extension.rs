/// Extension trait — all capability (built-in or user-provided) comes through this.
use crate::types::ToolCall;
use async_trait::async_trait;
use std::borrow::Cow;

/// Reason a tool call was blocked.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum BlockReason {
    Security(String),
    Policy(String),
    Other(String),
}

/// An autocomplete item for slash command arguments.
#[derive(Debug, Clone)]
pub struct AutocompleteItem {
    /// The value to insert when selected.
    pub value: String,
    /// Display label.
    pub label: String,
    /// Optional description.
    pub description: Option<String>,
}

/// A slash command handler (built-in or extension-provided).
/// Commands use the same Extension trait as tools — built-ins and
/// user extensions register commands through a uniform interface.
pub trait CommandHandler: Send + Sync {
    /// Execute the command with the given arguments string.
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult>;

    /// Get argument completions for autocomplete.
    /// Called when user types `/cmd ` — returns matching autocomplete items.
    fn argument_completions(&self, _prefix: &str) -> Vec<AutocompleteItem> {
        vec![]
    }
}

/// Result of executing a slash command.
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Command handled, show this info message.
    Info(String),
    /// Command caused a quit request.
    Quit,
    /// Command switched the model (new model name).
    ModelChanged(String),
    /// Show keyboard shortcuts help overlay.
    ShowHelp,
    /// Reload settings and auth from disk.
    Reloaded,
    /// Start a new session (clear conversation).
    NewSession,
}

/// A registered slash command.
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub handler: Box<dyn CommandHandler>,
}

/// An LLM-callable tool.
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;
    #[allow(dead_code)]
    fn label(&self) -> &str;

    /// Execute the tool. Return Ok(content) or Err(message).
    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
    ) -> anyhow::Result<String>;
}

#[async_trait]
#[allow(dead_code)]
pub trait Extension: Send + Sync {
    fn name(&self) -> Cow<'static, str>;

    /// Tools this extension provides (LLM-callable).
    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![]
    }

    /// Slash commands this extension provides (e.g. `/quit`, `/model`).
    /// Built-in commands and extension commands use the same interface.
    fn commands(&self) -> Vec<SlashCommand> {
        vec![]
    }

    /// Called before any tool executes. Return Some(reason) to block.
    async fn before_tool_call(&self, _tc: &ToolCall) -> Option<BlockReason> {
        None
    }

    /// Called after a tool executes. Return Some(text) to replace result.
    async fn after_tool_call(&self, _tc: &ToolCall, _result: &str) -> Option<String> {
        None
    }
}
