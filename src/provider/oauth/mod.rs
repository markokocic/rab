//! OAuth provider trait and registry — matching pi's OAuthProviderInterface.
//!
//! Each OAuth provider implements login (device code or callback-server flow),
//! token refresh, and API key derivation.

use std::collections::HashMap;

use async_trait::async_trait;
use std::sync::{Arc, Mutex, OnceLock};

pub mod device_code;
pub mod github_copilot;

/// Credentials returned from a successful OAuth login or refresh.
#[derive(Debug, Clone)]
pub struct OAuthCredentials {
    pub access: String,
    pub refresh: String,
    pub expires: i64, // epoch ms
    pub enterprise_url: Option<String>,
    /// Provider-specific extra data (e.g. available model IDs for Copilot).
    pub extra: HashMap<String, String>,
}

/// Info passed to `on_device_code` callback.
#[derive(Debug, Clone)]
pub struct DeviceCodeInfo {
    pub user_code: String,
    pub verification_uri: String,
    pub interval_seconds: Option<u32>,
    pub expires_in_seconds: Option<u32>,
}

/// A prompt shown to the user during login.
#[derive(Debug, Clone)]
pub enum OAuthPrompt {
    Text {
        message: String,
        placeholder: Option<String>,
        allow_empty: bool,
    },
}

/// Callbacks the login flow uses to interact with the user.
pub struct OAuthLoginCallbacks<'a> {
    pub on_device_code: Box<dyn FnMut(DeviceCodeInfo) + Send + 'a>,
    pub on_prompt: Box<dyn FnMut(OAuthPrompt) -> Result<String, String> + Send + 'a>,
    pub on_progress: Box<dyn FnMut(String) + Send + 'a>,
    pub signal: Option<tokio_util::sync::CancellationToken>,
}

/// An OAuth provider (matching pi's OAuthProviderInterface).
#[async_trait]
pub trait OAuthProvider: Send + Sync {
    /// The provider ID (matches the registry/provider id).
    fn id(&self) -> &str;

    /// Human-readable name.
    fn name(&self) -> &str;

    /// Run the login flow (device code for Copilot, callback-server for Anthropic).
    async fn login(
        &self,
        callbacks: &mut OAuthLoginCallbacks<'_>,
    ) -> Result<OAuthCredentials, String>;

    /// Refresh an expired token.
    async fn refresh_token(
        &self,
        credentials: &OAuthCredentials,
    ) -> Result<OAuthCredentials, String>;

    /// Derive the API key (access token) for API requests.
    fn get_api_key<'a>(&self, credentials: &'a OAuthCredentials) -> &'a str;
}

// ── Registry ───────────────────────────────────────────────────────

static BUILT_IN_PROVIDERS: &[&str] = &["github-copilot"];

static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<dyn OAuthProvider>>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, Arc<dyn OAuthProvider>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register an OAuth provider.
pub fn register(provider: Arc<dyn OAuthProvider>) {
    registry()
        .lock()
        .unwrap()
        .insert(provider.id().to_string(), provider);
}

/// Get an OAuth provider by ID.
pub fn get(id: &str) -> Option<Arc<dyn OAuthProvider>> {
    registry().lock().unwrap().get(id).cloned()
}

/// List all registered OAuth provider IDs.
pub fn list_ids() -> Vec<String> {
    registry().lock().unwrap().keys().cloned().collect()
}

/// Check if a provider ID corresponds to a built-in OAuth provider.
pub fn is_built_in(id: &str) -> bool {
    BUILT_IN_PROVIDERS.contains(&id)
}

/// Register all built-in OAuth providers (called once at startup).
pub fn register_builtins() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let gh = crate::provider::oauth::github_copilot::GitHubCopilotOAuth;
        register(Arc::new(gh));
    });
}
