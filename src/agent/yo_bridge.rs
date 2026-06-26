//! Bridge between rab's agent infrastructure and yoagent types.
//!
//! Migration path: this file shrinks as rab types are replaced by yoagent types
//! throughout the codebase. Goal: delete entirely.

use crate::agent::extension::AgentTool as RabAgentTool;
use crate::agent::provider::ToolDef;
use crate::agent::types::AgentMessage;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use yoagent::provider::model::{ModelConfig, OpenAiCompat};
use yoagent::types::{AgentEvent, Content, Message};

// ──────────────────────────────────────────────
// 1. Tool adapter: rab AgentTool → yoagent AgentTool

pub async fn summarize_text(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    messages: &[crate::agent::types::AgentMessage],
) -> Result<String, String> {
    use yoagent::provider::StreamProvider;
    use yoagent::provider::traits::StreamConfig;

    let yoagent_messages: Vec<yoagent::types::Message> = messages
        .iter()
        .map(|m| {
            let content = vec![yoagent::types::Content::Text {
                text: m.content.clone(),
            }];
            match m.role {
                crate::agent::types::Role::User => yoagent::types::Message::User {
                    content,
                    timestamp: 0,
                },
                crate::agent::types::Role::Assistant => yoagent::types::Message::Assistant {
                    content,
                    stop_reason: yoagent::types::StopReason::Stop,
                    model: model.to_string(),
                    provider: String::new(),
                    usage: yoagent::types::Usage::default(),
                    timestamp: 0,
                    error_message: None,
                },
                crate::agent::types::Role::ToolResult => yoagent::types::Message::ToolResult {
                    tool_call_id: m.tool_call_id.clone().unwrap_or_default(),
                    tool_name: String::new(),
                    content,
                    is_error: m.is_error,
                    timestamp: 0,
                },
            }
        })
        .collect();

    let config = StreamConfig {
        model: model.to_string(),
        system_prompt: system_prompt.to_string(),
        messages: yoagent_messages,
        tools: vec![],
        thinking_level: yoagent::types::ThinkingLevel::Off,
        api_key: api_key.to_string(),
        max_tokens: Some(2048),
        temperature: Some(0.3),
        model_config: Some(opencode_model_config()),
        cache_config: yoagent::types::CacheConfig::default(),
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = tokio_util::sync::CancellationToken::new();

    tokio::spawn(async move {
        let _ = yoagent::provider::OpenAiCompatProvider
            .stream(config, tx, cancel)
            .await;
    });

    let mut text = String::new();
    let mut last_error: Option<String> = None;

    while let Some(event) = rx.recv().await {
        match event {
            yoagent::provider::traits::StreamEvent::TextDelta { delta, .. } => {
                text.push_str(&delta);
            }
            yoagent::provider::traits::StreamEvent::Done { message } => {
                // Extract final text from the message
                if let yoagent::types::Message::Assistant { content, .. } = &message {
                    for c in content {
                        if let yoagent::types::Content::Text { text: t } = c
                            && text.is_empty()
                        {
                            text = t.clone();
                        }
                    }
                }
                break;
            }
            yoagent::provider::traits::StreamEvent::Error { .. } => {
                last_error = Some("Provider returned error".to_string());
                break;
            }
            _ => {}
        }
    }

    if let Some(err) = last_error {
        return Err(err);
    }
    Ok(text)
}

/// Collect tool definitions from a list of rab AgentTools.
/// Used by main.rs to build LLM tool schemas.
pub fn collect_tool_defs(agent_tools: &[Box<dyn RabAgentTool>]) -> Vec<ToolDef> {
    let mut defs = Vec::new();
    for tool in agent_tools {
        if !defs.iter().any(|d: &ToolDef| d.name == tool.name()) {
            defs.push(ToolDef {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters(),
            });
        }
    }
    defs
}

// ──────────────────────────────────────────────
// 2. Provider bridge (kept for compaction & AgentSession compatibility)
// ──────────────────────────────────────────────

/// Hardcoded to highest available.
pub fn opencode_model_config() -> ModelConfig {
    ModelConfig::openai_compat(
        "https://opencode.ai/zen/go/v1",
        "deepseek-v4-flash",
        "opencode-go",
        OpenAiCompat::deepseek(),
    )
}

// ──────────────────────────────────────────────
// 3. Message converters
// ──────────────────────────────────────────────

/// Convert rab `AgentMessage` slice → yoagent `Message` vec.
pub fn rab_messages_to_yoagent(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .map(|m| {
            use crate::agent::types::Role;
            let content = vec![Content::Text {
                text: m.content.clone(),
            }];
            match m.role {
                Role::User => Message::User {
                    content,
                    timestamp: 0,
                },
                Role::Assistant => Message::Assistant {
                    content,
                    stop_reason: yoagent::types::StopReason::Stop,
                    model: String::new(),
                    provider: String::new(),
                    usage: yoagent::types::Usage::default(),
                    timestamp: 0,
                    error_message: None,
                },
                Role::ToolResult => Message::ToolResult {
                    tool_call_id: m.tool_call_id.clone().unwrap_or_default(),
                    tool_name: String::new(),
                    content,
                    is_error: m.is_error,
                    timestamp: 0,
                },
            }
        })
        .collect()
}

/// Convert yoagent `AgentMessage` → rab `AgentMessage`.
pub fn yoagent_messages_to_rab(msgs: &[yoagent::types::AgentMessage]) -> Vec<AgentMessage> {
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
                    if let Content::ToolCall {
                        id,
                        name,
                        arguments,
                        ..
                    } = c
                    {
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

// ──────────────────────────────────────────────
// 3. Event converter: yoagent AgentEvent → rab AgentEvent
// ──────────────────────────────────────────────

use crate::agent::AgentEvent as RabEvent;

/// Forward loop - reads yoagent events from `rx`, converts to rab events, sends to `tx`.
/// Meant to be spawned as a task in front of the existing rab event consumer.
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
            yoagent::types::StreamDelta::Text { delta } => RabEvent::TextDelta {
                delta: delta.clone(),
            },
            yoagent::types::StreamDelta::Thinking { delta } => RabEvent::ThinkingDelta {
                delta: delta.clone(),
            },
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
        AgentEvent::MessageStart { .. } => return None,
        AgentEvent::MessageUpdate { delta, .. } => match delta {
            yoagent::types::StreamDelta::Text { delta } => RabEvent::TextDelta { delta },
            yoagent::types::StreamDelta::Thinking { delta } => RabEvent::ThinkingDelta { delta },
            yoagent::types::StreamDelta::ToolCallDelta { .. } => return None,
        },
        AgentEvent::MessageEnd { .. } => return None,
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
