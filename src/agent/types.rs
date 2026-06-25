use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Whether the agent executes tool calls in parallel (default) or sequentially.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ToolExecutionMode {
    /// Execute all tool calls concurrently after sequential preflight.
    #[default]
    Parallel,
    /// Execute tool calls one at a time in order.
    Sequential,
}

/// How queued messages are drained from a pending message queue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueueMode {
    /// Drain all queued messages at once.
    All,
    /// Drain one message at a time.
    OneAtATime,
}

/// A pending message queue with a configurable drain mode.
/// Used for steering (mid-stream) and follow-up (post-agent) message delivery.
#[derive(Debug)]
pub struct PendingMessageQueue {
    messages: Vec<AgentMessage>,
    mode: QueueMode,
}

impl PendingMessageQueue {
    pub fn new(mode: QueueMode) -> Self {
        Self {
            messages: Vec::new(),
            mode,
        }
    }

    /// Add a message to the back of the queue.
    pub fn enqueue(&mut self, msg: AgentMessage) {
        self.messages.push(msg);
    }

    /// Drain messages according to the current mode.
    pub fn drain(&mut self) -> Vec<AgentMessage> {
        match self.mode {
            QueueMode::All => self.messages.drain(..).collect(),
            QueueMode::OneAtATime => {
                if self.messages.is_empty() {
                    vec![]
                } else {
                    vec![self.messages.remove(0)]
                }
            }
        }
    }

    /// Drain all messages regardless of mode.
    /// Used for dequeue operations that need to restore all messages.
    pub fn drain_all(&mut self) -> Vec<AgentMessage> {
        self.messages.drain(..).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

/// Role of a message in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    User,
    Assistant,
    ToolResult,
}

/// A tool call requested by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Token usage information for an assistant response.
///
/// Fields match pi's `AssistantMessage.usage`:
///   input, output, cacheRead, cacheWrite, cost.{total}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub cache_tokens: Option<i32>,
    /// Cache write tokens (pi's `cacheWrite`).
    pub cache_write_tokens: Option<i32>,
    /// Total cost in USD (pi's `cost.total`).
    pub cost_total: Option<f64>,
}

/// A universal message type in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    pub id: String,
    pub parent_id: Option<String>,
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    pub is_error: bool,
    pub timestamp: i64,
}

impl AgentMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role: Role::User,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            usage: None,
            is_error: false,
            timestamp: Utc::now().timestamp_millis(),
        }
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role: Role::ToolResult,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: Some(tool_call_id.into()),
            usage: None,
            is_error,
            timestamp: Utc::now().timestamp_millis(),
        }
    }
}
