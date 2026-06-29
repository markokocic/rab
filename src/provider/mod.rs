//! Provider and model system.
//!
//! Loads a built-in model catalog from `models.json` in this directory,
//! overlays user overrides from `~/.rab/agent/models.json`,
//! and provides the right `StreamProvider` for each model's API protocol.

use std::path::Path;

use anyhow::bail;
use yoagent::provider::model::ModelConfig;

pub mod compat;
pub mod models;
pub mod openai_compat;
pub mod update;

/// A resolved model ready for use by the agent.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// The yoagent ModelConfig with correct base URL, compat, pricing, etc.
    pub model_config: ModelConfig,
    /// The API key for this provider (from auth.json or env var).
    pub api_key: String,
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
        let builtin_json = include_str!("models.json");
        let builtin = models::load_builtin(builtin_json)?;

        let user_path = agent_dir.join("models.json");
        let user = models::load_user(&user_path)?;

        let entries = models::merge(builtin, user);
        let auth_storage = crate::auth::AuthStorage::load()?;

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
    /// Scans all providers for a matching model ID. Returns the first match.
    /// Also resolves the API key for that provider.
    pub fn resolve(&self, model_id: &str) -> anyhow::Result<ResolvedModel> {
        for entry in &self.entries {
            if let Some(model_config) = entry.models.iter().find(|m| m.id == model_id) {
                let api_key = self
                    .auth_storage
                    .api_key(&entry.id)
                    .or_else(|| {
                        // Fallback: check environment variable
                        let env_var = entry.env_var_name();
                        std::env::var(env_var).ok()
                    })
                    .unwrap_or_default();

                return Ok(ResolvedModel {
                    model_config: model_config.clone(),
                    api_key,
                });
            }
        }

        bail!(
            "Unknown model '{}'. Available models: {}",
            model_id,
            self.list_models().join(", ")
        );
    }

    /// List all available model IDs (for UI selector and /model command).
    pub fn list_models(&self) -> Vec<String> {
        let mut models: Vec<String> = Vec::new();
        for entry in &self.entries {
            for m in &entry.models {
                models.push(m.id.clone());
            }
        }
        models.sort();
        models
    }

    /// Get the provider name for a model ID.
    pub fn provider_for_model(&self, model_id: &str) -> Option<String> {
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
}

/// Get the agent config directory (~/.rab/agent).
pub fn get_agent_dir() -> std::path::PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".rab").join("agent"))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/.rab/agent"))
}
