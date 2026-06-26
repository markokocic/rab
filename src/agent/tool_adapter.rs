//! Adapter helpers for implementing `yoagent::types::AgentTool` on rab tools.
//!
//! Each builtin tool adds a yoagent AgentTool impl that delegates to its rab
//! AgentTool impl. This file provides the shared bridging logic for `execute()`.

use crate::agent::extension::{AgentTool as RabAgentTool, Cancel, ToolOutput};
use crate::agent::types::AgentMessage;
use async_trait::async_trait;
use tokio::sync::mpsc;
use yoagent::types::{AgentTool as YoAgentTool, Content, ToolContext, ToolError, ToolResult};

/// Build the ModelConfig for opencode-go's OpenAI-compatible gateway.
pub fn opencode_model_config() -> yoagent::provider::model::ModelConfig {
    yoagent::provider::model::ModelConfig::openai_compat(
        "https://opencode.ai/zen/go/v1",
        "deepseek-v4-flash",
        "opencode-go",
        yoagent::provider::model::OpenAiCompat::deepseek(),
    )
}

/// Call yoagent's provider for a simple text completion (no tools, no streaming).
/// Used by compaction and branch summary.
pub async fn summarize_text(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    messages: &[AgentMessage],
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

/// Wrap a rab AgentTool as a yoagent AgentTool by delegating execute().
pub struct RabToYoAgentTool {
    pub inner: Box<dyn RabAgentTool>,
}

#[async_trait]
impl YoAgentTool for RabToYoAgentTool {
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
    ) -> Result<ToolResult, ToolError> {
        let tool_call_id = ctx.tool_call_id.clone();

        // Bridge cancellation: yoagent CancellationToken → rab Cancel
        let rab_cancel = Cancel::new();
        let watch_cancel = rab_cancel.clone();
        let yo_cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            yo_cancel.cancelled().await;
            watch_cancel.cancel();
        });

        // Bridge on_update: rab UnboundedSender<ToolOutput> → yoagent callback
        let (update_tx, mut update_rx) = mpsc::unbounded_channel::<ToolOutput>();
        if let Some(ref on_update) = ctx.on_update {
            let on_update = on_update.clone();
            tokio::spawn(async move {
                while let Some(output) = update_rx.recv().await {
                    let content = vec![Content::Text {
                        text: output.content,
                    }];
                    let result = ToolResult {
                        content,
                        details: output.details.unwrap_or(serde_json::Value::Null),
                    };
                    on_update(result);
                }
            });
        }

        // Delegate to rab's execute
        match self
            .inner
            .execute(tool_call_id, params, rab_cancel, Some(update_tx.clone()))
            .await
        {
            Ok(output) => {
                let content = vec![Content::Text {
                    text: output.content,
                }];
                Ok(ToolResult {
                    content,
                    details: output.details.unwrap_or(serde_json::Value::Null),
                })
            }
            Err(e) => Err(ToolError::Failed(e.to_string())),
        }
    }
}
