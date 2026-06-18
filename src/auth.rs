use anyhow::Context;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Credential for a provider (mirrors pi's auth.json schema).
#[derive(Debug, Clone, Deserialize)]
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

    fn path() -> anyhow::Result<PathBuf> {
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
