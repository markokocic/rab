//! Auth storage — read/write `~/.rab/agent/auth.json`.
//!
//! Pi-compatible credential store with file locking and OAuth auto-refresh.
//! Inspired by pi's `AuthStorage` + `AuthStorageBackend` pattern.
//!
//! Format (pi-compatible):
//! ```json
//! { "opencode-go": { "type": "api_key", "key": "sk-..." } }
//! ```

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

// ── Credential types ─────────────────────────────────────────────

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

// ── Backend enum (pi's AuthStorageBackend pattern) ────────────

/// Pluggable storage backend for `AuthStorage`.
///
/// Every access (read or write) goes through `with_lock`, ensuring
/// atomic read-modify-write semantics. This prevents the race where
/// a lock-free read observes a truncated/partial file during a write.
pub enum AuthStorageBackend {
    File(FileAuthStorageBackend),
    InMemory(InMemoryAuthStorageBackend),
}

impl AuthStorageBackend {
    /// Execute `f` under exclusive lock.
    ///
    /// * `current` — current raw file content (`None` if file doesn't exist / empty).
    /// * Return — `(T, Option<String>)` where the second element is the new
    ///   content to write, or `None` to leave the file unchanged.
    fn with_lock<T>(&self, f: impl FnOnce(Option<String>) -> (T, Option<String>)) -> T {
        match self {
            AuthStorageBackend::File(b) => b.with_lock(f),
            AuthStorageBackend::InMemory(b) => b.with_lock(f),
        }
    }
}

// ── File backend ──────────────────────────────────────────────

/// File-based backend backed by `~/.rab/agent/auth.json` (or a custom path).
///
/// Uses `fs2` for exclusive file locking. Every access (read and write) is
/// performed under the lock, preventing the race condition where a concurrent
/// read observes a truncated or partially-written file.
pub struct FileAuthStorageBackend {
    path: PathBuf,
}

impl FileAuthStorageBackend {
    /// Create a new file backend at the given path.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Create a file backend at the default `~/.rab/agent/auth.json`.
    pub fn default_path() -> anyhow::Result<PathBuf> {
        let dir = directories::BaseDirs::new().context("Could not determine home directory")?;
        Ok(dir.home_dir().join(".rab").join("agent").join("auth.json"))
    }

    fn with_exclusive_lock<T>(&self, f: impl FnOnce(&mut std::fs::File) -> T) -> T {
        use fs2::FileExt;

        // Ensure parent dir exists
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Open or create the auth file
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&self.path)
            .expect("Failed to open auth file");

        // Retry loop for lock acquisition (pi-compatible)
        let mut attempts = 0;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    attempts += 1;
                    if attempts >= 200 {
                        break; // Give up and proceed anyway
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => panic!("Failed to lock auth file: {}", e),
            }
        }

        let result = f(&mut file);
        let _ = file.unlock();
        result
    }
}

impl FileAuthStorageBackend {
    fn with_lock<T>(&self, f: impl FnOnce(Option<String>) -> (T, Option<String>)) -> T {
        use std::io::{Read, Seek, Write};

        self.with_exclusive_lock(|file| {
            // Read current content through the locked handle
            let content = {
                let mut s = String::new();
                let _ = file.rewind();
                match file.read_to_string(&mut s) {
                    Ok(_) if s.is_empty() => None,
                    Ok(_) => Some(s),
                    Err(_) => None,
                }
            };

            let (result, next) = f(content);
            if let Some(new_content) = next {
                // Write through the already-open handle to avoid a second
                // open which would cause a sharing violation on Windows.
                file.set_len(0).ok();
                file.rewind().ok();
                file.write_all(new_content.as_bytes()).ok();
                file.flush().ok();
            }
            result
        })
    }
}

// ── In-memory backend (for testing) ────────────────────────────

/// In-memory backend for testing. Never touches the filesystem.
pub struct InMemoryAuthStorageBackend {
    data: Mutex<Option<String>>,
}

impl InMemoryAuthStorageBackend {
    /// Create an empty in-memory backend.
    pub fn new() -> Self {
        Self {
            data: Mutex::new(None),
        }
    }

