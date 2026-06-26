//! Adapter helpers for implementing `yoagent::types::AgentTool` on rab tools.
//!
//! Each builtin tool adds a yoagent AgentTool impl that delegates to its rab
//! AgentTool impl. This file provides the shared bridging logic for `execute()`.

use crate::agent::extension::{AgentTool as RabAgentTool, Cancel, ToolOutput};
use async_trait::async_trait;
use tokio::sync::mpsc;
use yoagent::types::{AgentTool as YoAgentTool, Content, ToolContext, ToolError, ToolResult};

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
