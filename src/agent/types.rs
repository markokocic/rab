//! Replacement for rab's custom types — now re-exports yoagent types directly.
//!
//! This module was previously a compatibility layer that wrapped yoagent types.
//! All types are now used directly from `yoagent::types`.
//!
//! This module only provides:
//! 1. `ToolExecutionMode` — rab-specific enum with no yoagent equivalent
//! 2. Helper functions for common operations on yoagent types

pub use yoagent::types::{AgentMessage, Content, Message};

// ── Execution mode (rab-specific, no yoagent equivalent) ──

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ToolExecutionMode {
    #[default]
    Parallel,
    Sequential,
}

// ── Queue mode (rab-specific, no yoagent equivalent yet) ──

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueueMode {
    All,
    OneAtATime,
}

// ── PendingMessageQueue ────────────────────────────────────────────

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
    pub fn enqueue(&mut self, msg: AgentMessage) {
        self.messages.push(msg);
    }
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

// ── Helper functions for working with yoagent types ─────────────────

/// Extract all text content from a `Vec<Content>` as a single string.
pub fn content_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| {
            if let Content::Text { text } = c {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Extract all tool calls from a `Vec<Content>`.
pub fn content_tool_calls(content: &[Content]) -> Vec<(String, String, serde_json::Value)> {
    content
        .iter()
        .filter_map(|c| {
            if let Content::ToolCall {
                id,
                name,
                arguments,
                ..
            } = c
            {
                Some((id.clone(), name.clone(), arguments.clone()))
            } else {
                None
            }
        })
        .collect()
}

/// Get the text content of an AgentMessage (all text parts joined).
pub fn message_text(msg: &AgentMessage) -> String {
    match msg {
        AgentMessage::Llm(m) => match m {
            Message::User { content, .. }
            | Message::Assistant { content, .. }
            | Message::ToolResult { content, .. } => content_text(content),
        },
        AgentMessage::Extension(ext) => ext.data.to_string(),
    }
}

/// Get the timestamp from an AgentMessage.
pub fn message_timestamp(msg: &AgentMessage) -> u64 {
    match msg {
        AgentMessage::Llm(m) => match m {
            Message::User { timestamp, .. }
            | Message::Assistant { timestamp, .. }
            | Message::ToolResult { timestamp, .. } => *timestamp,
        },
        AgentMessage::Extension(_) => 0,
    }
}

/// Check if an AgentMessage is a tool result with an error.
pub fn message_is_error(msg: &AgentMessage) -> bool {
    matches!(
        msg,
        AgentMessage::Llm(Message::ToolResult { is_error: true, .. })
    )
}

/// Get the tool_call_id from a ToolResult message.
pub fn message_tool_call_id(msg: &AgentMessage) -> Option<&str> {
    match msg {
        AgentMessage::Llm(Message::ToolResult { tool_call_id, .. }) => Some(tool_call_id.as_str()),
        _ => None,
    }
}

/// Get the usage from an Assistant message.
pub fn message_usage(msg: &AgentMessage) -> Option<yoagent::types::Usage> {
    match msg {
        AgentMessage::Llm(Message::Assistant { usage, .. }) => Some(usage.clone()),
        _ => None,
    }
}

/// Check if an AgentMessage is a User message.
pub fn message_is_user(msg: &AgentMessage) -> bool {
    matches!(msg, AgentMessage::Llm(Message::User { .. }))
}

/// Check if an AgentMessage is an Assistant message.
pub fn message_is_assistant(msg: &AgentMessage) -> bool {
    matches!(msg, AgentMessage::Llm(Message::Assistant { .. }))
}

/// Check if an AgentMessage is a ToolResult message.
pub fn message_is_tool_result(msg: &AgentMessage) -> bool {
    matches!(msg, AgentMessage::Llm(Message::ToolResult { .. }))
}

/// Get the model from an Assistant message.
pub fn message_model(msg: &AgentMessage) -> Option<&str> {
    match msg {
        AgentMessage::Llm(Message::Assistant { model, .. }) => Some(model.as_str()),
        _ => None,
    }
}

/// Get the provider from an Assistant message.
pub fn message_provider(msg: &AgentMessage) -> Option<&str> {
    match msg {
        AgentMessage::Llm(Message::Assistant { provider, .. }) => Some(provider.as_str()),
        _ => None,
    }
}

/// Get the error_message from an Assistant message.
pub fn message_error_message(msg: &AgentMessage) -> Option<&str> {
    match msg {
        AgentMessage::Llm(Message::Assistant {
            error_message: Some(err),
            ..
        }) => Some(err.as_str()),
        _ => None,
    }
}

/// Create a simple User AgentMessage with text content.
pub fn user_message(text: impl Into<String>) -> AgentMessage {
    AgentMessage::Llm(Message::User {
        content: vec![Content::Text { text: text.into() }],
        timestamp: yoagent::types::now_ms(),
    })
}

/// Create a simple Assistant AgentMessage with text content.
pub fn assistant_message(text: impl Into<String>) -> AgentMessage {
    AgentMessage::Llm(Message::Assistant {
        content: vec![Content::Text { text: text.into() }],
        stop_reason: yoagent::types::StopReason::Stop,
        model: String::new(),
        provider: String::new(),
        usage: yoagent::types::Usage::default(),
        timestamp: yoagent::types::now_ms(),
        error_message: None,
    })
}

/// Create a ToolResult AgentMessage.
pub fn tool_result_message(
    tool_call_id: impl Into<String>,
    text: impl Into<String>,
    is_error: bool,
) -> AgentMessage {
    AgentMessage::Llm(Message::ToolResult {
        tool_call_id: tool_call_id.into(),
        tool_name: String::new(),
        content: vec![Content::Text { text: text.into() }],
        is_error,
        timestamp: yoagent::types::now_ms(),
    })
}

/// Count how many tool calls are in an AgentMessage.
pub fn message_tool_call_count(msg: &AgentMessage) -> usize {
    match msg {
        AgentMessage::Llm(Message::Assistant { content, .. }) => content
            .iter()
            .filter(|c| matches!(c, Content::ToolCall { .. }))
            .count(),
        AgentMessage::Llm(_) => 0,
        _ => 0,
    }
}