    /// Create an in-memory backend seeded with initial data.
    pub fn with_data(data: &str) -> Self {
        Self {
            data: Mutex::new(Some(data.to_string())),
        }
    }

    fn with_lock<T>(&self, f: impl FnOnce(Option<String>) -> (T, Option<String>)) -> T {
        let mut guard = self.data.lock().unwrap();
        let (result, next) = f(guard.clone());
        if let Some(new_content) = next {
            *guard = Some(new_content);
        }
        result
    }
}

impl Default for InMemoryAuthStorageBackend {
    fn default() -> Self {
        Self::new()
    }
}

// ── AuthStorage ────────────────────────────────────────────────

/// Credential store backed by a pluggable `AuthStorageBackend`.
///
/// All read/write operations go through the backend's `with_lock`,
/// ensuring atomic read-modify-write semantics.
///
/// # Examples
///
/// ```ignore
/// // File-backed (real usage)
/// let storage = AuthStorage::create()?;
///
/// // In-memory (testing)
/// let storage = AuthStorage::in_memory();
/// storage.set("my-provider", AuthCredential::ApiKey { key: "sk-..." });
/// ```
pub struct AuthStorage {
    backend: AuthStorageBackend,
    // Cache: populated on construction / reload, updated on writes.
    cache: Mutex<HashMap<String, AuthCredential>>,
}

impl AuthStorage {
    /// Create a file-backed `AuthStorage` at the default path.
    pub fn create() -> anyhow::Result<Self> {
        let path = FileAuthStorageBackend::default_path()?;
        Ok(Self::from_backend(AuthStorageBackend::File(
            FileAuthStorageBackend::new(path),
        )))
    }

    /// Create a file-backed `AuthStorage` at an explicit path.
    pub fn with_path(path: PathBuf) -> Self {
        Self::from_backend(AuthStorageBackend::File(FileAuthStorageBackend::new(path)))
    }

    /// Create an in-memory `AuthStorage` (for testing).
    pub fn in_memory() -> Self {
        Self::from_backend(AuthStorageBackend::InMemory(
            InMemoryAuthStorageBackend::new(),
        ))
    }

    /// Create an in-memory `AuthStorage` seeded with a JSON string (for testing).
    pub fn in_memory_with(data: &str) -> Self {
        Self::from_backend(AuthStorageBackend::InMemory(
            InMemoryAuthStorageBackend::with_data(data),
        ))
    }

    /// Create an `AuthStorage` from a custom backend.
    pub fn from_backend(backend: AuthStorageBackend) -> Self {
        let storage = Self {
            backend,
            cache: Mutex::new(HashMap::new()),
        };
        storage.reload();
        storage
    }

    // ── Load / reload ──────────────────────────────────────────

    /// Reload credentials from the backend.
    /// All reads go through `with_lock`, ensuring we never see partial writes.
    pub fn reload(&self) {
        let result: anyhow::Result<HashMap<String, AuthCredential>> =
            self.backend.with_lock(|content| {
                let data = match content {
                    Some(c) if !c.is_empty() => {
                        serde_json::from_str(&c).with_context(|| "Failed to parse auth.json")
                    }
                    _ => Ok(HashMap::new()),
                };
                (data, None)
            });

        if let Ok(data) = result {
            *self.cache.lock().unwrap() = data;
        }
    }

    // ── Read operations (from cache) ───────────────────────────

    /// Get the API key for a provider. Returns None if not configured or if OAuth.
    pub fn api_key(&self, provider: &str) -> Option<String> {
        self.cache
            .lock()
            .unwrap()
            .get(provider)
            .and_then(|cred| match cred {
                AuthCredential::ApiKey { key } => Some(key.clone()),
                AuthCredential::Oauth { .. } => None,
            })
    }

