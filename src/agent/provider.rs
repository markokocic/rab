// ── AgentEvent (used by TUI and session) ──

use crate::agent::types::AgentMessage;

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
