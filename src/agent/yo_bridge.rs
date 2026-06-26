//! Bridge between rab's agent infrastructure and yoagent types.
//!
//! Migration path: this file shrinks as rab types are replaced by yoagent types
//! throughout the codebase. Goal: delete entirely.

use crate::agent::types::AgentMessage;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use yoagent::types::{AgentEvent, Content, Message};

// ── Message converters ──

fn yoagent_messages_to_rab(msgs: &[yoagent::types::AgentMessage]) -> Vec<AgentMessage> {
    msgs.iter()
        .filter_map(|am| match am {
            yoagent::types::AgentMessage::Llm(m) => Some(yoagent_msg_to_rab(m)),
            yoagent::types::AgentMessage::Extension(_) => None,
        })
        .collect()
}

fn yoagent_msg_to_rab(msg: &Message) -> AgentMessage {
    let (role, content, tool_calls, tool_call_id, is_error) = match msg {
        Message::User { content, .. } => {
            let text = content_text(content);
            (crate::agent::types::Role::User, text, vec![], None, false)
        }
        Message::Assistant {
            content,
            error_message,
            ..
        } => {
            let text = content_text(content);
            let tcs: Vec<_> = content
                .iter()
                .filter_map(|c| {
                    if let Content::ToolCall { id, name, arguments, .. } = c {
                        Some(crate::agent::types::ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                        })
                    } else {
                        None
                    }
                })
                .collect();
            (
                crate::agent::types::Role::Assistant,
                text,
                tcs,
                None,
                error_message.is_some(),
            )
        }
        Message::ToolResult {
            content,
            tool_call_id,
            is_error,
            ..
        } => {
            let text = content_text(content);
            (
                crate::agent::types::Role::ToolResult,
                text,
                vec![],
                Some(tool_call_id.clone()),
                *is_error,
            )
        }
    };

    AgentMessage {
        id: uuid::Uuid::new_v4().to_string(),
        parent_id: None,
        role,
        content,
        tool_calls,
        tool_call_id,
        usage: None,
        is_error,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

fn content_text(content: &[Content]) -> String {
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

// ── Event converter: yoagent AgentEvent → rab AgentEvent ──

use crate::agent::AgentEvent as RabEvent;

/// Forward loop — reads yoagent events, converts, sends to rab channel.
pub async fn forward_events(mut rx: UnboundedReceiver<AgentEvent>, tx: UnboundedSender<RabEvent>) {
    while let Some(event) = rx.recv().await {
        if let Some(rab_event) = convert_event(event)
            && tx.send(rab_event).is_err()
        {
            break;
        }
    }
}

/// Convert a yoagent AgentEvent reference → rab AgentEvent.
pub fn convert_to_rab_event(event: &AgentEvent) -> Option<RabEvent> {
    Some(match event {
        AgentEvent::AgentStart => RabEvent::AgentStart,
        AgentEvent::AgentEnd { messages } => RabEvent::AgentEnd {
            messages: yoagent_messages_to_rab(messages),
        },
        AgentEvent::TurnStart => RabEvent::TurnStart,
        AgentEvent::TurnEnd { .. } => RabEvent::TurnEnd,
        AgentEvent::MessageUpdate { delta, .. } => match delta {
            yoagent::types::StreamDelta::Text { delta } => {
                RabEvent::TextDelta { delta: delta.clone() }
            }
            yoagent::types::StreamDelta::Thinking { delta } => {
                RabEvent::ThinkingDelta { delta: delta.clone() }
            }
            yoagent::types::StreamDelta::ToolCallDelta { .. } => return None,
        },
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => RabEvent::ToolCall {
            id: tool_call_id.clone(),
            name: tool_name.clone(),
            args: args.clone(),
        },
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => {
            let content = result
                .content
                .iter()
                .filter_map(|c| {
                    if let Content::Text { text } = c {
                        Some(text.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("");
            RabEvent::ToolResult {
                id: tool_call_id.clone(),
                name: tool_name.clone(),
                content,
                compact: None,
                is_error: *is_error,
                details: None,
            }
        }
        AgentEvent::ProgressMessage { text, .. } => RabEvent::ToolProgress {
            content: text.clone(),
            is_error: false,
        },
        AgentEvent::MessageStart { .. }
        | AgentEvent::MessageEnd { .. }
        | AgentEvent::ToolExecutionUpdate { .. }
        | AgentEvent::InputRejected { .. } => return None,
    })
}

fn convert_event(event: AgentEvent) -> Option<RabEvent> {
    Some(match event {
        AgentEvent::AgentStart => RabEvent::AgentStart,
        AgentEvent::AgentEnd { messages } => RabEvent::AgentEnd {
            messages: yoagent_messages_to_rab(&messages),
        },
        AgentEvent::TurnStart => RabEvent::TurnStart,
        AgentEvent::TurnEnd { .. } => RabEvent::TurnEnd,
        AgentEvent::MessageUpdate { delta, .. } => match delta {
            yoagent::types::StreamDelta::Text { delta } => RabEvent::TextDelta { delta },
            yoagent::types::StreamDelta::Thinking { delta } => RabEvent::ThinkingDelta { delta },
            yoagent::types::StreamDelta::ToolCallDelta { .. } => return None,
        },
        AgentEvent::MessageStart { .. } | AgentEvent::MessageEnd { .. } => return None,
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => RabEvent::ToolCall {
            id: tool_call_id,
            name: tool_name,
            args,
        },
        AgentEvent::ToolExecutionUpdate { .. } => return None,
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => {
            let content = result
                .content
                .iter()
                .filter_map(|c| {
                    if let Content::Text { text } = c {
                        Some(text.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("");
            RabEvent::ToolResult {
                id: tool_call_id,
                name: tool_name,
                content,
                compact: None,
                is_error,
                details: None,
            }
        }
        AgentEvent::ProgressMessage { text, .. } => RabEvent::ToolProgress {
            content: text,
            is_error: false,
        },
        AgentEvent::InputRejected { .. } => return None,
    })
}
