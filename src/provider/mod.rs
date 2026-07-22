//! Provider and model system.
//!
//! Loads a built-in model catalog from `models.json` in this directory,
//! overlays user overrides from `~/.rab/agent/models.json`,
//! and provides the right `StreamProvider` for each model's API protocol.

use std::collections::HashMap;
use std::path::Path;

use anyhow::bail;
use yoagent::provider::model::{CostConfig, ModelConfig};
use yoagent::types::Usage;

use crate::provider::compat::RabOpenAiCompat;

pub mod anthropic;
pub mod compat;
pub mod generate_models;
pub mod models;
pub mod oauth;
pub mod openai_compat;

/// A resolved model ready for use by the agent.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// The yoagent ModelConfig with correct base URL, compat, pricing, etc.
    pub model_config: ModelConfig,
    /// The API key for this provider (from auth.json or env var).
    pub api_key: String,
    /// Rich rab compat flags for this model.
    pub rab_compat: RabOpenAiCompat,
    /// Thinking-level filter map for this model (model_id → supported levels).
    pub thinking_map: Option<HashMap<String, serde_json::Value>>,
}

/// The provider registry — holds all known providers and their models.
pub struct ProviderRegistry {
    entries: Vec<models::ProviderEntry>,
    /// Auth storage for API key lookups.
    auth_storage: crate::auth::AuthStorage,
}

impl ProviderRegistry {
    /// Load the provider registry from built-in + user models.json.
    pub fn load(agent_dir: &Path) -> anyhow::Result<Self> {
        // Register built-in OAuth providers once
        crate::provider::oauth::register_builtins();

        let builtin_json = include_str!("models.json");
        let builtin = models::load_builtin(builtin_json)?;

        let user_path = agent_dir.join("models.json");
        let user = models::load_user(&user_path)?;

        let entries = models::merge(builtin, user);
        let auth_storage = crate::auth::AuthStorage::create()?;

        Ok(Self {
            entries,
            auth_storage,
        })
    }

    /// Reload from disk (for /reload support).
    pub fn reload(&mut self, agent_dir: &Path) -> anyhow::Result<()> {
        let fresh = Self::load(agent_dir)?;
        self.entries = fresh.entries;
        self.auth_storage = fresh.auth_storage;
        Ok(())
    }

    /// Resolve a model ID (e.g. "deepseek-v4-flash") to a `ResolvedModel`.
    ///
    /// Scans all providers for a matching model ID. If `preferred_provider` is
    /// set, that provider is checked first; otherwise returns the first match.
    /// Also resolves the API key for that provider.
    pub fn resolve(
        &self,
        model_id: &str,
        preferred_provider: Option<&str>,
    ) -> anyhow::Result<ResolvedModel> {
        // Try preferred provider first when specified.
        if let Some(preferred) = preferred_provider
            && let Some(result) = self.resolve_from_provider(model_id, preferred)
        {
            return Ok(result);
        }

        for entry in &self.entries {
            if let Some(model_config) = entry.models.iter().find(|m| m.id == model_id) {
                let api_key = self
                    .auth_storage
                    .api_key(&entry.id)
                    .or_else(|| {
                        // Check for valid OAuth access token
                        self.auth_storage.oauth_token(&entry.id)
                    })
                    .or_else(|| {
                        // Even if past the 5-minute buffer, still use the token
                        // as long as it's not truly expired.
                        self.auth_storage.oauth_token_past_buffer(&entry.id)
                    })
                    .or_else(|| {
                        // Fallback: check environment variable
                        let env_var = entry.env_var_name();
                        std::env::var(env_var).ok()
                    })
                    .unwrap_or_default();

                let mut model_config = model_config.clone();

                // For GitHub Copilot, derive the API base URL from the OAuth
                // token's proxy-ep field (pi-compatible dynamic endpoint).
                if entry.id == "github-copilot" {
                    let enterprise_domain =
                        self.auth_storage
                            .oauth_credential(&entry.id)
                            .and_then(|c| match c {
                                crate::auth::AuthCredential::Oauth { enterprise_url, .. } => {
                                    enterprise_url
                                }
                                _ => None,
                            });
                    let derived = crate::provider::oauth::github_copilot::get_copilot_base_url(
                        Some(&api_key),
                        enterprise_domain.as_deref(),
                    );
                    model_config.base_url = derived;

                    // Inject Copilot OAuth headers for proxy-ep tokens.
                    inject_github_copilot_headers(&mut model_config, &api_key);
                }

                return Ok(ResolvedModel {
                    model_config,
                    api_key,
                    rab_compat: entry.compats.get(model_id).cloned().unwrap_or_default(),
                    thinking_map: entry.thinking_maps.get(model_id).cloned(),
                });
            }
        }

        bail!(
            "Unknown model '{}'. Available models: {}",
            model_id,
            self.list_models().join(", ")
        );
    }

