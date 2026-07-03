//! Custom Anthropic Messages API provider that uses `model_config.base_url`
//! and forwards `model_config.headers` — unlike yoagent's AnthropicProvider
//! which hardcodes `https://api.anthropic.com` and ignores headers.
//!
//! This allows GitHub Copilot (and other proxies) to serve Anthropic-format
//! models through their own endpoints.

use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use yoagent::provider::traits::*;
use yoagent::types::*;

pub struct RabAnthropicProvider;

#[async_trait]
impl StreamProvider for RabAnthropicProvider {
    async fn stream(
        &self,
        config: StreamConfig,
        tx: mpsc::UnboundedSender<StreamEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Message, ProviderError> {
        let model_config = config.model_config.as_ref().ok_or_else(|| {
            ProviderError::Other("ModelConfig required for Anthropic provider".into())
        })?;

        let base_url = model_config.base_url.trim_end_matches('/');
        let url = format!("{}/v1/messages", base_url);

        let is_oauth = config.api_key.contains("sk-ant-oat");
        // GitHub Copilot tokens use Bearer auth (not x-api-key).
        let is_copilot = config.api_key.contains("proxy-ep=");
        let body = build_request_body(&config, model_config, is_oauth);
        debug!(
            "RabAnthropic request: model={} url={} oauth={} copilot={}",
            config.model, url, is_oauth, is_copilot
        );

        let client = reqwest::Client::new();
        let mut builder = client
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");

        if is_oauth || is_copilot {
            builder = builder
                .header("authorization", format!("Bearer {}", config.api_key))
                .header(
                    "anthropic-beta",
                    "claude-code-20250219,oauth-2025-04-20,fine-grained-tool-streaming-2025-05-14",
                )
                .header("anthropic-dangerous-direct-browser-access", "true")
                .header("user-agent", "claude-cli/2.1.2 (external, cli)")
                .header("x-app", "cli");
        } else {
            builder = builder.header("x-api-key", &config.api_key);
        }

        // Forward custom headers from model config (e.g. GitHub Copilot)
        for (k, v) in &model_config.headers {
            builder = builder.header(k, v);
        }

        let request = builder.json(&body);

        let mut es =
            EventSource::new(request).map_err(|e| ProviderError::Network(e.to_string()))?;

        let mut content: Vec<Content> = Vec::new();
        let mut usage = Usage::default();
        let mut stop_reason = StopReason::Stop;

        let _ = tx.send(StreamEvent::Start);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    es.close();
                    return Err(ProviderError::Cancelled);
                }
                event = es.next() => {
                    match event {
                        None => break,
                        Some(Ok(Event::Open)) => {}
                        Some(Ok(Event::Message(msg))) => {
                            match msg.event.as_str() {
                                "message_start" => {
                                    if let Ok(data) = serde_json::from_str::<AnthropicMessageStart>(&msg.data) {
                                        usage.input = data.message.usage.input_tokens;
                                        usage.cache_read = data.message.usage.cache_read_input_tokens;
                                        usage.cache_write = data.message.usage.cache_creation_input_tokens;
                                    }
                                }
                                "content_block_start" => {
                                    if let Ok(data) = serde_json::from_str::<AnthropicContentBlockStart>(&msg.data) {
                                        let idx = data.index as usize;
                                        match data.content_block {
                                            AnthropicContentBlock::Text { .. } => {
                                                while content.len() <= idx {
                                                    content.push(Content::Text { text: String::new() });
                                                }
                                            }
                                            AnthropicContentBlock::Thinking { .. } => {
                                                while content.len() <= idx {
                                                    content.push(Content::Thinking { thinking: String::new(), signature: None });
                                                }
                                            }
                                            AnthropicContentBlock::ToolUse { id, name, .. } => {
                                                while content.len() <= idx {
                                                    content.push(Content::ToolCall { provider_metadata: None,
                                                        id: id.clone(),
                                                        name: name.clone(),
                                                        arguments: serde_json::Value::Object(Default::default()),
                                                    });
                                                }
                                                let _ = tx.send(StreamEvent::ToolCallStart {
                                                    content_index: idx,
                                                    id,
                                                    name,
                                                });
                                            }
                                        }
                                    }
                                }
                                "content_block_delta" => {
                                    if let Ok(data) = serde_json::from_str::<AnthropicContentBlockDelta>(&msg.data) {
                                        let idx = data.index as usize;
                                        match data.delta {
                                            AnthropicDelta::TextDelta { text } => {
                                                if let Some(Content::Text { text: t }) = content.get_mut(idx) {
                                                    t.push_str(&text);
                                                }
                                                let _ = tx.send(StreamEvent::TextDelta {
                                                    content_index: idx,
                                                    delta: text,
                                                });
                                            }
                                            AnthropicDelta::ThinkingDelta { thinking } => {
                                                if let Some(Content::Thinking { thinking: t, .. }) = content.get_mut(idx) {
                                                    t.push_str(&thinking);
                                                }
                                                let _ = tx.send(StreamEvent::ThinkingDelta {
                                                    content_index: idx,
                                                    delta: thinking,
                                                });
                                            }
                                            AnthropicDelta::InputJsonDelta { partial_json } => {
                                                if let Some(Content::ToolCall { arguments, .. }) = content.get_mut(idx) {
                                                    let buf = arguments
                                                        .as_object_mut()
                                                        .and_then(|o| o.get_mut("__partial_json"))
                                                        .and_then(|v| v.as_str().map(|s| s.to_string()));
                                                    let new_buf = format!("{}{}", buf.unwrap_or_default(), partial_json);
                                                    if let Some(obj) = arguments.as_object_mut() {
                                                        obj.insert("__partial_json".into(), serde_json::Value::String(new_buf));
                                                    }
                                                }
                                                let _ = tx.send(StreamEvent::ToolCallDelta {
                                                    content_index: idx,
                                                    delta: partial_json,
                                                });
                                            }
                                            AnthropicDelta::SignatureDelta { signature } => {
                                                if let Some(Content::Thinking { signature: s, .. }) = content.get_mut(idx) {
                                                    *s = Some(signature);
                                                }
                                            }
                                        }
                                    }
                                }
                                "content_block_stop" => {
                                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg.data) {
                                        let idx = data["index"].as_u64().unwrap_or(0) as usize;
                                        if let Some(Content::ToolCall { arguments, .. }) = content.get_mut(idx)
                                            && let Some(partial) = arguments.as_object()
                                                .and_then(|o| o.get("__partial_json"))
                                                .and_then(|v| v.as_str())
                                                .map(|s| s.to_string())
                                            {
                                                if let Ok(parsed) = serde_json::from_str(&partial) {
                                                    *arguments = parsed;
                                                } else {
                                                    warn!("Failed to parse tool call JSON: {}", partial);
                                                    *arguments = serde_json::Value::Object(Default::default());
                                                }
                                            }
                                        let _ = tx.send(StreamEvent::ToolCallEnd { content_index: idx });
                                    }
                                }
                                "message_delta" => {
                                    if let Ok(data) = serde_json::from_str::<AnthropicMessageDelta>(&msg.data) {
                                        stop_reason = match data.delta.stop_reason.as_deref() {
                                            Some("tool_use") => StopReason::ToolUse,
                                            Some("max_tokens") => StopReason::Length,
                                            _ => StopReason::Stop,
                                        };
                                        usage.output = data.usage.output_tokens;
                                    }
                                }
                                "message_stop" => break,
                                "ping" => {}
                                "error" => {
                                    let provider_err = classify_sse_error_event(&msg.data);
                                    warn!("Anthropic stream error: {}", provider_err);
                                    return Err(provider_err);
                                }
                                other => {
                                    debug!("Unknown Anthropic event: {}", other);
                                }
                            }
                        }
                        Some(Err(e)) => {
                            let provider_err = classify_eventsource_error(e).await;
                            // HTTP 421 Misdirected Request: caused by HTTP/2 connection
                            // coalescing right after login. Reclassify as retryable so
                            // the agent retries on a fresh connection.
                            let provider_err = match &provider_err {
                                ProviderError::Api(msg) if msg.contains("421") => {
                                    ProviderError::Network(msg.clone())
                                }
                                _ => provider_err,
                            };
                            warn!("SSE error: {}", provider_err);
                            return Err(provider_err);
                        }
                    }
                }
            }
        }

        let has_tool_calls = content
            .iter()
            .any(|c| matches!(c, Content::ToolCall { .. }));
        if has_tool_calls {
            stop_reason = StopReason::ToolUse;
        }

        let message = Message::Assistant {
            content,
            stop_reason,
            model: config.model.clone(),
            provider: model_config.provider.clone(),
            usage,
            timestamp: now_ms(),
            error_message: None,
        };

        let _ = tx.send(StreamEvent::Done {
            message: message.clone(),
        });
        Ok(message)
    }
}

