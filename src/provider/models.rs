//! models.json — parses built-in and user models.json files
//! and constructs yoagent ModelConfigs with rich compat stored in headers.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use yoagent::provider::model::{
    ApiProtocol, CostConfig, MaxTokensField, ModelConfig, OpenAiCompat, ThinkingFormat,
};

use super::compat::{RabMaxTokensField, RabOpenAiCompat, RabThinkingFormat};

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
    #[allow(dead_code)]
    thinking_level_map: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    input: Option<Vec<String>>,
    #[serde(default)]
    cost: Option<CostDef>,
    #[serde(default)]
    context_window: Option<u32>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    compat: Option<RabOpenAiCompat>,
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
            _ => anyhow::bail!("Unknown API type: {}", api_str),
        };

        let base_url = m
            .base_url
            .clone()
            .or_else(|| def.base_url.clone())
            .unwrap_or_default();

        let input = m.input.clone().unwrap_or_else(|| vec!["text".to_string()]);
        let _has_image = input.iter().any(|s| s == "image");

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

        // Build the compat and store it as JSON in headers["_rab_compat"]
        let compat = m.compat.clone().unwrap_or_default();
        let compat_json = serde_json::to_string(&compat).unwrap_or_else(|_| "{}".to_string());

        // Also build yoagent's OpenAiCompat for models that use openai-completions
        let yoagent_compat = if api == ApiProtocol::OpenAiCompletions {
            Some(convert_to_yoagent_compat(&compat))
        } else {
            None
        };

        let mut headers = HashMap::new();
        headers.insert("_rab_compat".to_string(), compat_json);
        if let Some(tlm) = &m.thinking_level_map
            && let Ok(json) = serde_json::to_string(tlm)
        {
            headers.insert("_rab_thinking_map".to_string(), json);
        }

        let model = ModelConfig {
            id: m.id.clone(),
            name: m.name.clone().unwrap_or_else(|| m.id.clone()),
            api,
            provider: id.to_string(),
            base_url,
            reasoning: m.reasoning,
            context_window,
            max_tokens,
            cost,
            headers,
            compat: yoagent_compat,
        };

        models.push(model);
    }

    let env_var = def.env.as_ref().and_then(|e| e.get("apiKey")).cloned();

    Ok(ProviderEntry {
        id: id.to_string(),
        name: def.name.unwrap_or_else(|| id.to_string()),
        models,
        env_var_hint: env_var,
    })
}

/// Convert our rich compat to yoagent's OpenAiCompat for the fields they share.
fn convert_to_yoagent_compat(rab: &RabOpenAiCompat) -> OpenAiCompat {
    let max_tokens_field = match rab.max_tokens_field {
        RabMaxTokensField::MaxTokens => MaxTokensField::MaxTokens,
        RabMaxTokensField::MaxCompletionTokens => MaxTokensField::MaxCompletionTokens,
    };

    let thinking_format = match rab.thinking_format {
        RabThinkingFormat::OpenAi
        | RabThinkingFormat::OpenRouter
        | RabThinkingFormat::DeepSeek
        | RabThinkingFormat::Together
        | RabThinkingFormat::Zai
        | RabThinkingFormat::ChatTemplate
        | RabThinkingFormat::QwenChatTemplate
        | RabThinkingFormat::StringThinking
        | RabThinkingFormat::AntLing => ThinkingFormat::OpenAi,
        RabThinkingFormat::Qwen => ThinkingFormat::Qwen,
    };

    OpenAiCompat {
        supports_store: rab.supports_store,
        supports_developer_role: rab.supports_developer_role,
        supports_reasoning_effort: rab.supports_reasoning_effort,
        supports_thinking_control: rab.supports_thinking_control
            || rab.thinking_format == RabThinkingFormat::DeepSeek,
        supports_usage_in_streaming: rab.supports_usage_in_streaming,
        max_tokens_field,
        requires_tool_result_name: rab.requires_tool_result_name,
        requires_assistant_after_tool_result: rab.requires_assistant_after_tool_result,
        thinking_format,
    }
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
        assert!(model.headers.contains_key("_rab_compat"));
        assert_eq!(model.cost.input_per_million as u32, 1);
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