    /// Resolve from a specific provider. Returns `None` if the provider doesn't
    /// exist or doesn't have the given model.
    fn resolve_from_provider(&self, model_id: &str, provider_id: &str) -> Option<ResolvedModel> {
        let entry = self.entries.iter().find(|e| e.id == provider_id)?;
        let mut model_config = entry.models.iter().find(|m| m.id == model_id)?.clone();
        let api_key = self
            .auth_storage
            .api_key(provider_id)
            .or_else(|| {
                // Check for valid OAuth access token
                self.auth_storage.oauth_token(provider_id)
            })
            .or_else(|| {
                // Even if past the 5-minute buffer, still use the token
                // as long as it's not truly expired.
                self.auth_storage.oauth_token_past_buffer(provider_id)
            })
            .or_else(|| {
                let env_var = entry.env_var_name();
                std::env::var(env_var).ok()
            })
            .unwrap_or_default();

        // For GitHub Copilot, derive the API base URL from the OAuth
        // token's proxy-ep field (pi-compatible dynamic endpoint).
        if provider_id == "github-copilot" {
            let enterprise_domain = self
                .auth_storage
                .oauth_credential(provider_id)
                .and_then(|c| match c {
                    crate::auth::AuthCredential::Oauth { enterprise_url, .. } => enterprise_url,
                    _ => None,
                });
            let derived = crate::provider::oauth::github_copilot::get_copilot_base_url(
                Some(&api_key),
                enterprise_domain.as_deref(),
            );
            model_config.base_url = derived;

            // Inject Copilot OAuth headers for proxy-ep tokens.
            inject_github_copilot_headers(&mut model_config, &api_key);
        }

        Some(ResolvedModel {
            model_config,
            api_key,
            rab_compat: entry.compats.get(model_id).cloned().unwrap_or_default(),
            thinking_map: entry.thinking_maps.get(model_id).cloned(),
        })
    }

    /// List all available model IDs (for UI selector and /model command).
    /// Deduplicated: each model ID appears only once even if registered
    /// under multiple providers.
    pub fn list_models(&self) -> Vec<String> {
        let mut model_set = std::collections::BTreeSet::new();
        for entry in &self.entries {
            for m in &entry.models {
                model_set.insert(m.id.clone());
            }
        }
        model_set.into_iter().collect()
    }

    /// List model IDs from providers that have valid authentication.
    /// Used by the model cycle and selector to hide unconfigured providers.
    pub fn list_authenticated_model_ids(&self) -> Vec<String> {
        let mut model_set = std::collections::BTreeSet::new();
        for entry in &self.entries {
            if self.provider_has_auth(&entry.id) {
                for m in &entry.models {
                    model_set.insert(m.id.clone());
                }
            }
        }
        model_set.into_iter().collect()
    }

    /// List all (provider, model_id, model_name) tuples, one per provider entry.
    /// Unlike `list_models()`, the same model ID can appear under multiple
    /// providers. Used by the model selector to show provider-prefixed entries.
    pub fn list_model_provider_tuples(&self) -> Vec<(String, String, String)> {
        let mut result = Vec::new();
        for entry in &self.entries {
            for m in &entry.models {
                result.push((entry.id.clone(), m.id.clone(), m.name.clone()));
            }
        }
        result
    }