// ---------------------------------------------------------------------------
// Anthropic API request/response types
// ---------------------------------------------------------------------------

fn build_request_body(
    config: &StreamConfig,
    model_config: &yoagent::provider::model::ModelConfig,
    is_oauth: bool,
) -> serde_json::Value {
    let mut messages: Vec<serde_json::Value> = Vec::new();

    for msg in &config.messages {
        match msg {
            Message::User { content, .. } => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": content_to_anthropic(content),
                }));
            }
            Message::Assistant { content, .. } => {
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content_to_anthropic(content),
                }));
            }
            Message::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                let result_content = if content.iter().any(|c| matches!(c, Content::Image { .. })) {
                    serde_json::json!(content_to_anthropic(content))
                } else {
                    let text = content
                        .iter()
                        .find_map(|c| match c {
                            Content::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    serde_json::json!(text)
                };

                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": result_content,
                        "is_error": is_error,
                    }],
                }));
            }
        }
    }

    // Prompt caching — place cache_control breakpoints
    let cache = &config.cache_config;
    let caching_enabled = cache.enabled && cache.strategy != CacheStrategy::Disabled;
    let (cache_system, cache_tools, cache_messages) = match &cache.strategy {
        CacheStrategy::Auto => (true, true, true),
        CacheStrategy::Disabled => (false, false, false),
        CacheStrategy::Manual {
            cache_system,
            cache_tools,
            cache_messages,
        } => (*cache_system, *cache_tools, *cache_messages),
    };

    if caching_enabled && cache_messages && messages.len() >= 2 {
        for idx in (0..messages.len() - 1).rev() {
            if let Some(content) = messages[idx]["content"].as_array_mut()
                && let Some(last_block) = content.last_mut()
            {
                let is_empty_text = last_block.get("type").and_then(|t| t.as_str()) == Some("text")
                    && last_block
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .is_empty();
                if !is_empty_text {
                    last_block["cache_control"] = serde_json::json!({"type": "ephemeral"});
                    break;
                }
            }
        }
    }

    let mut body = serde_json::json!({
        "model": config.model,
        "max_tokens": config.max_tokens.unwrap_or(model_config.max_tokens),
        "stream": true,
        "messages": messages,
    });

    // System prompt with cache breakpoint
    if is_oauth {
        let mut system_blocks = vec![serde_json::json!({
            "type": "text",
            "text": "You are Claude Code, Anthropic's official CLI for Claude.",
        })];
        if !config.system_prompt.is_empty() {
            system_blocks.push(serde_json::json!({
                "type": "text",
                "text": config.system_prompt,
            }));
        }
        if caching_enabled
            && cache_system
            && let Some(last) = system_blocks.last_mut()
        {
            last["cache_control"] = serde_json::json!({"type": "ephemeral"});
        }
        body["system"] = serde_json::json!(system_blocks);
    } else if !config.system_prompt.is_empty() {
        let mut block = serde_json::json!({
            "type": "text",
            "text": config.system_prompt,
        });
        if caching_enabled && cache_system {
            block["cache_control"] = serde_json::json!({"type": "ephemeral"});
        }
        body["system"] = serde_json::json!([block]);
    }

    // Tools with cache breakpoint
    if !config.tools.is_empty() {
        let mut tools: Vec<serde_json::Value> = config
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                })
            })
            .collect();
        if caching_enabled
            && cache_tools
            && let Some(last_tool) = tools.last_mut()
        {
            last_tool["cache_control"] = serde_json::json!({"type": "ephemeral"});
        }
        body["tools"] = serde_json::json!(tools);
    }

    if config.thinking_level != ThinkingLevel::Off {
        // Pi-compatible budget values (minimal=1024, low=2048, medium=8192, high=16384).
        // Cap budget to max_tokens - 1024 to guarantee max_tokens > budget_tokens.
        let budget_base: u32 = match config.thinking_level {
            ThinkingLevel::Minimal => 1024,
            ThinkingLevel::Low => 2048,
            ThinkingLevel::Medium => 8192,
            ThinkingLevel::High => 16384,
            ThinkingLevel::Off => 0,
        };
        let max_tokens = config.max_tokens.unwrap_or(model_config.max_tokens);
        let budget = budget_base.min(max_tokens.saturating_sub(1024));
        body["thinking"] = serde_json::json!({
            "type": "enabled",
            "budget_tokens": budget,
        });
    }

    if let Some(temp) = config.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    body
}

