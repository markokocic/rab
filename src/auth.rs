//! Auth storage — read/write `~/.rab/agent/auth.json`.
//!
//! Pi-compatible credential store with file locking and OAuth auto-refresh.
//!
//! Format (pi-compatible):
//! ```json
//! { "opencode-go": { "type": "api_key", "key": "sk-..." } }
//! ```

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

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
        let content = read_json_file(&path)?;
        match content {
            Some(c) => serde_json::from_str(&c)
                .with_context(|| format!("Failed to parse {}", path.display())),
            None => Ok(Self::default()),
        }
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

    /// Get the OAuth access token for a provider.
    /// Returns None if not configured, if API key, or if the token is expired.
    pub fn oauth_token(&self, provider: &str) -> Option<String> {
        self.0.get(provider).and_then(|cred| match cred {
            AuthCredential::Oauth {
                access, expires, ..
            } => {
                if is_expired(*expires) {
                    return None;
                }
                Some(access.clone())
            }
            AuthCredential::ApiKey { .. } => None,
        })
    }

    /// Get the stored credential for a provider, if it's an OAuth credential.
    /// Returns None for API key credentials or missing entries.
    pub fn oauth_credential(&self, provider: &str) -> Option<AuthCredential> {
        self.0.get(provider).cloned().and_then(|cred| match cred {
            AuthCredential::Oauth { .. } => Some(cred),
            AuthCredential::ApiKey { .. } => None,
        })
    }

    /// Get all stored credentials.
    pub fn all_credentials(&self) -> &HashMap<String, AuthCredential> {
        &self.0
    }
}

// ── File locking helpers ────────────────────────────────────────

/// Acquire an exclusive file lock (blocking with retry) and run the closure.
/// Uses `fs2::FileExt::try_lock_exclusive` for cross-process safety,
/// matching pi's `proper-lockfile` pattern.
fn with_exclusive_lock<T>(path: &PathBuf, f: impl FnOnce() -> T) -> T {
    use fs2::FileExt;

    // Ensure parent dir exists
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Open or create the auth file itself (no truncate — we're just getting
    // a fd for locking; actual reads/writes are done separately).
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(path)
        .expect("Failed to open auth file");

    // Retry loop for lock acquisition (pi-compatible)
    let mut attempts = 0;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => break,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                attempts += 1;
                // Stale lock detection: if the lock is held for >10s, it's stale
                if attempts >= 200 {
                    break; // Give up and proceed anyway
                }
                if attempts > 5
                    && let Ok(metadata) = path.metadata()
                    && let Ok(modified) = metadata.modified()
                    && let Ok(elapsed) = modified.elapsed()
                    && elapsed > Duration::from_secs(10)
                {
                    // Stale lock — break it by unlocking and retrying
                    let _ = file.unlock();
                    continue;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("Failed to lock auth file: {}", e),
        }
    }

    let result = f();
    let _ = file.unlock();
    result
}

/// Read JSON from a file (no locking — caller should use exclusive lock for writes).
/// Returns None if the file doesn't exist.
fn read_json_file(path: &PathBuf) -> anyhow::Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut s = String::new();
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    file.read_to_string(&mut s)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(Some(s))
}

/// Read-modify-write the auth file under an exclusive lock.
fn modify_auth_file(
    path: &PathBuf,
    f: impl FnOnce(HashMap<String, AuthCredential>) -> (HashMap<String, AuthCredential>, bool),
) -> anyhow::Result<()> {
    with_exclusive_lock(path, || {
        let auth: HashMap<String, AuthCredential> = match read_json_file(path) {
            Ok(Some(c)) => serde_json::from_str(&c).unwrap_or_default(),
            _ => HashMap::new(),
        };

        let (result, changed) = f(auth);
        if changed {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(content) = serde_json::to_string_pretty(&result) {
                let _ = std::fs::write(path, &content);
            }
        }
    });
    Ok(())
}

// ── Helper ────────────────────────────────────────────────────

fn is_expired(expires: Option<i64>) -> bool {
    match expires {
        Some(exp) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            now >= exp
        }
        None => false, // No expiry = treat as not expired
    }
}

// ── Write operations ─────────────────────────────────────────────

/// Login a provider by storing its API key in auth.json.
pub fn login(provider: &str, api_key: &str) -> anyhow::Result<()> {
    let path = AuthStorage::path()?;
    let p = provider.to_string();
    let k = api_key.to_string();
    modify_auth_file(&path, |mut auth| {
        auth.insert(p, AuthCredential::ApiKey { key: k });
        (auth, true)
    })
}

/// Login a provider by storing its OAuth credentials in auth.json.
pub fn login_oauth(provider: &str, cred: &AuthCredential) -> anyhow::Result<()> {
    let path = AuthStorage::path()?;
    let p = provider.to_string();
    let c = cred.clone();
    modify_auth_file(&path, |mut auth| {
        auth.insert(p, c);
        (auth, true)
    })
}

