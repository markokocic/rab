//! `rab generate-models` subcommand.
//!
//! Fetches https://models.dev/api.json, applies pi-style corrections,
//! and writes `provider/src/models.json` in the repo root.
//!
//! All-or-nothing: any error aborts before writing.

use crate::util::tls;
use serde_json::Value;

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
/// Relative path to the models catalog, checked against CWD.
const OUTPUT_PATH: &str = "provider/src/models.json";

/// Describes a target provider for model generation.
struct ProviderInfo {
    /// Key used in models.json (e.g. "openai").
    key: &'static str,
    /// Key in the models.dev API JSON (e.g. "openai").
    models_dev_key: &'static str,
    /// Human-readable display name.
    name: &'static str,
    /// Default base URL (may be overridden per-model).
    base_url: &'static str,
    /// Default API type when npm is absent / unrecognised.
    /// Empty means "detect from npm".
    default_api: &'static str,
    /// Environment variable for the API key.
    env_var: &'static str,
    /// Provider-level HTTP headers.
    headers: &'static [(&'static str, &'static str)],
}

const TARGET_PROVIDERS: &[ProviderInfo] = &[
    // ── Existing providers ───────────────────────────────────────────
    ProviderInfo {
        key: "github-copilot",
        models_dev_key: "github-copilot",
        name: "GitHub Copilot",
        base_url: "https://api.individual.githubcopilot.com",
        default_api: "",
        env_var: "COPILOT_GITHUB_TOKEN",
        headers: &[
            ("User-Agent", "GitHubCopilotChat/0.35.0"),
            ("Editor-Version", "vscode/1.107.0"),
            ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
            ("Copilot-Integration-Id", "vscode-chat"),
        ],
    },
    ProviderInfo {
        key: "opencode",
        models_dev_key: "opencode",
        name: "OpenCode Zen",
        base_url: "https://opencode.ai/zen",
        default_api: "",
        env_var: "OPENCODE_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "opencode-go",
        models_dev_key: "opencode-go",
        name: "OpenCode Zen Go",
        base_url: "https://opencode.ai/zen/go",
        default_api: "",
        env_var: "OPENCODE_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "deepseek",
        models_dev_key: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com",
        default_api: "openai-completions",
        env_var: "DEEPSEEK_API_KEY",
        headers: &[],
    },
    // ── New providers (pi-compatible) ────────────────────────────────
    ProviderInfo {
        key: "anthropic",
        models_dev_key: "anthropic",
        name: "Anthropic",
        base_url: "https://api.anthropic.com",
        default_api: "anthropic-messages",
        env_var: "ANTHROPIC_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "google",
        models_dev_key: "google",
        name: "Google Gemini",
        base_url: "https://generativelanguage.googleapis.com",
        default_api: "google-generative-ai",
        env_var: "GEMINI_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "openai",
        models_dev_key: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        default_api: "openai-responses",
        env_var: "OPENAI_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "xai",
        models_dev_key: "xai",
        name: "xAI",
        base_url: "https://api.x.ai/v1",
        default_api: "openai-completions",
        env_var: "XAI_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "groq",
        models_dev_key: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        default_api: "openai-completions",
        env_var: "GROQ_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "cerebras",
        models_dev_key: "cerebras",
        name: "Cerebras",
        base_url: "https://api.cerebras.ai/v1",
        default_api: "openai-completions",
        env_var: "CEREBRAS_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "mistral",
        models_dev_key: "mistral",
        name: "Mistral",
        base_url: "https://api.mistral.ai",
        default_api: "mistral-conversations",
        env_var: "MISTRAL_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "openrouter",
        models_dev_key: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        default_api: "openai-completions",
        env_var: "OPENROUTER_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "minimax",
        models_dev_key: "minimax",
        name: "MiniMax",
        base_url: "https://api.minimaxi.com/anthropic",
        default_api: "anthropic-messages",
        env_var: "MINIMAX_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "moonshotai",
        models_dev_key: "moonshotai",
        name: "Moonshot AI",
        base_url: "https://api.moonshot.ai/v1",
        default_api: "openai-completions",
        env_var: "MOONSHOT_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "together",
        models_dev_key: "togetherai",
        name: "Together AI",
        base_url: "https://api.together.ai/v1",
        default_api: "openai-completions",
        env_var: "TOGETHER_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "fireworks",
        models_dev_key: "fireworks-ai",
        name: "Fireworks AI",
        base_url: "https://api.fireworks.ai/inference",
        default_api: "anthropic-messages",
        env_var: "FIREWORKS_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "huggingface",
        models_dev_key: "huggingface",
        name: "Hugging Face",
        base_url: "https://router.huggingface.co/v1",
        default_api: "openai-completions",
        env_var: "HF_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "zai",
        models_dev_key: "zai-coding-plan",
        name: "Z.ai (Zhipu AI)",
        base_url: "https://api.z.ai/api/paas/v4",
        default_api: "openai-completions",
        env_var: "ZAI_API_KEY",
        headers: &[],
    },
    ProviderInfo {
        key: "nvidia",
        models_dev_key: "nvidia",
        name: "NVIDIA NIM",
        base_url: "https://integrate.api.nvidia.com/v1",
        default_api: "openai-completions",
        env_var: "NVIDIA_API_KEY",
        headers: &[],
    },
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

        for info in TARGET_PROVIDERS {
            let Some(models_map) = root
                .get(info.models_dev_key)
                .and_then(|s| s.get("models"))
                .and_then(|m| m.as_object())
            else {
                eprintln!(
                    "Warning: No models for '{}' (key '{}') in models.dev. Skipping.",
                    info.key, info.models_dev_key
                );
                continue;
            };

            let models: Vec<Value> = models_map
                .iter()
                .filter(|(_, v)| {
                    v.get("tool_call").and_then(|x| x.as_bool()) == Some(true)
                        && v.get("status").and_then(|x| x.as_str()) != Some("deprecated")
                })
                .map(|(model_id, model_val)| build_model_entry(info, model_id, model_val))
                .collect::<Result<Vec<_>, _>>()?;

            let mut provider_entry = serde_json::json!({
                "name": info.name,
                "baseUrl": info.base_url,
                "api": info.default_api,
                "env": { "apiKey": info.env_var },
                "models": models
            });
            if !info.headers.is_empty() {
                let headers_obj: serde_json::Map<String, Value> = info
                    .headers
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
                    .collect();
                provider_entry["headers"] = Value::Object(headers_obj);
            }

            providers_obj.insert(info.key.to_string(), provider_entry);
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

fn build_model_entry(
    info: &ProviderInfo,
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

    let (api, base_url) = resolve_api_and_base_url(info, model_id, npm, obj);
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

    apply_corrections(info, model_id, &mut entry, api, reasoning, obj, npm);

    Ok(entry)
}

/// Determine the API identifier and optional base URL override for a model.
fn resolve_api_and_base_url<'a>(
    info: &ProviderInfo,
    model_id: &str,
    npm: Option<&str>,
    _obj: &'a serde_json::Map<String, Value>,
) -> (&'a str, Option<String>) {
    let base_path = info.base_url;

    // If the provider has a default_api set, use it and derive base URL.
    if !info.default_api.is_empty() && matches_npm_to_default(info.default_api, npm) {
        let base = match info.default_api {
            "anthropic-messages" | "mistral-conversations" => base_path.to_string(),
            "openai-completions" | "openai-responses" | "google-generative-ai" => {
                if base_path.ends_with("/v1") {
                    base_path.to_string()
                } else {
                    format!("{}/v1", base_path)
                }
            }
            _ => format!("{}/v1", base_path),
        };
        return (info.default_api, Some(base));
    }

    // Proxy providers (empty default_api): determine API from npm SDK hint,
    // then apply model-specific overrides that take precedence.
    if info.default_api.is_empty() {
        // Default for proxy providers: openai-completions with /v1
        let mut api = "openai-completions";
        let mut base_url = format!("{}/v1", base_path);

        // Apply npm SDK hints
        match npm {
            Some("@ai-sdk/openai") => {
                api = "openai-responses";
                base_url = format!("{}/v1", base_path);
            }
            Some("@ai-sdk/anthropic") => {
                api = "anthropic-messages";
                base_url = base_path.to_string();
            }
            Some("@ai-sdk/google") => {
                api = "google-generative-ai";
                base_url = format!("{}/v1", base_path);
            }
            _ => {}
        }

        // Model-specific overrides (take precedence over npm hints)
        if info.key == "opencode-go" {
            // models.dev reports these as @ai-sdk/anthropic but the opencode-go
            // endpoints only support openai-completions at /v1/chat/completions
            if model_id == "minimax-m2.7"
                || model_id == "qwen3.5-plus"
                || model_id == "qwen3.6-plus"
            {
                api = "openai-completions";
                base_url = format!("{}/v1", base_path);
            }
        }

        return (api, Some(base_url));
    }

    // Non-proxy providers with matching default_api are handled above.
    // Remaining providers (copilot, openai, xai, etc.) have explicit default_api
    // that didn't match npm, so apply provider-specific overrides.

    // Provider-specific overrides
    match info.key {
        "github-copilot" => resolve_github_copilot_api(model_id),
        "opencode-go" => resolve_opencode_go_api(model_id),
        "openai" => ("openai-responses", Some(format!("{}/v1", base_path))),
        "xai" => {
            if model_id == "grok-4.5" {
                ("openai-responses", Some(format!("{}/v1", base_path)))
            } else {
                ("openai-completions", Some(format!("{}/v1", base_path)))
            }
        }
        _ => {
            // Fallback: use default_api with /v1 suffix.
            let base = if base_path.ends_with("/v1") {
                base_path.to_string()
            } else {
                format!("{}/v1", base_path)
            };
            (info.default_api, Some(base))
        }
    }
}

/// Returns true if a given npm SDK hint matches the provider's default API.
fn matches_npm_to_default(default_api: &str, npm: Option<&str>) -> bool {
    match (default_api, npm) {
        // Anthropic Messages: @ai-sdk/anthropic or no npm hint
        ("anthropic-messages", None) => true,
        ("anthropic-messages", Some("@ai-sdk/anthropic")) => true,
        // OpenAI Completions: @ai-sdk/openai-compatible, no hint, or non-matching hint
        ("openai-completions", None) => true,
        ("openai-completions", Some("@ai-sdk/openai-compatible")) => true,
        // OpenAI Responses: @ai-sdk/openai
        ("openai-responses", Some("@ai-sdk/openai")) => true,
        // Google Generative AI: @ai-sdk/google
        ("google-generative-ai", Some("@ai-sdk/google")) => true,
        // Mistral Conversations: no npm hint
        ("mistral-conversations", None) => true,
        _ => false,
    }
}

fn resolve_github_copilot_api(model_id: &str) -> (&'static str, Option<String>) {
    let base = "https://api.individual.githubcopilot.com";
    // Claude models (excluding fable) use Anthropic Messages API.
    // claude-fable-* uses openai-completions.
    if model_id.starts_with("claude-") && !model_id.contains("fable") {
        return ("anthropic-messages", Some(base.into()));
    }
    // gpt-5, oswe, mai- models use openai-responses
    if model_id.starts_with("gpt-5") || model_id.starts_with("oswe") || model_id.starts_with("mai-")
    {
        return ("openai-responses", Some(base.into()));
    }
    ("openai-completions", Some(base.into()))
}

fn resolve_opencode_go_api(_model_id: &str) -> (&'static str, Option<String>) {
    let base = "https://opencode.ai/zen/go";
    // Default for opencode-go: openai-completions at /v1/chat/completions.
    // Only models with @ai-sdk/anthropic npm hint use anthropic-messages at root.
    ("openai-completions", Some(format!("{}/v1", base)))
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

/// Detect default OpenAI-completions compat from the provider key and base URL.
fn detect_openai_compat(info: &ProviderInfo) -> serde_json::Value {
    let base_url = info.base_url;
    let is_deepseek = info.key == "deepseek" || base_url.contains("deepseek.com");
    let is_xai = info.key == "xai" || base_url.contains("api.x.ai");
    let is_zai =
        info.key == "zai" || base_url.contains("api.z.ai") || base_url.contains("open.bigmodel.cn");
    let is_moonshot = info.key == "moonshotai" || base_url.contains("api.moonshot.");
    let is_together = info.key == "together" || base_url.contains("api.together.");
    let is_nvidia = info.key == "nvidia" || base_url.contains("integrate.api.nvidia.com");

    let use_max_tokens = is_deepseek || is_moonshot || is_together || is_nvidia || is_zai;
    let has_thinking_control = is_deepseek;
    let thinking_format = if is_deepseek {
        "deepseek"
    } else if is_zai {
        "zai"
    } else if is_together {
        "together"
    } else {
        "openai"
    };

    serde_json::json!({
        "supportsStore": false,
        "supportsDeveloperRole": false,
        "supportsReasoningEffort": !is_xai && !is_zai && !is_moonshot && !is_together && !is_nvidia,
        "supportsUsageInStreaming": true,
        "supportsThinkingControl": has_thinking_control,
        "maxTokensField": if use_max_tokens { "max_tokens" } else { "max_completion_tokens" },
        "requiresToolResultName": false,
        "requiresAssistantAfterToolResult": false,
        "requiresReasoningContentOnAssistantMessages": is_deepseek,
        "thinkingFormat": thinking_format,
    })
}

/// Apply pi-style corrections to a model entry.
fn apply_corrections(
    info: &ProviderInfo,
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
            (info.key, model_id),
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
        if info.key == "github-copilot" {
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

        // Anthropic adaptive thinking thinkingLevelMap
        if model_id.contains("opus-4-6")
            || model_id.contains("opus-4.6")
            || model_id.contains("sonnet-4-6")
            || model_id.contains("sonnet-4.6")
        {
            entry["thinkingLevelMap"] = serde_json::json!({ "max": "max" });
        }
        if model_id.contains("opus-4-7")
            || model_id.contains("opus-4.7")
            || model_id.contains("opus-4-8")
            || model_id.contains("opus-4.8")
            || model_id.contains("sonnet-5")
            || model_id.contains("sonnet.5")
        {
            entry["thinkingLevelMap"] = serde_json::json!({ "xhigh": "xhigh", "max": "max" });
        }
        if model_id.contains("fable-5") {
            entry["thinkingLevelMap"] =
                serde_json::json!({ "off": null, "xhigh": "xhigh", "max": "max" });
        }

        return;
    }

    // ── OpenAI Responses corrections ──────────────────────────────────────
    if api == "openai-responses" || api == "openai-codex-responses" {
        // GPT-5.x thinking level maps (pi-compatible)
        if model_id.starts_with("gpt-5") {
            let mut tlm = serde_json::Map::new();
            // Default off → "none" for models that don't support reasoning off
            if model_id.contains("gpt-5.4")
                || model_id.contains("gpt-5.5")
                || model_id.ends_with("gpt-5.5-pro")
            {
                tlm.insert("off".into(), Value::String("none".into()));
            } else {
                tlm.insert("off".into(), Value::Null);
            }
            // xhigh for gpt-5.2+
            if model_id.contains("gpt-5.2")
                || model_id.contains("gpt-5.3")
                || model_id.contains("gpt-5.4")
                || model_id.contains("gpt-5.5")
                || model_id.contains("gpt-5.6")
                || model_id.ends_with("gpt-5.5-pro")
            {
                tlm.insert("xhigh".into(), Value::String("xhigh".into()));
            }
            // max for gpt-5.6
            if model_id.contains("gpt-5.6") {
                tlm.insert("max".into(), Value::String("max".into()));
            }
            // minimal → null for gpt-5.5
            if model_id.ends_with("gpt-5.5") || model_id.ends_with("gpt-5.5-pro") {
                tlm.insert("minimal".into(), Value::Null);
            }
            if !tlm.is_empty() {
                entry["thinkingLevelMap"] = Value::Object(tlm);
            }
        }

        // GitHub Copilot: minimal → "low" for gpt-5.x
        if info.key == "github-copilot" && model_id.starts_with("gpt-5") {
            let existing = entry
                .get("thinkingLevelMap")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));
            let mut map = existing.as_object().cloned().unwrap_or_default();
            map.insert("minimal".into(), Value::String("low".into()));
            entry["thinkingLevelMap"] = Value::Object(map);
        }

        return;
    }

    // ── OpenAI Completions corrections ───────────────────────────────────
    if api != "openai-completions" {
        return;
    }

    // Auto-detect compat from provider metadata (keep as Value for bracket-mut access)
    let mut compat = detect_openai_compat(info);

    if model_id.contains("deepseek-v4") {
        compat["requiresReasoningContentOnAssistantMessages"] = Value::Bool(true);
        if info.key == "opencode" {
            compat["thinkingFormat"] = Value::String("openai".into());
            compat["supportsLongCacheRetention"] = Value::Bool(false);
        } else {
            compat["thinkingFormat"] = Value::String("deepseek".into());
        }
        entry["thinkingLevelMap"] = serde_json::json!({
            "minimal": null, "low": null, "medium": null, "high": "high", "max": "max"
        });
    }

    if model_id == "kimi-k2.6" {
        compat["thinkingFormat"] = Value::String("deepseek".into());
        compat["supportsReasoningEffort"] = Value::Bool(false);
        compat["supportsLongCacheRetention"] = Value::Bool(false);
        if info.key == "opencode-go" {
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

    if info.key == "opencode-go" && model_id.starts_with("qwen3") {
        compat["thinkingFormat"] = Value::String("qwen".into());
    }

    // GitHub Copilot: openai-completions models
    if info.key == "github-copilot" {
        compat["supportsReasoningEffort"] = Value::Bool(false);
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

    // Grok 4.5 uses openai-responses, but grok-3/grok-4 use openai-completions
    if info.key == "xai"
        && (model_id.contains("grok-3")
            || (model_id.contains("grok-4") && !model_id.contains("grok-4.5")))
    {
        compat["supportsStore"] = Value::Bool(false);
        compat["supportsDeveloperRole"] = Value::Bool(false);
    }

    // DeepSeek V4 thinkingLevelMap for openai-completions
    if info.key == "deepseek" && model_id.contains("deepseek-v4") {
        entry["thinkingLevelMap"] = serde_json::json!({
            "minimal": null, "low": null, "medium": null, "high": "high", "max": "max"
        });
    }

    entry["compat"] = compat;
}

async fn fetch(url: &str) -> anyhow::Result<String> {
    let response = tls::reqwest_client()
        .get(url)
        .send()
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
