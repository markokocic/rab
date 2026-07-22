//! GitHub Copilot OAuth provider — matching pi's github-copilot.ts exactly.
//!
//! Uses the device code flow (RFC 8628) to authenticate with GitHub.
//! After login, fetches available models and enables them.

use std::collections::HashMap;

use async_trait::async_trait;
use base64::Engine;
use rab_util::tls;

use super::device_code::{PollOptions, PollStatus, poll_device_code_flow};
use super::{DeviceCodeInfo, OAuthCredentials, OAuthLoginCallbacks, OAuthPrompt, OAuthProvider};

const CLIENT_ID_ENCODED: &str = "SXYxLmI1MDdhMDhjODdlY2ZlOTg=";

const COPILOT_HEADERS: &[(&str, &str)] = &[
    ("User-Agent", "GitHubCopilotChat/0.35.0"),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];
const COPILOT_API_VERSION: &str = "2026-06-01";

fn client_id() -> String {
    String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(CLIENT_ID_ENCODED)
            .expect("valid base64"),
    )
    .expect("valid utf8")
}

pub fn normalize_domain(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let url_str = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{}", trimmed)
    };
    url::Url::parse(&url_str)
        .ok()
        .map(|u| u.host_str().unwrap_or("").to_string())
}

fn get_urls(domain: &str) -> (String, String, String) {
    (
        format!("https://{}/login/device/code", domain),
        format!("https://{}/login/oauth/access_token", domain),
        format!("https://api.{}/copilot_internal/v2/token", domain),
    )
}

/// Parse the proxy-ep from a Copilot token and convert to API base URL.
fn get_base_url_from_token(token: &str) -> Option<String> {
    for part in token.split(';') {
        if let Some(host) = part.strip_prefix("proxy-ep=") {
            let api_host = host.replacen("proxy.", "api.", 1);
            return Some(format!("https://{}", api_host));
        }
    }
    None
}

/// Get the GitHub Copilot API base URL.
pub fn get_copilot_base_url(token: Option<&str>, enterprise_domain: Option<&str>) -> String {
    if let Some(t) = token
        && let Some(url) = get_base_url_from_token(t)
    {
        return url;
    }
    if let Some(domain) = enterprise_domain {
        return format!("https://copilot-api.{}", domain);
    }
    "https://api.individual.githubcopilot.com".to_string()
}