/// Logout a provider by removing its credential from auth.json.
/// If `provider` is `None`, clears all credentials.
/// Returns true if something was actually removed.
pub fn logout(provider: Option<&str>) -> anyhow::Result<bool> {
    let path = AuthStorage::path()?;
    if !path.exists() {
        return Ok(false);
    }

    let result = with_exclusive_lock(&path, || -> bool {
        let auth: HashMap<String, AuthCredential> = match read_json_file(&path) {
            Ok(Some(c)) => serde_json::from_str(&c).unwrap_or_default(),
            _ => return false,
        };

        let (new_auth, removed) = match provider {
            Some(prov) => {
                let mut a = auth;
                let removed = a.remove(prov).is_some();
                (a, removed)
            }
            None => {
                let removed = !auth.is_empty();
                (HashMap::new(), removed)
            }
        };

        if removed {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(content) = serde_json::to_string_pretty(&new_auth) {
                let _ = std::fs::write(&path, &content);
            }
        }
        removed
    });

    Ok(result)
}

/// List all providers that have credentials stored.
pub fn list_logged_in() -> anyhow::Result<Vec<String>> {
    let path = AuthStorage::path()?;
    let content = read_json_file(&path)?;
    match content {
        Some(c) => {
            let auth: HashMap<String, AuthCredential> = serde_json::from_str(&c)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            Ok(auth.keys().cloned().collect())
        }
        None => Ok(Vec::new()),
    }
}

// ── Enhanced credential read ──────────────────────────────────────

/// Read a credential from auth.json. Returns None if the provider has no stored credential.
pub fn read_credential(provider: &str) -> anyhow::Result<Option<AuthCredential>> {
    let path = AuthStorage::path()?;
    let content = read_json_file(&path)?;
    match content {
        Some(c) => {
            let auth: HashMap<String, AuthCredential> = serde_json::from_str(&c)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            Ok(auth.get(provider).cloned())
        }
        None => Ok(None),
    }
}

/// Atomically modify a single provider's credential (pi-compatible `CredentialStore.modify()`).
/// `f` receives the current credential (None if missing), returns the new
/// credential, or None to delete the entry.
pub fn modify_credential(
    provider: &str,
    f: impl FnOnce(Option<AuthCredential>) -> Option<AuthCredential>,
) -> anyhow::Result<()> {
    let path = AuthStorage::path()?;
    let p = provider.to_string();
    modify_auth_file(&path, |auth| {
        let current = auth.get(&p).cloned();
        let next = f(current);
        let mut updated = auth;
        match next {
            Some(cred) => {
                updated.insert(p, cred);
            }
            None => {
                updated.remove(&p);
            }
        }
        (updated, true)
    })
}

/// Refresh an expired OAuth token using the registered OAuth provider.
/// Returns the new access token string, or None if refresh fails.
/// Matching pi's `AuthStorage.refreshOAuthTokenWithLock()` pattern.
pub async fn refresh_oauth_token(provider: &str) -> Option<String> {
    let credential = read_credential(provider).ok()??;
    let oauth_cred = match &credential {
        AuthCredential::Oauth { .. } => credential,
        _ => return None,
    };
    let expires = match &oauth_cred {
        AuthCredential::Oauth { expires, .. } => *expires,
        _ => return None,
    };

    // If token is still valid for more than 5 minutes, return current access token
    if !is_expired(Some(expires.unwrap_or(i64::MAX))) {
        let buffer_ms = 300_000;
        if let AuthCredential::Oauth { access, .. } = &oauth_cred {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            if now < expires.unwrap_or(i64::MAX) - buffer_ms {
                return Some(access.clone());
            }
        }
    }

    let oauth_provider = crate::provider::oauth::get(provider)?;

    // Build OAuthCredentials for the refresh call
    let oauth_creds = match &oauth_cred {
        AuthCredential::Oauth {
            access,
            refresh,
            expires,
            enterprise_url,
            ..
        } => crate::provider::oauth::OAuthCredentials {
            access: access.clone(),
            refresh: refresh.clone().unwrap_or_default(),
            expires: expires.unwrap_or(0),
            enterprise_url: enterprise_url.clone(),
            extra: std::collections::HashMap::new(),
        },
        _ => return None,
    };

    let new_creds = oauth_provider.refresh_token(&oauth_creds).await.ok()?;
    let new_access = new_creds.access.clone();

    // Store updated credentials under file lock
    let result = modify_credential(provider, |_| {
        Some(AuthCredential::Oauth {
            access: new_creds.access.clone(),
            refresh: Some(new_creds.refresh),
            expires: Some(new_creds.expires),
            enterprise_url: new_creds.enterprise_url,
        })
    });

    match result {
        Ok(_) => Some(new_access),
        Err(_) => None,
    }
}
