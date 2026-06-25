use crate::agent::types::{AgentMessage, ToolCall, Usage};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

/// Events emitted during a streaming LLM request.
#[derive(Debug, Clone)]
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

    /// Update reasoning effort (thinking level) at runtime.
    /// Default implementation is a no-op for providers that don't support this.
    fn set_reasoning_effort(&self, _level: Option<&str>) {}
}

// ── AgentEvent (moved from loop.rs, used by TUI) ──

/// Events emitted by the agent loop for UI consumers.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart,
    TurnStart,
    TextDelta {
        delta: String,
    },
    ThinkingDelta {
        delta: String,
    },
    ToolCall {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// Progressive args update (pi calls renderCall multiple times).
    ToolCallArgsUpdate {
        id: String,
        args: serde_json::Value,
    },
    ToolResult {
        id: String,
        name: String,
        content: String,
        compact: Option<String>,
        is_error: bool,
        /// Structured details for the UI renderer (not sent to LLM).
        details: Option<serde_json::Value>,
    },
    /// Intermediate tool execution progress (bash streaming output).
    ToolProgress {
        content: String,
        is_error: bool,
    },
    /// Stream was aborted or errored.
    Aborted {
        reason: String,
    },
    /// A user message was injected from the steering or follow-up queue.
    UserMessage {
        content: String,
    },
    TurnEnd,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
}

/// Transform function: rewrites messages before each LLM call.
pub type TransformFn = Box<dyn Fn(&[AgentMessage]) -> Vec<AgentMessage> + Send + Sync>;

/// Prepare-next-turn function: optionally modifies context between turns.
pub type PrepareNextTurnFn = Box<dyn Fn(&[AgentMessage]) -> Option<TurnUpdate> + Send + Sync>;

/// Should-stop-after-turn predicate: early-stop check.
pub type ShouldStopFn = Box<dyn Fn(&[AgentMessage]) -> bool + Send + Sync>;

/// Optional return value from `prepare_next_turn` to modify context for the next turn.
pub struct TurnUpdate {
    /// Replace the full message context for the next LLM call.
    pub context: Option<Vec<AgentMessage>>,
}
