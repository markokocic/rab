//! models.json — parses built-in and user models.json files
//! and constructs yoagent ModelConfigs with rab compat carried alongside.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use yoagent::provider::model::{ApiProtocol, CostConfig, ModelConfig};

use super::compat::RabOpenAiCompat;

/// Root structure of models.json
#[derive(Debug, Deserialize)]
struct ModelsJson {
    providers: HashMap<String, ProviderDef>,
}

/// A provider definition in models.json
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDef {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default)]
    models: Vec<ModelDef>,
}

/// A single model entry in models.json
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelDef {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    thinking_level_map: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    cost: Option<CostDef>,
    #[serde(default)]
    context_window: Option<u32>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    compat: Option<RabOpenAiCompat>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CostDef {
    input: f64,
    output: f64,
    #[serde(default)]
    cache_read: f64,
    #[serde(default)]
    cache_write: f64,
}

/// A resolved provider entry in the registry.
#[derive(Debug, Clone)]
pub struct ProviderEntry {
    pub id: String,
    pub name: String,
    pub models: Vec<ModelConfig>,
    /// Rich compat flags per model (keyed by model id).
    pub compats: HashMap<String, RabOpenAiCompat>,
    /// Thinking-level maps per model (keyed by model id).
    pub thinking_maps: HashMap<String, HashMap<String, serde_json::Value>>,
    pub env_var_hint: Option<String>,
}

impl ProviderEntry {
    pub fn env_var_name(&self) -> &str {
        self.env_var_hint.as_deref().unwrap_or("API_KEY")
    }
}

/// Parse a single provider definition from models.json into a `ProviderEntry`.
fn parse_provider(id: &str, def: ProviderDef) -> anyhow::Result<ProviderEntry> {
    let mut models = Vec::new();
    let provider_api = def.api.as_deref();

    for m in &def.models {
        let api_str = m
            .api
            .as_deref()
            .or(provider_api)
            .unwrap_or("openai-completions");
        let api = match api_str {
            "openai-completions" => ApiProtocol::OpenAiCompletions,
            "anthropic-messages" => ApiProtocol::AnthropicMessages,
            "openai-responses" => ApiProtocol::OpenAiResponses,
            "google-generative-ai" => ApiProtocol::GoogleGenerativeAi,
            "google-vertex" => ApiProtocol::GoogleVertex,
            "bedrock-converse-stream" => ApiProtocol::BedrockConverseStream,
            "azure-openai-responses" => ApiProtocol::AzureOpenAiResponses,
            "mistral-conversations" => ApiProtocol::OpenAiCompletions,
            _ => anyhow::bail!("Unknown API type: {}", api_str),
        };

        let base_url = m
            .base_url
            .clone()
            .or_else(|| def.base_url.clone())
            .unwrap_or_default();

        let cost = m
            .cost
            .as_ref()
            .map(|c| CostConfig {
                input_per_million: c.input,
                output_per_million: c.output,
                cache_read_per_million: c.cache_read,
                cache_write_per_million: c.cache_write,
            })
            .unwrap_or_default();

        let context_window = m.context_window.unwrap_or(128_000);
        let max_tokens = m.max_tokens.unwrap_or(16_384);

        // Collect user-specified headers only (no internal metadata)
        let mut headers = HashMap::new();
        if let Some(provider_headers) = &def.headers {
            for (k, v) in provider_headers {
                headers.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        if let Some(model_headers) = &m.headers {
            for (k, v) in model_headers {
                headers.insert(k.clone(), v.clone());
            }
        }

        let mut model = ModelConfig::custom(
            api,
            id.to_string(),
            base_url,
            m.id.clone(),
            m.name.clone().unwrap_or_else(|| m.id.clone()),
        );
        model.reasoning = m.reasoning;
        model.context_window = context_window;
        model.max_tokens = max_tokens;
        model.cost = cost;
        model.headers = headers;
        models.push(model);
    }

    // Collect compat and thinking_map per model id
    let mut compats = HashMap::new();
    let mut thinking_maps = HashMap::new();
    for m in &def.models {
        if let Some(ref compat) = m.compat {
            compats.insert(m.id.clone(), compat.clone());
        }
        if let Some(ref tlm) = m.thinking_level_map {
            thinking_maps.insert(m.id.clone(), tlm.clone());
        }
    }

    let env_var = def.env.as_ref().and_then(|e| e.get("apiKey")).cloned();

    Ok(ProviderEntry {
        id: id.to_string(),
        name: def.name.unwrap_or_else(|| id.to_string()),
        models,
        compats,
        thinking_maps,
        env_var_hint: env_var,
    })
}

/// Load providers from an embedded JSON string (from `include_str!`).
pub fn load_builtin(builtin_json: &str) -> anyhow::Result<Vec<ProviderEntry>> {
    let parsed: ModelsJson =
        serde_json::from_str(builtin_json).context("Failed to parse built-in models.json")?;

    let mut entries = Vec::new();
    for (id, def) in parsed.providers {
        match parse_provider(&id, def) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                eprintln!("Warning: skipping provider '{}': {}", id, e);
            }
        }
    }
    Ok(entries)
}

/// Load providers from a user's models.json file (returns empty vec if file missing).
pub fn load_user(path: &Path) -> anyhow::Result<Vec<ProviderEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    load_builtin(&content)
}

