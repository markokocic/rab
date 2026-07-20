//! `rab generate-models` subcommand.
//!
//! Fetches https://models.dev/api.json, applies pi-style corrections,
//! and writes `provider/models.json` in the repo root.
//!
//! All-or-nothing: any error aborts before writing.

use serde_json::Value;

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
/// Relative path to the models catalog, checked against CWD.
const OUTPUT_PATH: &str = "src/provider/models.json";

/// Providers we care about and their model-dev key.
const TARGET_PROVIDERS: &[(&str, &str)] = &[
    ("github-copilot", "github-copilot"),
    ("opencode", "opencode"),
    ("opencode-go", "opencode-go"),
    ("deepseek", "deepseek"),
];

/// Run the generate. Called from main.rs when args contain "generate-models".
pub async fn run_generate_models() -> anyhow::Result<()> {
    // 1. Fetch models.dev
    eprintln!("Fetching {} ...", MODELS_DEV_URL);
    let raw = fetch(MODELS_DEV_URL).await?;
    let root: Value = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("Failed to parse models.dev response: {}", e))?;

    // 2. Resolve output path and verify it exists (safety guard)
    let output_path = std::env::current_dir()?.join(OUTPUT_PATH);
    if !output_path.exists() {
        anyhow::bail!(
            "{} not found.\nRun this from the rab repo root, or specify a project that has the built-in catalog.\n  cargo run -- generate-models",
            output_path.display()
        );
    }

    // 3. Read existing file (preserve user edits to other providers)
    let mut output: Value = if output_path.exists() {
        let content = std::fs::read_to_string(&output_path)?;
        serde_json::from_str(&content).unwrap_or(Value::Object(serde_json::Map::new()))
    } else {
        Value::Object(serde_json::Map::new())
    };

    if !output.is_object() {
        output = Value::Object(serde_json::Map::new());
    }

    // 4. Process each target provider — all processing inside a block
    //    so the mutable borrow on `output` drops before the write below.
    let total: usize = {
        let obj = output
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("output is not an object"))?;

        if !obj.contains_key("providers") {
            obj.insert("providers".into(), Value::Object(serde_json::Map::new()));
        }

        let providers_obj = obj["providers"]
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("providers is not an object"))?;

        for &(provider_key, models_dev_key) in TARGET_PROVIDERS {
            let models_map = root
                .get(models_dev_key)
                .and_then(|s| s.get("models"))
                .and_then(|m| m.as_object())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No models for '{}' in models.dev. Aborting.",
                        models_dev_key
                    )
                })?;

            let models: Vec<Value> = models_map
                .iter()
                .filter(|(_, v)| {
                    v.get("tool_call").and_then(|x| x.as_bool()) == Some(true)
                        && v.get("status").and_then(|x| x.as_str()) != Some("deprecated")
                })
                .map(|(model_id, model_val)| build_model_entry(provider_key, model_id, model_val))
                .collect::<Result<Vec<_>, _>>()?;

            let headers = provider_headers(provider_key);
            let mut provider_entry = serde_json::json!({
                "name": provider_display_name(provider_key),
                "baseUrl": provider_base_url(provider_key),
                "api": provider_base_api(provider_key),
                "env": { "apiKey": provider_env_var(provider_key) },
                "models": models
            });
            if !headers.is_empty() {
                let headers_obj: serde_json::Map<String, Value> = headers
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
                    .collect();
                provider_entry["headers"] = Value::Object(headers_obj);
            }

            providers_obj.insert(provider_key.to_string(), provider_entry);
        }

        // Count total models
        providers_obj
            .values()
            .filter_map(|p| p.get("models").and_then(|m| m.as_array()))
            .map(|m| m.len())
            .sum()
    }; // mutable borrow on `output` ends here

    // 5. Write back (only reached if all processing succeeded)
    let json = serde_json::to_string_pretty(&output)?;
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, &json)?;

    eprintln!(
        "Wrote {} models across {} providers to {}",
        total,
        TARGET_PROVIDERS.len(),
        output_path.display()
    );
    Ok(())
}

