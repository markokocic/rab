//! Custom OpenAI-compatible streaming provider with pi-level compat support.
//!
//! Replaces `yoagent::provider::OpenAiCompatProvider` with richer compat handling:
//! - DeepSeek `thinking: { type }` format (not `reasoning_effort`)
//! - `reasoning_content` on replayed assistant messages
//! - Configurable max_tokens field name
//! - All pi `OpenAICompletionsCompat` flags

use super::compat::{RabMaxTokensField, RabOpenAiCompat, RabThinkingFormat};
use crate::tls;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::EventSource;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use yoagent::provider::model::ModelConfig;
use yoagent::provider::traits::*;
use yoagent::types::*;

/// Our custom OpenAI-compatible streaming provider.
///
/// Rich compat is passed directly via the `compat` field (no longer read
/// from `ModelConfig::headers`). Falls back to yoagent's `OpenAiCompat`
/// if absent.
pub struct RabOpenAiCompatProvider {
    pub compat: RabOpenAiCompat,
}

impl RabOpenAiCompatProvider {
    pub fn new(compat: RabOpenAiCompat) -> Self {
        Self { compat }
    }
}

#[async_trait]
impl StreamProvider for RabOpenAiCompatProvider {
    async fn stream(
        &self,
        config: StreamConfig,
        tx: mpsc::UnboundedSender<StreamEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Message, ProviderError> {
        let model_config = config.model_config.as_ref().ok_or_else(|| {
            ProviderError::Other("ModelConfig required for OpenAI provider".into())
        })?;

        let rab_compat = &self.compat;

        let base_url = &model_config.base_url;
        let url = format!("{}/chat/completions", base_url);

        let body = build_request_body(&config, model_config, rab_compat);
        debug!("OpenAI compat request: model={} url={}", config.model, url);

        let client = tls::reqwest_client();
        let mut request = client
            .post(&url)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", config.api_key));

        // Add any extra headers from model config.
        // Skip `anthropic-*` headers injected for Anthropic provider models
        // at resolution time — they must not be sent to OpenAI-compatible endpoints.
        for (k, v) in &model_config.headers {
            if !k.starts_with("anthropic-") {
                request = request.header(k, v);
            }
        }

        let request = request.json(&body);

        let mut es =
            EventSource::new(request).map_err(|e| ProviderError::Network(e.to_string()))?;

        let mut content: Vec<Content> = Vec::new();
        let mut usage = Usage::default();
        let mut stop_reason = StopReason::Stop;
        let mut saw_finish_reason = false;
        let mut tool_call_buffers: Vec<ToolCallBuffer> = Vec::new();

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
                        Some(Ok(reqwest_eventsource::Event::Open)) => {}
                        Some(Ok(reqwest_eventsource::Event::Message(msg))) => {
                            if msg.data == "[DONE]" {
                                break;
                            }

                            let chunk: OpenAiChunk = match serde_json::from_str(&msg.data) {
                                Ok(c) => c,
                                Err(e) => {
                                    debug!("Failed to parse OpenAI chunk: {} data={}", e, &msg.data);
                                    continue;
                                }
                            };

                            // Process usage
                            if let Some(u) = &chunk.usage {
                                let cache_read = u
                                    .prompt_cache_hit_tokens
                                    .or_else(|| {
                                        u.prompt_tokens_details.as_ref().map(|d| d.cached_tokens)
                                    })
                                    .unwrap_or(0);
                                usage.input = u.prompt_cache_miss_tokens.unwrap_or_else(|| {
                                    u.prompt_tokens.saturating_sub(cache_read)
                                });
                                usage.output = u.completion_tokens;
                                usage.total_tokens = u.total_tokens;
                                usage.cache_read = cache_read;
                            }

                            for choice in &chunk.choices {
                                let delta = &choice.delta;

                                // Handle reasoning/thinking content
                                let reasoning = match rab_compat.thinking_format {
                                    RabThinkingFormat::DeepSeek
                                    | RabThinkingFormat::OpenAi
                                    | RabThinkingFormat::OpenRouter
                                    | RabThinkingFormat::Together
                                    | RabThinkingFormat::Zai
                                    | RabThinkingFormat::Qwen
                                    | RabThinkingFormat::ChatTemplate
                                    | RabThinkingFormat::QwenChatTemplate
                                    | RabThinkingFormat::StringThinking
                                    | RabThinkingFormat::AntLing => delta.reasoning_content.as_deref(),
                                };
                                if let Some(reasoning_text) = reasoning {
                                    let thinking_idx = content.iter().position(|c| matches!(c, Content::Thinking { .. }));
                                    let idx = match thinking_idx {
                                        Some(i) => i,
                                        None => {
                                            content.push(Content::thinking(String::new()));
                                            content.len() - 1
                                        }
                                    };
                                    if let Some(Content::Thinking { thinking, .. }) = content.get_mut(idx) {
                                        thinking.push_str(reasoning_text);
                                    }
                                    let _ = tx.send(StreamEvent::ThinkingDelta {
                                        content_index: idx,
                                        delta: reasoning_text.to_string(),
                                    });
                                }

                                // Handle text content
                                if let Some(text) = &delta.content {
                                    let text_idx = content.iter().position(|c| matches!(c, Content::Text { .. }));
                                    let idx = match text_idx {
                                        Some(i) => i,
                                        None => {
                                            content.push(Content::Text { text: String::new() });
                                            content.len() - 1
                                        }
                                    };
                                    if let Some(Content::Text { text: t }) = content.get_mut(idx) {
                                        t.push_str(text);
                                    }
                                    let _ = tx.send(StreamEvent::TextDelta {
                                        content_index: idx,
                                        delta: text.clone(),
                                    });
                                }

                                // Handle tool calls
                                if let Some(tool_calls) = &delta.tool_calls {
                                    for tc in tool_calls {
                                        let tc_index = tc.index as usize;
                                        while tool_call_buffers.len() <= tc_index {
                                            tool_call_buffers.push(ToolCallBuffer::default());
                                        }
                                        let buf = &mut tool_call_buffers[tc_index];
                                        if let Some(id) = &tc.id {
                                            buf.id = id.clone();
                                        }
                                        if let Some(f) = &tc.function {
                                            if let Some(name) = &f.name {
                                                buf.name.clone_from(name);
                                                let _ = tx.send(StreamEvent::ToolCallStart {
                                                    content_index: content.len() + tc_index,
                                                    id: buf.id.clone(),
                                                    name: name.clone(),
                                                });
                                            }
                                            if let Some(args) = &f.arguments {
                                                buf.arguments.push_str(args);
                                                let _ = tx.send(StreamEvent::ToolCallDelta {
                                                    content_index: content.len() + tc_index,
                                                    delta: args.clone(),
                                                });
                                            }
                                        }
                                    }
                                }

                                // Handle finish reason
                                if let Some(reason) = &choice.finish_reason {
                                    saw_finish_reason = true;
                                    stop_reason = match reason.as_str() {
                                        "stop" => StopReason::Stop,
                                        "length" => StopReason::Length,
                                        "tool_calls" => StopReason::ToolUse,
                                        _ => StopReason::Stop,
                                    };
                                }
                            }
                        }
                        // Some providers (e.g. MiniMax) close the connection
                        // without the OpenAI-standard `data: [DONE]` terminator.
                        // If a finish_reason was already received, the response
                        // is complete — treat as clean EOF. (This eventsource
                        // surfaces every body close as StreamEnded; network
                        // drops surface as Transport instead.) A StreamEnded
                        // with NO finish_reason is genuine truncation and
                        // stays an error.
                        Some(Err(reqwest_eventsource::Error::StreamEnded)) if saw_finish_reason => {
                            debug!("provider closed stream without [DONE] after finish_reason");
                            break;
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
                            warn!("OpenAI SSE error: {}", provider_err);
                            return Err(provider_err);
                        }
                    }
                }
            }
        }

        // Finalize tool calls
        for buf in &tool_call_buffers {
            let args = serde_json::from_str(&buf.arguments).unwrap_or_else(|e| {
                if !buf.arguments.is_empty() {
                    warn!(
                        tool = %buf.name,
                        len = buf.arguments.len(),
                        "tool-call arguments failed to parse ({e}); using empty object"
                    );
                }
                serde_json::Value::Object(Default::default())
            });
            content.push(Content::tool_call(buf.id.clone(), buf.name.clone(), args));
            let _ = tx.send(StreamEvent::ToolCallEnd {
                content_index: content.len() - 1,
            });
        }

        if !tool_call_buffers.is_empty() {
            stop_reason = StopReason::ToolUse;
        }

        let message = Message::assistant(
            content,
            stop_reason,
            config.model.clone(),
            model_config.provider.clone(),
            usage,
        )
        .with_timestamp(now_ms());

        let _ = tx.send(StreamEvent::Done {
            message: message.clone(),
        });
        Ok(message)
    }
}

