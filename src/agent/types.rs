//! Helper functions for common operations on yoagent types.
//!
//! All message types are used directly from `yoagent::types`.

pub use yoagent::types::{AgentMessage, Content, Message};

// ── Helper functions for working with yoagent types ─────────────────

/// Extract all text content from a `Vec<Content>` as a single string.
pub(crate) fn content_text(content: &[Content]) -> String {
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

/// Compute a dedup key for a message, distinguishing messages that
/// `message_text()` alone would conflate (e.g. two assistant messages
/// with empty text but different tool calls).
pub fn message_dedup_key(msg: &AgentMessage) -> String {
    match msg {
        AgentMessage::Llm(m) => match m {
            Message::User { content, .. } => {
                format!("user:{}", content_text(content))
            }
            Message::Assistant {
                content,
                stop_reason,
                ..
            } => {
                // Include tool call IDs and stop_reason so two assistant
                // messages with empty text but different tool calls get
                // distinct keys.
                let tc_ids: Vec<&str> = content
                    .iter()
                    .filter_map(|c| {
                        if let Content::ToolCall { id, .. } = c {
                            Some(id.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                format!(
                    "assistant:{}:{:?}:{:?}",
                    content_text(content),
                    tc_ids,
                    stop_reason
                )
            }
            Message::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                format!(
                    "tool:{}:{}:{}",
                    tool_call_id,
                    content_text(content),
                    is_error
                )
            }
        },
        AgentMessage::Extension(ext) => {
            format!("ext:{}:{}", ext.kind, ext.data)
        }
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

/// Check if an AgentMessage is a system-generated stop notification
/// (e.g. execution limit reached, max tokens exceeded).
/// The agent loop injects these as user messages starting with `[Agent stopped:`.
pub fn message_is_system_stop(msg: &AgentMessage) -> bool {
    if !message_is_user(msg) {
        return false;
    }
    let text = message_text(msg);
    text.trim_start().starts_with("[Agent stopped:")
}

/// Extract the error_message from an Assistant message, if present.
pub fn message_error(msg: &AgentMessage) -> Option<&str> {
    match msg {
        AgentMessage::Llm(Message::Assistant {
            error_message: Some(e),
            ..
        }) => Some(e.as_str()),
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
    tool_name: impl Into<String>,
    text: impl Into<String>,
    is_error: bool,
) -> AgentMessage {
    AgentMessage::Llm(Message::ToolResult {
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
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

// ── Extension message helpers (pi-compatible custom_message) ────────

/// Check if an AgentMessage is an Extension message.
pub fn message_is_extension(msg: &AgentMessage) -> bool {
    matches!(msg, AgentMessage::Extension(_))
}

/// Get the kind/customType from an Extension message.
pub fn message_extension_kind(msg: &AgentMessage) -> Option<&str> {
    match msg {
        AgentMessage::Extension(ext) => Some(ext.kind.as_str()),
        _ => None,
    }
}

/// Get the text content from an Extension message's data field.
pub fn message_extension_text(msg: &AgentMessage) -> Option<String> {
    match msg {
        AgentMessage::Extension(ext) => ext
            .data
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

/// Create an Extension message (pi-compatible custom_message).
/// `kind` identifies the type ("info", "error", "system_stop", etc.).
pub fn extension_message(
    kind: impl Into<String>,
    text: impl Into<String>,
    display: bool,
) -> AgentMessage {
    AgentMessage::Extension(yoagent::types::ExtensionMessage::new(
        kind,
        serde_json::json!({
            "text": text.into(),
            "display": display,
        }),
    ))
}

/// Create an Extension message with structured details.
pub fn extension_message_with_details(
    kind: impl Into<String>,
    text: impl Into<String>,
    display: bool,
    details: serde_json::Value,
) -> AgentMessage {
    AgentMessage::Extension(yoagent::types::ExtensionMessage::new(
        kind,
        serde_json::json!({
            "text": text.into(),
            "display": display,
            "details": details,
        }),
    ))
}

/// Create a base ModelConfig for the opencode-go provider.
/// Sets the standard context_window (1M) and max_tokens (393216)
/// for DeepSeek v4 models. Callers override if needed.
pub fn base_model_config(model: &str) -> yoagent::provider::model::ModelConfig {
    let mut mc = yoagent::provider::model::ModelConfig::openai_compat(
        "https://opencode.ai/zen/go/v1",
        model,
        "opencode-go",
        yoagent::provider::model::OpenAiCompat::deepseek(),
    );
    mc.context_window = 1_000_000;
    mc.max_tokens = 393_216;
    mc
}
