//! Type compatibility layer — wraps yoagent types behind rab's existing API.
//!
//! Gradually being replaced by direct yoagent types throughout the codebase.

use chrono::Utc;
use serde::{Deserialize, Serialize};

// ── Execution / queue modes (rab-specific, no yoagent equivalent) ──

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ToolExecutionMode {
    #[default]
    Parallel,
    Sequential,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueueMode {
    All,
    OneAtATime,
}

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

// ── Role ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    User,
    Assistant,
    ToolResult,
}

// ── ToolCall (rab struct, kept for compat) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl ToolCall {
    pub fn to_yoagent_content(&self) -> yoagent::types::Content {
        yoagent::types::Content::ToolCall {
            id: self.id.clone(),
            name: self.name.clone(),
            arguments: self.arguments.clone(),
            provider_metadata: None,
        }
    }
}

// ── Usage (rab struct, kept for compat) ──

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub cache_tokens: Option<i32>,
    pub cache_write_tokens: Option<i32>,
    pub cost_total: Option<f64>,
}

impl From<yoagent::types::Usage> for Usage {
    fn from(u: yoagent::types::Usage) -> Self {
        Self {
            input_tokens: Some(u.input as i32),
            output_tokens: Some(u.output as i32),
            cache_tokens: Some(u.cache_read as i32),
            cache_write_tokens: Some(u.cache_write as i32),
            cost_total: None,
        }
    }
}

impl From<Usage> for yoagent::types::Usage {
    fn from(u: Usage) -> Self {
        Self {
            input: u.input_tokens.unwrap_or(0) as u64,
            output: u.output_tokens.unwrap_or(0) as u64,
            cache_read: u.cache_tokens.unwrap_or(0) as u64,
            cache_write: u.cache_write_tokens.unwrap_or(0) as u64,
            total_tokens: (u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0)) as u64,
        }
    }
}

// ── AgentMessage — struct with field access, serde, yoagent conversion ──

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

    /// Convert to a vec of yoagent Message for API calls.
    pub fn to_yoagent_vec(msgs: &[Self]) -> Vec<yoagent::types::Message> {
        msgs.iter().map(|m| m.to_yoagent_msg()).collect()
    }

    fn to_yoagent_msg(&self) -> yoagent::types::Message {
        let content = {
            let mut parts = vec![yoagent::types::Content::Text {
                text: self.content.clone(),
            }];
            for tc in &self.tool_calls {
                parts.push(tc.to_yoagent_content());
            }
            parts
        };
        match self.role {
            Role::User => yoagent::types::Message::User {
                content,
                timestamp: self.timestamp as u64,
            },
            Role::Assistant => yoagent::types::Message::Assistant {
                content,
                stop_reason: yoagent::types::StopReason::Stop,
                model: String::new(),
                provider: String::new(),
                usage: self.usage.clone().map(|u| u.into()).unwrap_or_default(),
                timestamp: self.timestamp as u64,
                error_message: if self.is_error {
                    Some(self.content.clone())
                } else {
                    None
                },
            },
            Role::ToolResult => yoagent::types::Message::ToolResult {
                tool_call_id: self.tool_call_id.clone().unwrap_or_default(),
                tool_name: String::new(),
                content: vec![yoagent::types::Content::Text {
                    text: self.content.clone(),
                }],
                is_error: self.is_error,
                timestamp: self.timestamp as u64,
            },
        }
    }
}
