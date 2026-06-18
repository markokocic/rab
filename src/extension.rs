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

/// A slash command registered by an extension.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str,
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

    /// Additional slash commands (e.g. `/mycommand`).
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
