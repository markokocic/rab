use crate::agent::types::{AgentMessage, ToolCall, Usage};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

/// Events emitted during a streaming LLM request.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum StreamEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    Done {
        text: String,
        usage: Usage,
        stop_reason: StopReason,
        tool_calls: Vec<ToolCall>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Error,
}

/// Tool definition sent to the LLM.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// The one thing the agent loop needs from a provider.
#[async_trait]
pub trait Provider: Send + Sync {
    async fn stream(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
        tools: &[ToolDef],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>>;
}
