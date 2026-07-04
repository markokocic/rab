//! Thin wrapper around `yoagent::provider::AnthropicProvider`.
//!
//! Adds rab-specific support:
//! - GitHub Copilot OAuth headers for `proxy-ep=` tokens
//! - Fixes the provider string from the hardcoded `"anthropic"` to the
//!   per-model provider from `ModelConfig`

use async_trait::async_trait;
use tokio::sync::mpsc;
use yoagent::provider::traits::*;
use yoagent::types::*;

pub struct RabAnthropicProvider;

#[async_trait]
impl StreamProvider for RabAnthropicProvider {
    async fn stream(
        &self,
        mut config: StreamConfig,
        tx: mpsc::UnboundedSender<StreamEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Message, ProviderError> {
        // Save the per-model provider string before consuming config.
        // Upstream hardcodes "anthropic"; we fix it to the actual provider.
        let provider_override = config.model_config.as_ref().map(|mc| mc.provider.clone());

        // GitHub Copilot tokens (proxy-ep=) need Bearer auth + Claude Code
        // identity headers, but don't start with sk-ant-oat so the upstream
        // OAuth path won't activate. Inject the necessary headers here.
        if config.api_key.contains("proxy-ep=")
            && let Some(mc) = &mut config.model_config
        {
            mc.headers
                .entry("authorization".into())
                .or_insert_with(|| format!("Bearer {}", config.api_key));
            mc.headers
                .entry("anthropic-beta".into())
                .or_insert_with(|| {
                    "claude-code-20250219,oauth-2025-04-20,\
                     fine-grained-tool-streaming-2025-05-14"
                        .into()
                });
            mc.headers
                .entry("anthropic-dangerous-direct-browser-access".into())
                .or_insert_with(|| "true".into());
            mc.headers
                .entry("user-agent".into())
                .or_insert_with(|| "claude-cli/2.1.2 (external, cli)".into());
            mc.headers
                .entry("x-app".into())
                .or_insert_with(|| "cli".into());
        }

        // Delegate all protocol handling to the upstream provider — it handles
        // SSE parsing, prompt caching, adaptive/legacy thinking, auth (OAuth,
        // bearer, x-api-key), custom headers, StopReason::Refusal, etc.
        let mut message = yoagent::provider::AnthropicProvider
            .stream(config, tx, cancel)
            .await?;

        // Fix provider string: upstream hardcodes "anthropic", but the actual
        // provider (e.g. "github-copilot", "opencode", "opencode-go") comes
        // from ModelConfig and is used for display and cost tracking.
        if let Some(provider) = provider_override
            && let Message::Assistant { provider: p, .. } = &mut message
        {
            *p = provider;
        }

        Ok(message)
    }
}