fn provider_display_name(key: &str) -> &'static str {
    match key {
        "github-copilot" => "GitHub Copilot",
        "opencode-go" => "OpenCode Zen Go",
        "deepseek" => "DeepSeek",
        _ => "OpenCode Zen",
    }
}

fn provider_base_url(key: &str) -> &'static str {
    match key {
        "github-copilot" => "https://api.individual.githubcopilot.com",
        "opencode-go" => "https://opencode.ai/zen/go",
        "deepseek" => "https://api.deepseek.com",
        _ => "https://opencode.ai/zen",
    }
}

fn provider_env_var(key: &str) -> &'static str {
    match key {
        "github-copilot" => "COPILOT_GITHUB_TOKEN",
        "deepseek" => "DEEPSEEK_API_KEY",
        _ => "OPENCODE_API_KEY",
    }
}

fn provider_base_api(key: &str) -> &'static str {
    let _ = key;
    "openai-completions"
}

/// Provider-level HTTP headers (e.g. for GitHub Copilot).
fn provider_headers(key: &str) -> Vec<(&'static str, &'static str)> {
    match key {
        "github-copilot" => vec![
            ("User-Agent", "GitHubCopilotChat/0.35.0"),
            ("Editor-Version", "vscode/1.107.0"),
            ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
            ("Copilot-Integration-Id", "vscode-chat"),
        ],
        _ => vec![],
    }
}

