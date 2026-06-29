//! Auth storage — read/write `~/.rab/agent/auth.json`.
//!
//! Format (pi-compatible):
//! ```json
//! { "opencode-go": { "type": "api_key", "key": "sk-..." } }
//! ```

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Credential for a provider (mirrors pi's auth.json schema).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthCredential {
    #[serde(rename = "api_key")]
    ApiKey { key: String },
    #[serde(rename = "oauth")]
    Oauth {
        access: String,
        refresh: Option<String>,
        expires: Option<i64>,
        #[serde(rename = "enterpriseUrl")]
        enterprise_url: Option<String>,
    },
}

/// Auth storage loaded from ~/.rab/auth.json.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthStorage(HashMap<String, AuthCredential>);

impl AuthStorage {
    /// Load auth from `~/.rab/agent/auth.json`. Returns empty if file doesn't exist.
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(Self::path()?)
    }

    /// Load auth from an explicit path (for testing).
    pub fn load_from(path: std::path::PathBuf) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))
    }

    /// Get the path to the auth file.
    pub fn path() -> anyhow::Result<PathBuf> {
        let dir = directories::BaseDirs::new().context("Could not determine home directory")?;
        Ok(dir.home_dir().join(".rab").join("agent").join("auth.json"))
    }

    /// Get the API key for a provider. Returns None if not configured or if OAuth.
    pub fn api_key(&self, provider: &str) -> Option<String> {
        self.0.get(provider).and_then(|cred| match cred {
            AuthCredential::ApiKey { key } => Some(key.clone()),
            AuthCredential::Oauth { .. } => None,
        })
    }

    /// Get the OAuth access token for a provider. Returns None if not configured or if API key.
    pub fn oauth_token(&self, provider: &str) -> Option<String> {
        self.0.get(provider).and_then(|cred| match cred {
            AuthCredential::Oauth { access, .. } => Some(access.clone()),
            AuthCredential::ApiKey { .. } => None,
        })
    }
}

// ── Write operations ─────────────────────────────────────────────

/// Login a provider by storing its API key in auth.json.
/// If the provider already has a credential, it is overwritten.
pub fn login(provider: &str, api_key: &str) -> anyhow::Result<()> {
    let path = AuthStorage::path()?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut auth: HashMap<String, AuthCredential> = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?
    } else {
        HashMap::new()
    };

    auth.insert(
        provider.to_string(),
        AuthCredential::ApiKey {
            key: api_key.to_string(),
        },
    );

    let content = serde_json::to_string_pretty(&auth)?;
    std::fs::write(&path, &content)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Logout a provider by removing its credential from auth.json.
/// If `provider` is `None`, clears all credentials.
/// Returns true if something was actually removed.
pub fn logout(provider: Option<&str>) -> anyhow::Result<bool> {
    let path = AuthStorage::path()?;
    if !path.exists() {
        return Ok(false);
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mut auth: HashMap<String, AuthCredential> = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;

    let removed = match provider {
        Some(prov) => auth.remove(prov).is_some(),
        None => {
            let count = auth.len();
            auth.clear();
            count > 0
        }
    };

    if removed {
        let new_content = serde_json::to_string_pretty(&auth)?;
        std::fs::write(&path, &new_content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
    }
    Ok(removed)
}

/// List all providers that have credentials stored.
pub fn list_logged_in() -> anyhow::Result<Vec<String>> {
    let path = AuthStorage::path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let auth: HashMap<String, AuthCredential> = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(auth.keys().cloned().collect())
}
