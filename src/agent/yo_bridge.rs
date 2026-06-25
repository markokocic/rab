//! Bridge between rab's agent infrastructure and yoagent types.
//!
//! Migration path: this file shrinks as rab types are replaced by yoagent types
//! throughout the codebase. Goal: delete entirely.

use crate::agent::extension::{AgentTool as RabAgentTool, Cancel, ToolOutput};
use crate::agent::provider::{Provider, StopReason, StreamEvent, ToolDef};
use crate::agent::types::AgentMessage;
use crate::auth::AuthStorage;
use async_trait::async_trait;
use futures::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use yoagent::provider::StreamProvider;
use yoagent::provider::model::{ModelConfig, OpenAiCompat};
use yoagent::provider::traits::StreamConfig;
use yoagent::types::ThinkingLevel;
use yoagent::types::{AgentEvent, Content, Message, ToolContext, ToolError, ToolResult};

// ──────────────────────────────────────────────
// 1. Tool adapter: rab AgentTool → yoagent AgentTool
// ──────────────────────────────────────────────

/// Wraps a rab `AgentTool` so it can be used by yoagent's loop.
pub struct RabToolAdapter {
    inner: Box<dyn RabAgentTool>,
}

impl RabToolAdapter {
    pub fn new(inner: Box<dyn RabAgentTool>) -> Self {
        Self { inner }
    }

    /// Wrap a slice of rab tools.
    pub fn wrap_all(tools: &[Box<dyn RabAgentTool>]) -> Vec<Box<dyn yoagent::types::AgentTool>> {
        tools
            .iter()
            .map(|t| {
                let inner = t.clone_boxed();
                Box::new(RabToolAdapter { inner }) as Box<dyn yoagent::types::AgentTool>
            })
            .collect()
    }
}

#[async_trait]
impl yoagent::types::AgentTool for RabToolAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn label(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters()
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> std::result::Result<ToolResult, ToolError> {
        let tool_call_id = ctx.tool_call_id.clone();
        let cancel = Cancel::new();
        let (update_tx, mut update_rx) = tokio::sync::mpsc::unbounded_channel::<ToolOutput>();

        // Forward updates from the tool to yoagent's progress callback
        if let Some(on_progress) = &ctx.on_progress {
            let on_progress = on_progress.clone();
            tokio::spawn(async move {
                while let Some(output) = update_rx.recv().await {
                    let _ = on_progress(output.content);
                }
            });
        }

        match self
            .inner
            .execute(tool_call_id, params, cancel, Some(update_tx))
            .await
        {
            Ok(output) => {
                let content = vec![Content::Text {
                    text: output.content,
                }];
                Ok(ToolResult {
                    content,
                    details: serde_json::Value::Null,
                })
            }
            Err(e) => Err(ToolError::Failed(e.to_string())),
        }
    }
}

// ──────────────────────────────────────────────
// 2. Provider bridge (kept for compaction & AgentSession compatibility)
// ──────────────────────────────────────────────

/// Hardcoded to highest available.
const THINKING_LEVEL: ThinkingLevel = ThinkingLevel::High;

pub fn opencode_model_config() -> ModelConfig {
    ModelConfig::openai_compat(
        "https://opencode.ai/zen/go/v1",
        "deepseek-v4-flash",
        "opencode-go",
        OpenAiCompat::deepseek(),
    )
}

/// YoAgent-backed provider implementing rab's `Provider` trait.
/// Kept for compaction in AgentSession.
pub struct YoAgentProvider {
    api_key: String,
}

impl YoAgentProvider {
    pub fn new(auth: &AuthStorage) -> anyhow::Result<Self> {
        let api_key = auth
            .api_key("opencode-go")
            .ok_or_else(|| anyhow::anyhow!("No opencode-go API key"))?;
        Ok(Self { api_key })
    }
}