#[derive(Default)]
struct ToolCallBuffer {
    id: String,
    name: String,
    arguments: String,
}

fn build_request_body(
    config: &StreamConfig,
    model_config: &ModelConfig,
    rab_compat: &RabOpenAiCompat,
) -> serde_json::Value {
    let mut messages: Vec<serde_json::Value> = Vec::new();

    // System prompt
    if !config.system_prompt.is_empty() {
        let role = if rab_compat.supports_developer_role {
            "developer"
        } else {
            "system"
        };
        messages.push(serde_json::json!({
            "role": role,
            "content": config.system_prompt,
        }));
    }

    for msg in &config.messages {
        match msg {
            Message::User { content, .. } => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": content_to_openai(content, rab_compat),
                }));
            }
            Message::Assistant { content, .. } => {
                let mut parts: Vec<serde_json::Value> = Vec::new();
                let mut tool_calls: Vec<serde_json::Value> = Vec::new();
                let mut reasoning_content: Option<String> = None;

                for c in content {
                    match c {
                        Content::Text { text } if text.is_empty() => {}
                        Content::Text { text } => {
                            parts.push(serde_json::json!({"type": "text", "text": text}));
                        }
                        Content::Thinking { thinking, .. } => {
                            // DeepSeek requires reasoning_content on replayed assistant messages
                            if rab_compat.requires_reasoning_content_on_assistant_messages {
                                reasoning_content = Some(thinking.clone());
                            } else {
                                parts.push(serde_json::json!({"type": "text", "text": thinking}));
                            }
                        }
                        Content::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            tool_calls.push(serde_json::json!({
                                "id": id,
                                "type": "function",
                                "function": {"name": name, "arguments": arguments.to_string()},
                            }));
                        }
                        _ => {}
                    }
                }

                // Skip assistant messages with no content and no tool calls.
                // Some providers (e.g. DeepSeek) require "content or tool_calls must be set".
                // Mirrors pi's guard in openai-completions.ts which prevents API errors
                // from aborted/partial responses that got no content.
                if parts.is_empty() && tool_calls.is_empty() {
                    continue;
                }

                let mut msg_obj = serde_json::json!({"role": "assistant"});
                if !parts.is_empty() {
                    msg_obj["content"] = serde_json::json!(parts);
                }
                if !tool_calls.is_empty() {
                    msg_obj["tool_calls"] = serde_json::json!(tool_calls);
                }
                if let Some(rc) = reasoning_content {
                    msg_obj["reasoning_content"] = serde_json::json!(rc);
                }
                messages.push(msg_obj);
            }
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                ..
            } => {
                let content_val = if content.iter().any(|c| matches!(c, Content::Image { .. })) {
                    content_to_openai(content, rab_compat)
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

                let mut msg_obj = serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "content": content_val,
                });
                if rab_compat.requires_tool_result_name {
                    msg_obj["name"] = serde_json::json!(tool_name);
                }
                messages.push(msg_obj);
            }
        }
    }
    maybe_insert_assistant_after_tool_results(&mut messages, rab_compat);

    let max_tokens_val = config.max_tokens.unwrap_or(model_config.max_tokens);
    let mut body = serde_json::json!({
        "model": config.model,
        "stream": true,
        "stream_options": {"include_usage": rab_compat.supports_usage_in_streaming},
        "messages": messages,
    });

    match rab_compat.max_tokens_field {
        RabMaxTokensField::MaxCompletionTokens => {
            body["max_completion_tokens"] = serde_json::json!(max_tokens_val);
        }
        RabMaxTokensField::MaxTokens => {
            body["max_tokens"] = serde_json::json!(max_tokens_val);
        }
    }

    // Thinking control — DeepSeek uses thinking.type, not reasoning_effort
    if rab_compat.supports_thinking_control {
        let thinking_type = if config.thinking_level == ThinkingLevel::Off {
            "disabled"
        } else {
            "enabled"
        };
        body["thinking"] = serde_json::json!({ "type": thinking_type });
    }

    if !config.tools.is_empty() {
        let tools: Vec<serde_json::Value> = config
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::json!(tools);
    }

    // reasoning_effort — only if the provider supports it
    if config.thinking_level != ThinkingLevel::Off && rab_compat.supports_reasoning_effort {
        let effort = match config.thinking_level {
            ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::Off => unreachable!(),
        };
        body["reasoning_effort"] = serde_json::json!(effort);
    }

    if let Some(temp) = config.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    body
}

