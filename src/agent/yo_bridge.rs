//! Bridge between rab's agent infrastructure and yoagent types.
//!
//! Migration path: each section here shrinks as we move code to use yoagent
//! types directly. The goal is to delete this file entirely.

use crate::agent::provider::{Provider, StopReason, StreamEvent, ToolDef};
use crate::agent::types::{AgentMessage, Role, ToolCall, Usage};
use crate::auth::AuthStorage;
use async_trait::async_trait;
use futures::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use tokio::sync::mpsc;
use yoagent::provider::StreamProvider;
use yoagent::provider::model::{ModelConfig, OpenAiCompat};
use yoagent::provider::traits::{StreamConfig, ToolDefinition};
use yoagent::types::ThinkingLevel;

// ── Hardcoded to highest available ──
const THINKING_LEVEL: ThinkingLevel = ThinkingLevel::High;

/// Build a `ModelConfig` for the opencode-go gateway.
fn opencode_model_config() -> ModelConfig {
    ModelConfig::openai_compat(
        "https://opencode.ai/zen/go/v1",
        "deepseek-v4-flash",
        "opencode-go",
        OpenAiCompat::deepseek(),
    )
}

/// Build a `StreamConfig` from rab parameters.
fn build_stream_config(
    model: &str,
    system_prompt: &str,
    messages: &[AgentMessage],
    tools: &[ToolDef],
    api_key: &str,
) -> StreamConfig {
    let yoagent_messages: Vec<yoagent::types::Message> = messages
        .iter()
        .map(|m| match m.role {
            Role::User => yoagent::types::Message::user(&m.content),
            Role::Assistant => {
                let mut content = Vec::new();
                if !m.content.is_empty() {
                    content.push(yoagent::types::Content::Text {
                        text: m.content.clone(),
                    });
                }
                for tc in &m.tool_calls {
                    content.push(yoagent::types::Content::ToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                        provider_metadata: None,
                    });
                }
                yoagent::types::Message::Assistant {
                    content,
                    stop_reason: yoagent::types::StopReason::Stop,
                    model: model.to_string(),
                    provider: String::new(),
                    usage: yoagent::types::Usage::default(),
                    timestamp: 0,
                    error_message: None,
                }
            }
            Role::ToolResult => {
                let content = vec![yoagent::types::Content::Text {
                    text: m.content.clone(),
                }];
                yoagent::types::Message::ToolResult {
                    tool_call_id: m.tool_call_id.clone().unwrap_or_default(),
                    tool_name: String::new(),
                    content,
                    is_error: m.is_error,
                    timestamp: 0,
                }
            }
        })
        .collect();

    let yoagent_tools: Vec<ToolDefinition> = tools
        .iter()
        .map(|t| ToolDefinition {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
        })
        .collect();

    StreamConfig {
        model: model.to_string(),
        system_prompt: system_prompt.to_string(),
        messages: yoagent_messages,
        tools: yoagent_tools,
        thinking_level: THINKING_LEVEL,
        api_key: api_key.to_string(),
        max_tokens: None,
        temperature: None,
        model_config: Some(opencode_model_config()),
        cache_config: yoagent::types::CacheConfig::default(),
    }
}

/// A yoagent-backed provider that implements rab's `Provider` trait.
/// This is the bridge. Once the loop is replaced by yoagent's loop, this goes away.
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
        let config = build_stream_config(model, system_prompt, messages, tools, &self.api_key);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = tokio_util::sync::CancellationToken::new();

        // Buffer for accumulating tool calls from Start/Delta/End triples.
        let mut partial_tool_calls: HashMap<usize, ToolCallAccum> = HashMap::new();

        // Spawn the yoagent provider; it sends StreamEvents to tx.
        // OpenAiCompatProvider is a ZST, so we create a fresh one.
        let provider = yoagent::provider::OpenAiCompatProvider;
        tokio::spawn(async move {
            let _ = provider.stream(config, tx, cancel).await;
        });

        // Convert the mpsc channel into a Stream of rab StreamEvents.
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
                        partial_tool_calls.insert(content_index, ToolCallAccum {
                            id,
                            name,
                            arguments: String::new(),
                        });
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
                        let (text, tool_calls, usage, stop_reason) = extract_from_message(&message);
                        yield StreamEvent::Done {
                            text,
                            usage,
                            stop_reason,
                            tool_calls,
                        };
                    }
                    yoagent::provider::traits::StreamEvent::Error { message: err_msg } => {
                        let error_text = match &err_msg {
                            yoagent::types::Message::Assistant { error_message: Some(e), .. } => e.clone(),
                            _ => String::new(),
                        };
                        yield StreamEvent::Error {
                            message: error_text,
                        };
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    fn set_reasoning_effort(&self, _level: Option<&str>) {
        // Hardcoded to High, no-op
    }
}

struct ToolCallAccum {
    id: String,
    name: String,
    arguments: String,
}

/// Extract rab types from a yoagent `Message`.
fn extract_from_message(
    msg: &yoagent::types::Message,
) -> (String, Vec<ToolCall>, Usage, StopReason) {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut usage = Usage::default();
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
                yoagent::types::Content::Text { text: t } => text.push_str(t),
                yoagent::types::Content::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    tool_calls.push(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                }
                yoagent::types::Content::Thinking { .. } => {}
                yoagent::types::Content::Image { .. } => {}
            }
        }

        stop_reason = match sr {
            yoagent::types::StopReason::Stop => StopReason::EndTurn,
            yoagent::types::StopReason::ToolUse => StopReason::ToolUse,
            yoagent::types::StopReason::Length => StopReason::MaxTokens,
            yoagent::types::StopReason::Error => StopReason::Error,
            yoagent::types::StopReason::Aborted => StopReason::EndTurn,
        };

        usage = Usage {
            input_tokens: Some(u.input as i32),
            output_tokens: Some(u.output as i32),
            cache_tokens: Some(u.cache_read as i32),
            cache_write_tokens: Some(u.cache_write as i32),
            cost_total: None,
        };
    }

    (text, tool_calls, usage, stop_reason)
}