    /// Get the provider name for a model ID.
    ///
    /// When `preferred_provider` is set and that provider has the model,
    /// returns the preferred provider. Otherwise returns the first match.
    pub fn provider_for_model(
        &self,
        model_id: &str,
        preferred_provider: Option<&str>,
    ) -> Option<String> {
        // Try preferred provider first.
        if let Some(preferred) = preferred_provider
            && self
                .entries
                .iter()
                .any(|e| e.id == preferred && e.models.iter().any(|m| m.id == model_id))
        {
            return Some(preferred.to_string());
        }

        for entry in &self.entries {
            if entry.models.iter().any(|m| m.id == model_id) {
                return Some(entry.id.clone());
            }
        }
        None
    }

    /// Get the API key for a provider.
    pub fn api_key_for_provider(&self, provider_id: &str) -> Option<String> {
        self.auth_storage.api_key(provider_id)
    }

    /// Count the number of distinct providers in the registry.
    pub fn count_providers(&self) -> usize {
        self.entries.len()
    }

    /// List all provider (id, name) tuples.
    pub fn list_providers(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .map(|e| (e.id.clone(), e.name.clone()))
            .collect()
    }

    /// Get the list of provider IDs that have stored credentials.
    pub fn configured_providers(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|e| {
                if self.auth_storage.api_key(&e.id).is_some() {
                    Some(e.id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check whether a provider has valid authentication (stored credential or env var).
    pub fn provider_has_auth(&self, provider_id: &str) -> bool {
        if self.auth_storage.api_key(provider_id).is_some()
            || self.auth_storage.oauth_token(provider_id).is_some()
        {
            return true;
        }
        // Check if this is an OAuth provider that could be logged in
        if crate::provider::oauth::is_built_in(provider_id) {
            return self.auth_storage.oauth_token(provider_id).is_some();
        }
        // Check env var
        self.entries
            .iter()
            .find(|e| e.id == provider_id)
            .and_then(|e| {
                let env_name = e.env_var_name();
                if std::env::var(env_name).is_ok() {
                    Some(())
                } else {
                    None
                }
            })
            .is_some()
    }

    /// Get auth status for a provider (for UI display).
    pub fn auth_status_for_provider(
        &self,
        provider_id: &str,
    ) -> crate::agent::ui::components::oauth_selector::ProviderAuthStatus {
        let has_stored = self.auth_storage.api_key(provider_id).is_some()
            || self.auth_storage.oauth_token(provider_id).is_some();

        // Check env var
        let env_var = self
            .entries
            .iter()
            .find(|e| e.id == provider_id)
            .and_then(|e| {
                let env_name = e.env_var_name();
                if std::env::var(env_name).is_ok() {
                    Some(env_name.to_string())
                } else {
                    None
                }
            });

        let configured = has_stored || env_var.is_some();
        let (source, label) = if has_stored {
            (Some("stored".to_string()), None)
        } else if let Some(env) = env_var {
            (Some("environment".to_string()), Some(env))
        } else {
            (None, None)
        };

        crate::agent::ui::components::oauth_selector::ProviderAuthStatus {
            configured,
            source,
            label,
        }
    }
}

/// Calculate the USD cost components of a usage record given a model's cost config.
///
/// Matches pi's `calculateCost()` in `packages/ai/src/models.ts`.
/// Returns `(input, output, cache_read, cache_write, total)`.
///
/// Note: pi also handles Anthropic's 1h cache write pricing (cacheWrite1h),
/// but yoagent's Usage struct doesn't expose that field, so it's omitted here.
pub fn calculate_cost(cost_config: &CostConfig, usage: &Usage) -> (f64, f64, f64, f64, f64) {
    let input_cost = (cost_config.input_per_million / 1_000_000.0) * usage.input as f64;
    let output_cost = (cost_config.output_per_million / 1_000_000.0) * usage.output as f64;
    let cache_read_cost =
        (cost_config.cache_read_per_million / 1_000_000.0) * usage.cache_read as f64;
    let cache_write_cost =
        (cost_config.cache_write_per_million / 1_000_000.0) * usage.cache_write as f64;
    let total = input_cost + output_cost + cache_read_cost + cache_write_cost;
    (
        input_cost,
        output_cost,
        cache_read_cost,
        cache_write_cost,
        total,
    )
}

/// Get the agent config directory (~/.rab/agent).
/// Inject GitHub Copilot OAuth headers into a model config when using
/// a `proxy-ep=` token (Claude Code OAuth).
fn inject_github_copilot_headers(model_config: &mut ModelConfig, api_key: &str) {
    if !api_key.contains("proxy-ep=") {
        return;
    }
    model_config
        .headers
        .entry("authorization".into())
        .or_insert_with(|| format!("Bearer {api_key}"));
    model_config
        .headers
        .entry("anthropic-beta".into())
        .or_insert_with(|| {
            "claude-code-20250219,oauth-2025-04-20,\
             fine-grained-tool-streaming-2025-05-14"
                .into()
        });
    model_config
        .headers
        .entry("anthropic-dangerous-direct-browser-access".into())
        .or_insert_with(|| "true".into());
    model_config
        .headers
        .entry("user-agent".into())
        .or_insert_with(|| "claude-cli/2.1.2 (external, cli)".into());
    model_config
        .headers
        .entry("x-app".into())
        .or_insert_with(|| "cli".into());
}

/// Inject provider-specific attribution and session headers into a ModelConfig.
///
/// Pi-compatible: mirrors pi's `mergeProviderAttributionHeaders` in
/// `provider-attribution.ts`. Adds:
/// - OpenCode: `x-opencode-session` + `x-opencode-client` (always, when session ID
///   is available and provider is opencode or base URL matches opencode.ai)
/// - OpenRouter: `HTTP-Referer`, `X-OpenRouter-Title`, `X-OpenRouter-Categories`
/// - NVIDIA NIM: `X-BILLING-INVOKE-ORIGIN`
///
/// Attribution headers (OpenRouter, NVIDIA) are only added when telemetry is enabled.
pub fn inject_provider_attribution_headers(
    model_config: &mut ModelConfig,
    session_id: Option<&str>,
    enable_telemetry: bool,
) {
    // Session headers for OpenCode (always sent when session ID is available).
    if let Some(sid) = session_id {
        let is_opencode = model_config.provider == "opencode"
            || model_config.provider == "opencode-go"
            || model_config.base_url.contains("opencode.ai");
        if is_opencode {
            model_config
                .headers
                .entry("x-opencode-session".into())
                .or_insert_with(|| sid.to_string());
            model_config
                .headers
                .entry("x-opencode-client".into())
                .or_insert_with(|| "rab".into());
        }
    }

    // Attribution headers (only when install telemetry is enabled).
    if enable_telemetry {
        if model_config.provider == "openrouter" || model_config.base_url.contains("openrouter.ai")
        {
            model_config
                .headers
                .entry("HTTP-Referer".into())
                .or_insert_with(|| "https://rab.dev".into());
            model_config
                .headers
                .entry("X-OpenRouter-Title".into())
                .or_insert_with(|| "rab".into());
            model_config
                .headers
                .entry("X-OpenRouter-Categories".into())
                .or_insert_with(|| "cli-agent".into());
        } else if model_config.provider == "nvidia"
            || model_config.base_url.contains("integrate.api.nvidia.com")
        {
            model_config
                .headers
                .entry("X-BILLING-INVOKE-ORIGIN".into())
                .or_insert_with(|| "Rab".into());
        }
    }
}

pub fn get_agent_dir() -> std::path::PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".rab").join("agent"))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/.rab/agent"))
}