fn content_to_anthropic(content: &[Content]) -> Vec<serde_json::Value> {
    content
        .iter()
        .filter(|c| !matches!(c, Content::Text { text } if text.is_empty()))
        .map(|c| match c {
            Content::Text { text } => serde_json::json!({"type": "text", "text": text}),
            Content::Image { data, mime_type } => serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": mime_type, "data": data},
            }),
            Content::Thinking {
                thinking,
                signature,
            } => serde_json::json!({
                "type": "thinking",
                "thinking": thinking,
                "signature": signature.as_deref().unwrap_or(""),
            }),
            Content::ToolCall {
                id,
                name,
                arguments,
                ..
            } => serde_json::json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": arguments,
            }),
        })
        .collect()
}

// Anthropic SSE event types
#[derive(Deserialize)]
struct AnthropicMessageStart {
    message: AnthropicMessageInfo,
}

#[derive(Deserialize)]
struct AnthropicMessageInfo {
    usage: AnthropicUsage,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

#[derive(Deserialize)]
struct AnthropicContentBlockStart {
    index: u64,
    content_block: AnthropicContentBlock,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text {
        #[allow(dead_code)]
        text: String,
    },
    #[serde(rename = "thinking")]
    Thinking {
        #[allow(dead_code)]
        thinking: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Deserialize)]
struct AnthropicContentBlockDelta {
    index: u64,
    delta: AnthropicDelta,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::enum_variant_names)]
enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

#[derive(Deserialize)]
struct AnthropicMessageDelta {
    delta: AnthropicMessageDeltaInner,
    usage: AnthropicUsage,
}

#[derive(Deserialize)]
struct AnthropicMessageDeltaInner {
    stop_reason: Option<String>,
}