#[async_trait]
impl Provider for YoAgentProvider {
    async fn stream(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
        tools: &[ToolDef],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>> {
        let config = StreamConfig {
            model: model.to_string(),
            system_prompt: system_prompt.to_string(),
            messages: rab_messages_to_yoagent(messages),
            tools: tools
                .iter()
                .map(|t| yoagent::provider::traits::ToolDefinition {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                })
                .collect(),
            thinking_level: THINKING_LEVEL,
            api_key: self.api_key.clone(),
            max_tokens: None,
            temperature: None,
            model_config: Some(opencode_model_config()),
            cache_config: yoagent::types::CacheConfig::default(),
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut partial_tool_calls: HashMap<usize, ToolCallAccum> = HashMap::new();

        tokio::spawn(async move {
            let _ = yoagent::provider::OpenAiCompatProvider
                .stream(config, tx, cancel)
                .await;
        });

        let stream = async_stream::stream! {
            while let Some(event) = rx.recv().await {
                match event {
                    yoagent::provider::traits::StreamEvent::Start => {}
                    yoagent::provider::traits::StreamEvent::TextDelta { delta, .. } => {
                        yield StreamEvent::TextDelta { text: delta };
                    }
                    yoagent::provider::traits::StreamEvent::ThinkingDelta { delta, .. } => {
                        yield StreamEvent::ThinkingDelta { text: delta };
                    }
                    yoagent::provider::traits::StreamEvent::ToolCallStart { content_index, id, name } => {
                        partial_tool_calls.insert(content_index, ToolCallAccum { id, name, arguments: String::new() });
                    }
                    yoagent::provider::traits::StreamEvent::ToolCallDelta { content_index, delta } => {
                        if let Some(acc) = partial_tool_calls.get_mut(&content_index) {
                            acc.arguments.push_str(&delta);
                        }
                    }
                    yoagent::provider::traits::StreamEvent::ToolCallEnd { content_index } => {
                        if let Some(acc) = partial_tool_calls.remove(&content_index) {
                            let args: serde_json::Value =
                                serde_json::from_str(&acc.arguments)
                                    .unwrap_or(serde_json::Value::String(acc.arguments.clone()));
                            yield StreamEvent::ToolCall {
                                id: acc.id,
                                name: acc.name,
                                arguments: serde_json::to_string(&args).unwrap_or_default(),
                            };
                        }
                    }
                    yoagent::provider::traits::StreamEvent::Done { message } => {
                        let (text, tool_calls, usage, stop_reason) = extract_from_provider_message(&message);
                        yield StreamEvent::Done { text, usage, stop_reason, tool_calls };
                    }
                    yoagent::provider::traits::StreamEvent::Error { message: err_msg } => {
                        let error_text = match &err_msg {
                            yoagent::types::Message::Assistant { error_message: Some(e), .. } => e.clone(),
                            _ => String::new(),
                        };
                        yield StreamEvent::Error { message: error_text };
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    fn set_reasoning_effort(&self, _level: Option<&str>) {}
}

struct ToolCallAccum {
    id: String,
    name: String,
    arguments: String,
}

fn extract_from_provider_message(
    msg: &yoagent::types::Message,
) -> (
    String,
    Vec<crate::agent::types::ToolCall>,
    crate::agent::types::Usage,
    StopReason,
) {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut usage = crate::agent::types::Usage::default();
    let mut stop_reason = StopReason::EndTurn;

    if let yoagent::types::Message::Assistant {
        content,
        stop_reason: sr,
        usage: u,
        ..
    } = msg
    {
        for c in content {
            match c {
                Content::Text { text: t } => text.push_str(t),
                Content::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    tool_calls.push(crate::agent::types::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                }
                _ => {}
            }
        }

        stop_reason = match sr {
            yoagent::types::StopReason::Stop => StopReason::EndTurn,
            yoagent::types::StopReason::ToolUse => StopReason::ToolUse,
            yoagent::types::StopReason::Length => StopReason::MaxTokens,
            yoagent::types::StopReason::Error => StopReason::Error,
            yoagent::types::StopReason::Aborted => StopReason::EndTurn,
        };

        usage = crate::agent::types::Usage {
            input_tokens: Some(u.input as i32),
            output_tokens: Some(u.output as i32),
            cache_tokens: Some(u.cache_read as i32),
            cache_write_tokens: Some(u.cache_write as i32),
            cost_total: None,
        };
    }

    (text, tool_calls, usage, stop_reason)
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