fn maybe_insert_assistant_after_tool_results(
    messages: &mut Vec<serde_json::Value>,
    rab_compat: &RabOpenAiCompat,
) {
    if !rab_compat.requires_assistant_after_tool_result {
        return;
    }

    let last_is_tool = messages
        .last()
        .and_then(|m| m.get("role"))
        .and_then(|role| role.as_str())
        == Some("tool");
    if last_is_tool {
        messages.push(serde_json::json!({
            "role": "assistant",
            "content": "",
        }));
    }
}

fn content_to_openai(content: &[Content], _rab_compat: &RabOpenAiCompat) -> serde_json::Value {
    if content.len() == 1
        && let Content::Text { text } = &content[0]
        && !text.is_empty()
    {
        return serde_json::json!(text);
    }
    let parts: Vec<serde_json::Value> = content
        .iter()
        .filter(|c| !matches!(c, Content::Text { text } if text.is_empty()))
        .filter_map(|c| match c {
            Content::Text { text } => Some(serde_json::json!({"type": "text", "text": text})),
            Content::Image { data, mime_type } => Some(serde_json::json!({
                "type": "image_url",
                "image_url": {"url": format!("data:{};base64,{}", mime_type, data)},
            })),
            _ => None,
        })
        .collect();
    serde_json::json!(parts)
}

// ── SSE response types ─────────────────────────────────────────────

#[derive(Deserialize)]
struct OpenAiChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    delta: OpenAiDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct OpenAiDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Deserialize)]
struct OpenAiToolCallDelta {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Deserialize)]
struct OpenAiFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    prompt_cache_hit_tokens: Option<u64>,
    #[serde(default)]
    prompt_cache_miss_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: u64,
}