/// Fetch JSON from a URL with headers.
async fn fetch_json(url: &str, headers: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    let client = tls::reqwest_client();
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.map_err(|e| format!("HTTP error: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, text));
    }
    resp.json().await.map_err(|e| format!("JSON error: {}", e))
}

/// Post form-encoded body to a URL.
async fn post_form(
    url: &str,
    headers: &[(&str, &str)],
    form: &[(&str, &str)],
) -> Result<serde_json::Value, String> {
    let client = tls::reqwest_client();
    let mut req = client.post(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let params: Vec<(&str, &str)> = form.to_vec();
    let resp = req
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("HTTP error: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, text));
    }
    resp.json().await.map_err(|e| format!("JSON error: {}", e))
}

/// Start the device code flow.
async fn start_device_flow(domain: &str) -> Result<serde_json::Value, String> {
    let (device_code_url, _, _) = get_urls(domain);
    post_form(
        &device_code_url,
        &[
            ("Accept", "application/json"),
            ("User-Agent", "GitHubCopilotChat/0.35.0"),
        ],
        &[("client_id", &client_id()), ("scope", "read:user")],
    )
    .await
}

/// Poll for the GitHub access token.
async fn poll_for_github_access_token(
    domain: &str,
    device_code: &str,
    interval: Option<u32>,
    expires_in: Option<u32>,
    cancel: Option<tokio_util::sync::CancellationToken>,
) -> Result<String, String> {
    let (_, access_token_url, _) = get_urls(domain);
    let client_id = client_id();
    let device_code = device_code.to_string();

    poll_device_code_flow(PollOptions {
        interval_seconds: interval,
        expires_in_seconds: expires_in,
        cancel,
        poll: Box::new(move || {
            let access_token_url = access_token_url.clone();
            let client_id = client_id.clone();
            let device_code = device_code.clone();
            Box::pin(async move {
                let raw = post_form(
                    &access_token_url,
                    &[
                        ("Accept", "application/json"),
                        ("User-Agent", "GitHubCopilotChat/0.35.0"),
                    ],
                    &[
                        ("client_id", &client_id),
                        ("device_code", &device_code),
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ],
                )
                .await?;

                if let Some(token) = raw.get("access_token").and_then(|t| t.as_str()) {
                    return Ok(PollStatus::Complete(token.to_string()));
                }

                if let Some(error) = raw.get("error").and_then(|e| e.as_str()) {
                    match error {
                        "authorization_pending" => return Ok(PollStatus::Pending),
                        "slow_down" => return Ok(PollStatus::SlowDown),
                        _ => {
                            let desc = raw
                                .get("error_description")
                                .and_then(|d| d.as_str())
                                .unwrap_or("");
                            return Ok(PollStatus::Failed(format!(
                                "Device flow failed: {}{}",
                                error,
                                if desc.is_empty() {
                                    String::new()
                                } else {
                                    format!(": {}", desc)
                                }
                            )));
                        }
                    }
                }

                Ok(PollStatus::Failed(
                    "Invalid device token response".to_string(),
                ))
            })
        }),
    })
    .await
}

/// Exchange GitHub access token for a Copilot token.
async fn exchange_for_copilot_token(
    github_token: &str,
    enterprise_domain: Option<&str>,
) -> Result<serde_json::Value, String> {
    let domain = enterprise_domain.unwrap_or("github.com");
    let (_, _, copilot_token_url) = get_urls(domain);

    let auth_val = format!("Bearer {}", github_token);
    let mut headers: Vec<(&str, &str)> =
        vec![("Accept", "application/json"), ("Authorization", &auth_val)];
    for (k, v) in COPILOT_HEADERS {
        headers.push((k, v));
    }

    fetch_json(&copilot_token_url, &headers).await
}

/// Refresh the Copilot token using the refresh token.
async fn refresh_copilot_access_token(
    refresh_token: &str,
    enterprise_domain: Option<&str>,
) -> Result<serde_json::Value, String> {
    let domain = enterprise_domain.unwrap_or("github.com");
    let (_, _, copilot_token_url) = get_urls(domain);

    let auth_val = format!("Bearer {}", refresh_token);
    let mut headers: Vec<(&str, &str)> =
        vec![("Accept", "application/json"), ("Authorization", &auth_val)];
    for (k, v) in COPILOT_HEADERS {
        headers.push((k, v));
    }

    fetch_json(&copilot_token_url, &headers).await
}

/// Fetch available Copilot model IDs.
async fn fetch_available_model_ids(
    copilot_token: &str,
    enterprise_domain: Option<&str>,
) -> Result<Vec<String>, String> {
    let base_url = get_copilot_base_url(Some(copilot_token), enterprise_domain);
    let url = format!("{}/models", base_url);

    let auth_val = format!("Bearer {}", copilot_token);
    let mut headers: Vec<(&str, &str)> =
        vec![("Accept", "application/json"), ("Authorization", &auth_val)];
    for (k, v) in COPILOT_HEADERS {
        headers.push((k, v));
    }
    headers.push(("X-GitHub-Api-Version", COPILOT_API_VERSION));

    let raw = fetch_json(&url, &headers).await?;

    // Parse model list
    let data = raw.get("data").and_then(|d| d.as_array());
    match data {
        Some(items) => {
            let ids: Vec<String> = items
                .iter()
                .filter(|item| {
                    let policy = item.get("policy").and_then(|p| p.as_object());
                    let capabilities = item.get("capabilities").and_then(|c| c.as_object());
                    let supports = capabilities
                        .and_then(|c| c.get("supports"))
                        .and_then(|s| s.as_object());
                    let model_picker_enabled = item
                        .get("model_picker_enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let policy_enabled =
                        policy.and_then(|p| p.get("state")).and_then(|s| s.as_str())
                            != Some("disabled");
                    let supports_tool_calls = supports
                        .and_then(|s| s.get("tool_calls"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    model_picker_enabled && policy_enabled && supports_tool_calls
                })
                .filter_map(|item| {
                    item.get("id")
                        .and_then(|id| id.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            Ok(ids)
        }
        None => Err("Invalid Copilot models response: missing data array".to_string()),
    }
}

/// Enable a model via the policy endpoint.
async fn enable_model(
    copilot_token: &str,
    model_id: &str,
    enterprise_domain: Option<&str>,
) -> Result<bool, String> {
    let base_url = get_copilot_base_url(Some(copilot_token), enterprise_domain);
    let url = format!("{}/models/{}/policy", base_url, model_id);

    let client = tls::reqwest_client();
    let auth_header = format!("Bearer {}", copilot_token);
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", &auth_header)
        .header("openai-intent", "chat-policy")
        .header("x-interaction-type", "chat-policy");
    for (k, v) in COPILOT_HEADERS {
        req = req.header(*k, *v);
    }
    let body = serde_json::json!({"state": "enabled"});
    let resp = req.json(&body).send().await;
    Ok(resp.map(|r| r.status().is_success()).unwrap_or(false))
}

/// Enable all known GitHub Copilot models after login.
async fn enable_all_models(
    copilot_token: &str,
    enterprise_domain: Option<&str>,
    on_progress: &mut (dyn FnMut(String, bool) + Send),
) {
    // Known Copilot model IDs from GITHUB_COPILOT_MODELS
    let known_models = [
        "claude-sonnet-4-20250514",
        "claude-sonnet-4.5-preview-20250619",
        "claude-opus-4-20250514",
        "claude-opus-4.5-preview-20250619",
        "claude-haiku-4-20250514",
        "claude-haiku-4.5-preview-20250619",
        "claude-fable-5",
        "claude-haiku-4.5",
        "claude-opus-4.5",
        "claude-sonnet-4",
        "gpt-4o",
        "gpt-4o-mini",
        "o3",
        "o4-mini",
        "gemini-2.5-flash-001",
        "gemini-2.5-pro-001",
    ];

    // Pi-compatible parallel enabling via join_all
    use futures::future::join_all;
    let tasks: Vec<_> = known_models
        .iter()
        .map(|model_id| {
            let token = copilot_token.to_string();
            let domain = enterprise_domain.map(|s| s.to_string());
            let mid = model_id.to_string();
            async move {
                let success = enable_model(&token, &mid, domain.as_deref())
                    .await
                    .unwrap_or(false);
                (mid, success)
            }
        })
        .collect();

    let results = join_all(tasks).await;
    for (model_id, success) in results {
        on_progress(model_id, success);
    }
}

// ── OAuthProvider implementation ───────────────────────────────────

pub struct GitHubCopilotOAuth;

#[async_trait]
impl OAuthProvider for GitHubCopilotOAuth {
    fn id(&self) -> &str {
        "github-copilot"
    }

    fn name(&self) -> &str {
        "GitHub Copilot"
    }

    async fn login(
        &self,
        callbacks: &mut OAuthLoginCallbacks<'_>,
    ) -> Result<OAuthCredentials, String> {
        // 1. Prompt for enterprise domain
        let input = (callbacks.on_prompt)(OAuthPrompt::Text {
            message: "GitHub Enterprise URL/domain (blank for github.com)".to_string(),
            placeholder: Some("company.ghe.com".to_string()),
            allow_empty: true,
        })?;

        if let Some(ref cancel) = callbacks.signal
            && cancel.is_cancelled()
        {
            return Err("Login cancelled".to_string());
        }

        let trimmed = input.trim().to_string();
        let enterprise_domain = if trimmed.is_empty() {
            None
        } else {
            normalize_domain(&trimmed)
        };
        if !trimmed.is_empty() && enterprise_domain.is_none() {
            return Err("Invalid GitHub Enterprise URL/domain".to_string());
        }
        let domain = enterprise_domain
            .clone()
            .unwrap_or_else(|| "github.com".to_string());

        // 2. Start device flow
        let device_resp = start_device_flow(&domain).await?;

        let device_code = device_resp
            .get("device_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing device_code in response".to_string())?
            .to_string();
        let user_code = device_resp
            .get("user_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing user_code in response".to_string())?
            .to_string();
        let verification_uri = device_resp
            .get("verification_uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing verification_uri in response".to_string())?
            .to_string();
        let interval = device_resp
            .get("interval")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let expires_in = device_resp
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);

        // Validate verification_uri is a trusted URL
        if let Ok(parsed) = url::Url::parse(&verification_uri) {
            if parsed.scheme() != "https" && parsed.scheme() != "http" {
                return Err("Untrusted verification_uri in device code response".to_string());
            }
        } else {
            return Err("Invalid verification_uri in device code response".to_string());
        }

        // 3. Notify user with device code info
        (callbacks.on_device_code)(DeviceCodeInfo {
            user_code: user_code.clone(),
            verification_uri: verification_uri.clone(),
            interval_seconds: interval,
            expires_in_seconds: expires_in,
        });

        // 4. Poll for GitHub access token
        let cancel = callbacks.signal.clone();
        let github_access_token =
            poll_for_github_access_token(&domain, &device_code, interval, expires_in, cancel)
                .await?;

        // 5. Exchange for Copilot token
        let copilot_resp =
            exchange_for_copilot_token(&github_access_token, enterprise_domain.as_deref()).await?;

        let token = copilot_resp
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing token in Copilot response".to_string())?
            .to_string();
        let expires_at = copilot_resp
            .get("expires_at")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| "Missing expires_at in Copilot response".to_string())?
            as i64;

        // 6. Enable all models
        (callbacks.on_progress)("Enabling models...".to_string());
        enable_all_models(
            &token,
            enterprise_domain.as_deref(),
            &mut |model, success| {
                (callbacks.on_progress)(format!(
                    "Model {}: {}",
                    model,
                    if success { "enabled" } else { "skipped" }
                ));
            },
        )
        .await;

        // 7. Fetch available model IDs
        let available_ids = fetch_available_model_ids(&token, enterprise_domain.as_deref())
            .await
            .unwrap_or_default();

        let mut extra = HashMap::new();
        extra.insert("availableModelIds".to_string(), available_ids.join(","));
        if let Some(ref ed) = enterprise_domain {
            extra.insert("enterpriseUrl".to_string(), ed.clone());
        }

        Ok(OAuthCredentials {
            access: token.clone(),
            refresh: github_access_token,
            expires: (expires_at * 1000) - (5 * 60 * 1000), // 5 min buffer
            enterprise_url: enterprise_domain,
            extra,
        })
    }

    async fn refresh_token(
        &self,
        credentials: &OAuthCredentials,
    ) -> Result<OAuthCredentials, String> {
        let enterprise_domain = credentials.enterprise_url.as_deref();
        let raw = refresh_copilot_access_token(&credentials.refresh, enterprise_domain).await?;

        let token = raw
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing token in Copilot refresh response".to_string())?
            .to_string();
        let expires_at = raw
            .get("expires_at")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| "Missing expires_at in Copilot refresh response".to_string())?
            as i64;

        // Fetch available model IDs
        let available_ids = fetch_available_model_ids(&token, enterprise_domain)
            .await
            .unwrap_or_default();

        let mut extra = credentials.extra.clone();
        extra.insert("availableModelIds".to_string(), available_ids.join(","));

        Ok(OAuthCredentials {
            access: token,
            refresh: credentials.refresh.clone(),
            expires: (expires_at * 1000) - (5 * 60 * 1000),
            enterprise_url: credentials.enterprise_url.clone(),
            extra,
        })
    }

    fn get_api_key<'a>(&self, credentials: &'a OAuthCredentials) -> &'a str {
        &credentials.access
    }
}