/// Merge user providers on top of built-in providers.
/// User providers with the same `id` replace built-in entries entirely.
pub fn merge(builtin: Vec<ProviderEntry>, user: Vec<ProviderEntry>) -> Vec<ProviderEntry> {
    let mut map: HashMap<String, ProviderEntry> = HashMap::new();
    for entry in builtin {
        map.insert(entry.id.clone(), entry);
    }
    for entry in user {
        map.insert(entry.id.clone(), entry);
    }
    map.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_builtin() {
        let json = r#"{
            "providers": {
                "test-provider": {
                    "name": "Test",
                    "baseUrl": "https://test.example/v1",
                    "api": "openai-completions",
                    "env": { "apiKey": "TEST_API_KEY" },
                    "models": [
                        {
                            "id": "test-model",
                            "name": "Test Model",
                            "reasoning": true,
                            "cost": { "input": 1.0, "output": 2.0 },
                            "contextWindow": 100000,
                            "maxTokens": 32000
                        }
                    ]
                }
            }
        }"#;
        let entries = load_builtin(json).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.id, "test-provider");
        assert_eq!(entry.models.len(), 1);
        let model = &entry.models[0];
        assert_eq!(model.id, "test-model");
        assert_eq!(model.api, ApiProtocol::OpenAiCompletions);
        assert!(model.reasoning);
        assert!(!model.headers.contains_key("_rab_compat"));
        assert_eq!(model.cost.input_per_million as u32, 1);
    }

    #[test]
    fn test_mistral_api_type() {
        let json = r#"{
            "providers": {
                "mistral": {
                    "name": "Mistral",
                    "baseUrl": "https://api.mistral.ai",
                    "api": "mistral-conversations",
                    "env": { "apiKey": "MISTRAL_API_KEY" },
                    "models": [
                        {
                            "id": "mistral-large-latest"
                        }
                    ]
                }
            }
        }"#;
        let entries = load_builtin(json).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.id, "mistral");
        assert_eq!(entry.models.len(), 1);
        assert_eq!(entry.models[0].api, ApiProtocol::OpenAiCompletions);
    }

    #[test]
    fn test_merge_user_overrides_builtin() {
        let builtin = load_builtin(r#"{"providers":{"p1":{"name":"Builtin","baseUrl":"https://builtin.example","models":[{"id":"m1","cost":{"input":1,"output":2},"contextWindow":1000,"maxTokens":500}]}}}"#).unwrap();
        let user = load_builtin(r#"{"providers":{"p1":{"name":"User","baseUrl":"https://user.example","models":[{"id":"m1","cost":{"input":3,"output":4},"contextWindow":2000,"maxTokens":1000}]}}}"#).unwrap();
        let merged = merge(builtin, user);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "User");
        assert_eq!(merged[0].models[0].cost.input_per_million as u32, 3);
    }
}