    /// Get the OAuth access token for a provider.
    /// Returns None if not configured, if API key, or if the token is expired
    /// (past the buffered expiration).
    pub fn oauth_token(&self, provider: &str) -> Option<String> {
        self.cache
            .lock()
            .unwrap()
            .get(provider)
            .and_then(|cred| match cred {
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

    /// Get the OAuth access token even if past the buffer, as long as it's
    /// not truly expired (past the actual `expires_at`).
    ///
    /// The stored `expires` already has a 5-minute buffer subtracted, so this
    /// adds the buffer back before checking: token is accepted until the real
    /// expiry. Returns None if the token is fully expired, not configured, or
    /// if the credential is an API key.
    pub fn oauth_token_past_buffer(&self, provider: &str) -> Option<String> {
        self.cache
            .lock()
            .unwrap()
            .get(provider)
            .and_then(|cred| match cred {
                AuthCredential::Oauth {
                    access, expires, ..
                } => {
                    // The stored expires already has the 5-min buffer subtracted.
                    // Restore it to get the actual API-side expiration.
                    let actual = expires.map(|e| e + 300_000);
                    if is_expired(actual) {
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
        self.cache
            .lock()
            .unwrap()
            .get(provider)
            .cloned()
            .and_then(|cred| match cred {
                AuthCredential::Oauth { .. } => Some(cred),
                AuthCredential::ApiKey { .. } => None,
            })
    }

    /// Get all stored credentials.
    pub fn all_credentials(&self) -> HashMap<String, AuthCredential> {
        self.cache.lock().unwrap().clone()
    }

    /// Get raw credential for a provider (both API key and OAuth).
    pub fn get(&self, provider: &str) -> Option<AuthCredential> {
        self.cache.lock().unwrap().get(provider).cloned()
    }

    /// List all providers with stored credentials.
    pub fn list(&self) -> Vec<String> {
        self.cache.lock().unwrap().keys().cloned().collect()
    }

    // ── Write operations (through backend lock) ────────────────

    /// Set an API key credential for a provider.
    pub fn set_api_key(&self, provider: &str, api_key: &str) -> anyhow::Result<()> {
        let p = provider.to_string();
        let k = api_key.to_string();
        self.modify_all(move |auth| {
            let mut map = auth;
            map.insert(p, AuthCredential::ApiKey { key: k });
            map
        })
    }

    /// Set an OAuth credential for a provider.
    pub fn set_oauth(&self, provider: &str, cred: &AuthCredential) -> anyhow::Result<()> {
        let p = provider.to_string();
        let c = cred.clone();
        self.modify_all(move |auth| {
            let mut map = auth;
            map.insert(p, c);
            map
        })
    }

    /// Remove a provider's credential. Returns true if something was removed.
    pub fn remove(&self, provider: &str) -> anyhow::Result<bool> {
        let p = provider.to_string();
        let removed = self.backend.with_lock(|current| {
            let data: HashMap<String, AuthCredential> = match &current {
                Some(c) if !c.is_empty() => match serde_json::from_str(c) {
                    Ok(d) => d,
                    Err(e) => return (Err(anyhow::Error::from(e)), None),
                },
                _ => HashMap::new(),
            };
            let mut updated = data;
            let removed = updated.remove(&p).is_some();
            if removed {
                let next = if updated.is_empty() {
                    // Write empty object, not empty file, to keep valid JSON
                    Some("{}".to_string())
                } else {
                    Some(serde_json::to_string_pretty(&updated).unwrap())
                };
                (Ok(removed), next)
            } else {
                (Ok(false), None)
            }
        })?;
        if removed {
            self.reload();
        }
        Ok(removed)
    }

    /// Remove all credentials. Returns true if anything was removed.
    pub fn clear(&self) -> anyhow::Result<bool> {
        let cleared = self.backend.with_lock(|current| {
            let data: HashMap<String, AuthCredential> = match &current {
                Some(c) if !c.is_empty() => match serde_json::from_str(c) {
                    Ok(d) => d,
                    Err(e) => return (Err(anyhow::Error::from(e)), None),
                },
                _ => HashMap::new(),
            };
            let had = !data.is_empty();
            if had {
                (Ok(true), Some("{}".to_string()))
            } else {
                (Ok(false), None)
            }
        })?;
        if cleared {
            self.reload();
        }
        Ok(cleared)
    }

    /// Atomically modify a provider's credential (pi-compatible `CredentialStore.modify()`).
    /// `f` receives the current credential (None if missing), returns the new
    /// credential, or None to delete the entry.
    pub fn modify(
        &self,
        provider: &str,
        f: impl FnOnce(Option<AuthCredential>) -> Option<AuthCredential>,
    ) -> anyhow::Result<()> {
        let p = provider.to_string();
        let result = self.backend.with_lock(|current| {
            let mut data: HashMap<String, AuthCredential> = match &current {
                Some(c) if !c.is_empty() => match serde_json::from_str(c) {
                    Ok(d) => d,
                    Err(e) => return (Err(anyhow::Error::from(e)), None),
                },
                _ => HashMap::new(),
            };

            let current_cred = data.get(&p).cloned();
            let next = f(current_cred);
            let changed = match &next {
                Some(cred) => {
                    data.insert(p.clone(), cred.clone());
                    true
                }
                None => data.remove(&p).is_some(),
            };

            if changed {
                let content = serde_json::to_string_pretty(&data).unwrap();
                (Ok(()), Some(content))
            } else {
                (Ok(()), None)
            }
        });

        result?;
        self.reload();
        Ok(())
    }

    // ── Internal helpers ───────────────────────────────────────

    /// Internal: replace all credentials (used by `set_api_key`, `set_oauth`).
    fn modify_all(
        &self,
        f: impl FnOnce(HashMap<String, AuthCredential>) -> HashMap<String, AuthCredential>,
    ) -> anyhow::Result<()> {
        self.backend.with_lock(|current| {
            let data: HashMap<String, AuthCredential> = match &current {
                Some(c) if !c.is_empty() => match serde_json::from_str(c) {
                    Ok(d) => d,
                    Err(e) => return (Err(anyhow::Error::from(e)), None),
                },
                _ => HashMap::new(),
            };
            let updated = f(data);
            let content = serde_json::to_string_pretty(&updated).unwrap();
            (Ok(()), Some(content))
        })?;

        self.reload();
        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────

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

// ── Free functions (backward-compatible thin wrappers) ──────────

use std::sync::OnceLock;

fn default_storage() -> &'static AuthStorage {
    static STORAGE: OnceLock<AuthStorage> = OnceLock::new();
    STORAGE.get_or_init(|| AuthStorage::create().unwrap_or_else(|_| AuthStorage::in_memory()))
}

/// Login a provider by storing its API key in auth.json.
pub fn login(provider: &str, api_key: &str) -> anyhow::Result<()> {
    default_storage().set_api_key(provider, api_key)
}

/// Login a provider by storing its OAuth credentials in auth.json.
pub fn login_oauth(provider: &str, cred: &AuthCredential) -> anyhow::Result<()> {
    default_storage().set_oauth(provider, cred)
}

/// Logout a provider by removing its credential from auth.json.
/// If `provider` is `None`, clears all credentials.
/// Returns true if something was actually removed.
pub fn logout(provider: Option<&str>) -> anyhow::Result<bool> {
    match provider {
        Some(p) => default_storage().remove(p),
        None => default_storage().clear(),
    }
}

/// List all providers that have credentials stored.
pub fn list_logged_in() -> anyhow::Result<Vec<String>> {
    Ok(default_storage().list())
}

/// Read a credential from auth.json. Returns None if the provider has no stored credential.
pub fn read_credential(provider: &str) -> anyhow::Result<Option<AuthCredential>> {
    Ok(default_storage().get(provider))
}

/// Atomically modify a single provider's credential (pi-compatible `CredentialStore.modify()`).
pub fn modify_credential(
    provider: &str,
    f: impl FnOnce(Option<AuthCredential>) -> Option<AuthCredential>,
) -> anyhow::Result<()> {
    default_storage().modify(provider, f)
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

    let oauth_provider = crate::oauth::get(provider)?;

    // Build OAuthCredentials for the refresh call
    let oauth_creds = match &oauth_cred {
        AuthCredential::Oauth {
            access,
            refresh,
            expires,
            enterprise_url,
            ..
        } => crate::oauth::OAuthCredentials {
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
