//! Thin wrapper around `yoagent::provider::AnthropicProvider`.
//!
//! Upstream hardcodes `provider: "anthropic"` in the assistant message.
//! This wrapper corrects it to the per-model provider from `ModelConfig`
//! so that cost tracking and display use the correct provider name.

use async_trait::async_trait;
use tokio::sync::mpsc;
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
        let provider_override = config.model_config.as_ref().map(|mc| mc.provider.clone());

        let mut message = yoagent::provider::AnthropicProvider
            .stream(config, tx, cancel)
            .await?;

        // Upstream hardcodes "anthropic"; fix to the actual provider name.
        if let Some(provider) = provider_override
            && let Message::Assistant { provider: p, .. } = &mut message
        {
            *p = provider;
        }

        Ok(message)
    }
}