fn build_model_entry(
    provider_key: &str,
    model_id: &str,
    model_val: &Value,
) -> anyhow::Result<Value> {
    let obj = model_val
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Model '{}' is not an object", model_id))?;

    let npm = obj
        .get("provider")
        .and_then(|p| p.get("npm"))
        .and_then(|v| v.as_str());

    let (api, base_url) = resolve_api_and_base_url(provider_key, model_id, npm, obj);
    let reasoning = obj
        .get("reasoning")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut input: Vec<Value> = vec!["text".into()];
    if let Some(mods) = obj
        .get("modalities")
        .and_then(|m| m.get("input"))
        .and_then(|m| m.as_array())
        && mods.iter().any(|m| m.as_str() == Some("image"))
    {
        input.push("image".into());
    }

    let input_cost = obj
        .get("cost")
        .and_then(|c| c.get("input"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let output_cost = obj
        .get("cost")
        .and_then(|c| c.get("output"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let cache_read = obj
        .get("cost")
        .and_then(|c| c.get("cache_read"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let cache_write = obj
        .get("cost")
        .and_then(|c| c.get("cache_write"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let context_window = obj
        .get("limit")
        .and_then(|l| l.get("context"))
        .and_then(|v| v.as_u64())
        .unwrap_or(4096);
    let max_tokens = obj
        .get("limit")
        .and_then(|l| l.get("output"))
        .and_then(|v| v.as_u64())
        .unwrap_or(4096);

    let mut entry = serde_json::json!({
        "id": model_id,
        "name": obj.get("name").and_then(|v| v.as_str()).unwrap_or(model_id),
        "api": api,
        "reasoning": reasoning,
        "input": input,
        "cost": {
            "input": input_cost,
            "output": output_cost,
            "cacheRead": cache_read,
            "cacheWrite": cache_write
        },
        "contextWindow": context_window,
        "maxTokens": max_tokens
    });

    if let Some(bu) = base_url {
        entry["baseUrl"] = Value::String(bu);
    }

    apply_corrections(provider_key, model_id, &mut entry, api, reasoning, obj, npm);

    Ok(entry)
}

/// Determine the API identifier and optional base URL override for a model.
fn resolve_api_and_base_url<'a>(
    provider_key: &str,
    model_id: &str,
    npm: Option<&str>,
    _obj: &'a serde_json::Map<String, Value>,
) -> (&'a str, Option<String>) {
    let base_path = provider_base_url(provider_key);

    match npm {
        Some("@ai-sdk/openai") => ("openai-responses", Some(format!("{}/v1", base_path))),
        Some("@ai-sdk/anthropic") => ("anthropic-messages", Some(base_path.into())),
        Some("@ai-sdk/google") => ("google-generative-ai", Some(format!("{}/v1", base_path))),
        _ => {
            // GitHub Copilot: Claude Anthropic models (haiku, sonnet, opus) use the
            // Anthropic Messages API. claude-fable-* uses openai-completions.
            // models.dev does not set npm for Copilot models, so detect by ID.
            if provider_key == "github-copilot"
                && model_id.starts_with("claude-")
                && !model_id.contains("fable")
            {
                return ("anthropic-messages", Some(base_path.into()));
            }
            // All other GitHub Copilot models use openai-completions at the root (no /v1).
            if provider_key == "github-copilot" {
                return ("openai-completions", Some(base_path.into()));
            }
            if provider_key == "opencode-go" && model_id == "minimax-m2.7" {
                return ("openai-completions", Some(format!("{}/v1", base_path)));
            }
            if provider_key == "opencode-go"
                && (model_id == "qwen3.5-plus" || model_id == "qwen3.6-plus")
            {
                return ("openai-completions", Some(format!("{}/v1", base_path)));
            }
            ("openai-completions", Some(format!("{}/v1", base_path)))
        }
    }
}

fn is_anthropic_adaptive_thinking_model(model_id: &str) -> bool {
    model_id.contains("opus-4-6")
        || model_id.contains("opus-4.6")
        || model_id.contains("opus-4-7")
        || model_id.contains("opus-4.7")
        || model_id.contains("opus-4-8")
        || model_id.contains("opus-4.8")
        || model_id.contains("sonnet-4-6")
        || model_id.contains("sonnet-4.6")
        || model_id.contains("sonnet-5")
        || model_id.contains("sonnet.5")
        || model_id.contains("fable-5")
}

fn is_anthropic_temperature_unsupported(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    id.contains("opus-4-7")
        || id.contains("opus-4.7")
        || id.contains("opus-4-8")
        || id.contains("opus-4.8")
}

/// Apply pi-style corrections to a model entry.
fn apply_corrections(
    provider_key: &str,
    model_id: &str,
    entry: &mut Value,
    api: &str,
    _reasoning: bool,
    _obj: &serde_json::Map<String, Value>,
    _npm: Option<&str>,
) {
    // ── Anthropic Messages corrections ────────────────────────────────────
    if api == "anthropic-messages" {
        let mut compat = serde_json::Map::new();

        // forceAdaptiveThinking for extended-thinking models
        if is_anthropic_adaptive_thinking_model(model_id) {
            compat.insert("forceAdaptiveThinking".into(), Value::Bool(true));
        }

        // supportsTemperature: false for opus-4.7 / opus-4.8
        if is_anthropic_temperature_unsupported(model_id) {
            compat.insert("supportsTemperature".into(), Value::Bool(false));
        }

        // supportsEagerToolInputStreaming: false for specific GitHub Copilot models
        let copilot_eager_unsupported = matches!(
            (provider_key, model_id),
            ("github-copilot", "claude-haiku-4.5")
                | ("github-copilot", "claude-sonnet-4")
                | ("github-copilot", "claude-sonnet-4.5")
        );
        if copilot_eager_unsupported {
            compat.insert("supportsEagerToolInputStreaming".into(), Value::Bool(false));
        }

        if !compat.is_empty() {
            entry["compat"] = Value::Object(compat);
        }

        // thinkingLevelMap overrides for GitHub Copilot Claude models
        if provider_key == "github-copilot" {
            let override_map: Option<Value> = match model_id {
                "claude-opus-4.6" | "claude-opus-4-6" => Some(serde_json::json!({ "max": "max" })),
                "claude-opus-4.7" | "claude-opus-4-7" => Some(serde_json::json!({
                    "xhigh": "xhigh", "max": "max", "minimal": "low"
                })),
                "claude-opus-4.8" | "claude-opus-4-8" => Some(serde_json::json!({
                    "xhigh": "xhigh", "max": "max", "minimal": "low"
                })),
                "claude-sonnet-4.6" | "claude-sonnet-4-6" => {
                    Some(serde_json::json!({ "minimal": "low", "max": "max" }))
                }
                _ => None,
            };
            if let Some(map) = override_map {
                entry["thinkingLevelMap"] = map;
            }
        }

        return;
    }

    // ── OpenAI Completions corrections ───────────────────────────────────
    if api != "openai-completions" {
        return;
    }

    let mut compat = serde_json::json!({
        "supportsStore": false,
        "supportsDeveloperRole": false,
        "maxTokensField": "max_tokens"
    });

    if model_id.contains("deepseek-v4") {
        compat["requiresReasoningContentOnAssistantMessages"] = Value::Bool(true);
        if provider_key == "opencode" {
            // opencode preserves native reasoning_effort
            compat["thinkingFormat"] = Value::String("openai".into());
            compat["supportsLongCacheRetention"] = Value::Bool(false);
        } else {
            compat["thinkingFormat"] = Value::String("deepseek".into());
        }
        // supportsReasoningEffort stays at default (true) to match pi
        entry["thinkingLevelMap"] = serde_json::json!({
            "minimal": null, "low": null, "medium": null, "high": "high", "max": "max"
        });
    }

    if model_id == "kimi-k2.6" {
        compat["thinkingFormat"] = Value::String("deepseek".into());
        compat["supportsReasoningEffort"] = Value::Bool(false);
        compat["supportsLongCacheRetention"] = Value::Bool(false);
        // Only opencode-go kimi-k2.6 gets a thinkingLevelMap (pi behavior)
        if provider_key == "opencode-go" {
            entry["thinkingLevelMap"] = serde_json::json!({
                "minimal": null, "low": null, "medium": null
            });
        }
    }

    if model_id == "kimi-k2.5" {
        compat["supportsLongCacheRetention"] = Value::Bool(false);
    }

    if model_id == "minimax-m2.7" {
        compat["supportsLongCacheRetention"] = Value::Bool(false);
    }

    if model_id == "deepseek-reasoner" {
        compat["requiresReasoningContentOnAssistantMessages"] = Value::Bool(true);
        compat["thinkingFormat"] = Value::String("deepseek".into());
        // supportsReasoningEffort stays at default (true) to match pi
        entry["thinkingLevelMap"] = serde_json::json!({
            "minimal": null, "low": null, "medium": null, "high": "high", "max": "max"
        });
    }

    if model_id == "grok-build-0.1" {
        compat["supportsReasoningEffort"] = Value::Bool(false);
        entry["thinkingLevelMap"] = serde_json::json!({
            "off": null, "minimal": null, "low": null, "medium": null
        });
    }

    if provider_key == "opencode-go" && model_id.starts_with("qwen3") {
        compat["thinkingFormat"] = Value::String("qwen".into());
    }

    // GitHub Copilot: openai-completions models
    if provider_key == "github-copilot" {
        compat["supportsReasoningEffort"] = Value::Bool(false);
        // gpt-5.x models need minimal: "low" in thinkingLevelMap
        if model_id.starts_with("gpt-5") {
            entry["thinkingLevelMap"] = match entry.get("thinkingLevelMap").cloned() {
                Some(Value::Object(mut map)) => {
                    map.insert("minimal".into(), Value::String("low".into()));
                    Value::Object(map)
                }
                _ => serde_json::json!({ "minimal": "low" }),
            };
        }
    }

    entry["compat"] = compat;
}

async fn fetch(url: &str) -> anyhow::Result<String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| anyhow::anyhow!("Network error fetching {}: {}", url, e))?;

    if !response.status().is_success() {
        anyhow::bail!("HTTP {} fetching {}", response.status(), url);
    }

    response
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))
}
