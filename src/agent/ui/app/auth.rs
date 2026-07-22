//! Authentication dialogs and login/logout flows.
//!
//! Extracted from `mod.rs` to reduce file size.

use super::App;
use super::chat::show_status;
use super::types::OverlayResult;
use crate::agent::ui::components::oauth_selector::AuthType;
use crate::provider::auth;
use crate::tui::TUI;

/// Handle a login command result. If `api_key` is provided, stores it immediately
/// and performs post-login completion (model auto-selection, registry refresh).
pub fn handle_login(app: &mut App, provider: &str, api_key: Option<&str>) {
    let provider = if provider.is_empty() {
        "opencode-go"
    } else {
        provider
    };
    if let Some(key) = api_key {
        match auth::login(provider, key) {
            Ok(_) => {
                app.refresh_registry();
                // Post-login completion
                complete_login(
                    app,
                    provider,
                    crate::agent::ui::components::oauth_selector::AuthType::ApiKey,
                );
            }
            Err(e) => show_status(app, format!("Login failed: {}", e)),
        }
    } else {
        show_status(app, format!("Usage: /login {} <api-key>", provider));
    }
}

/// Handle a logout command result.
pub fn handle_logout(app: &mut App, provider: Option<&str>) {
    match auth::logout(provider) {
        Ok(true) => {
            let msg = provider
                .map(|p| format!("Logged out from {}", p))
                .unwrap_or_else(|| "Logged out from all providers".into());
            show_status(app, msg);
        }
        Ok(false) => {
            let msg = provider
                .map(|p| format!("No credentials for {}", p))
                .unwrap_or_else(|| "No credentials found".into());
            show_status(app, msg);
        }
        Err(e) => {
            show_status(app, format!("Logout failed: {}", e));
        }
    }
}

/// Show the login provider selector overlay, optionally filtered by auth type.
/// Shows all available providers from the registry for the user to pick one.
pub fn show_login_provider_selector(app: &mut App, tui: &mut TUI, auth_type: Option<AuthType>) {
    use crate::agent::ui::components::oauth_selector::{
        AuthSelectorProvider, AuthType, OAuthSelector, SelectorMode,
    };

    let all_providers = app.registry.list_providers();

    // Build the provider list, including OAuth providers from the OAuth registry
    let mut providers: Vec<AuthSelectorProvider> = Vec::new();

    // Add API key providers
    for (id, name) in all_providers {
        let is_oauth_provider = crate::provider::oauth::get(&id).is_some();
        match auth_type {
            Some(AuthType::ApiKey) => {
                // Skip OAuth-only providers (those not in models.json)
                if !is_oauth_provider {
                    providers.push(AuthSelectorProvider {
                        id,
                        name,
                        auth_type: AuthType::ApiKey,
                    });
                }
            }
            Some(AuthType::OAuth) => {
                // Only include OAuth providers
                if is_oauth_provider {
                    providers.push(AuthSelectorProvider {
                        id,
                        name,
                        auth_type: AuthType::OAuth,
                    });
                }
            }
            None => {
                providers.push(AuthSelectorProvider {
                    id,
                    name,
                    auth_type: if is_oauth_provider {
                        AuthType::OAuth
                    } else {
                        AuthType::ApiKey
                    },
                });
            }
        }
    }

    // Also add OAuth providers that aren't in models.json (e.g. only in OAuth registry)
    if auth_type != Some(AuthType::ApiKey) {
        for oauth_id in crate::provider::oauth::list_ids() {
            if !providers.iter().any(|p| p.id == oauth_id)
                && let Some(provider) = crate::provider::oauth::get(&oauth_id)
            {
                providers.push(AuthSelectorProvider {
                    id: oauth_id,
                    name: provider.name().to_string(),
                    auth_type: AuthType::OAuth,
                });
            }
        }
    }

    // Sort alphabetically by name for consistent display.
    providers.sort_by_key(|a| a.name.to_lowercase());

    if providers.is_empty() {
        app.status_text = Some(match auth_type {
            Some(AuthType::OAuth) => "No subscription providers available.".into(),
            Some(AuthType::ApiKey) => "No API key providers available.".into(),
            None => "No providers available.".into(),
        });
        return;
    }

    let signal = app.overlay_result_signal.clone();
    let mut selector = OAuthSelector::new(
        providers,
        |provider_id| app.registry.auth_status_for_provider(provider_id),
        SelectorMode::Login,
    );

    selector.on_select(move |provider_id: String| {
        *signal.borrow_mut() = Some(OverlayResult::LoginProviderSelected(provider_id));
    });
    selector.on_cancel(|| {});

    tui.show_positioned_overlay(Box::new(selector), crate::tui::OverlayPosition::Bottom);
}

/// Show the API key input dialog for a specific provider.
/// Uses LoginDialog which matches pi's LoginDialogComponent.
pub fn show_api_key_login_dialog(app: &mut App, tui: &mut TUI, provider_id: &str) {
    use crate::agent::ui::components::LoginDialog;

    // Find the provider name from the registry
    let provider_name = app
        .registry
        .list_providers()
        .into_iter()
        .find(|(id, _)| id == provider_id)
        .map(|(_, name)| name)
        .unwrap_or_else(|| provider_id.to_string());

    let mut dialog = LoginDialog::new(provider_id.to_string(), provider_name.clone());

    let signal = app.overlay_result_signal.clone();
    let provider_id_clone = provider_id.to_string();

    dialog.on_submit(move |api_key: String| {
        *signal.borrow_mut() = Some(OverlayResult::LoginApiKeyProvided {
            provider: provider_id_clone,
            key: api_key,
        });
    });

    dialog.on_cancel(|| {});

    dialog.show_prompt("Enter API key:", Some("sk-..."));

    tui.show_positioned_overlay(Box::new(dialog), crate::tui::OverlayPosition::Bottom);
}

/// Show the OAuth login dialog for a specific provider.
/// Matches pi's showLoginDialog for OAuth providers.
pub fn show_oauth_login_dialog(app: &mut App, tui: &mut TUI, provider_id: &str) {
    let provider_name = app
        .registry
        .list_providers()
        .into_iter()
        .find(|(id, _)| id == provider_id)
        .map(|(_, name)| name)
        .unwrap_or_else(|| {
            crate::provider::oauth::get(provider_id)
                .map(|p| p.name().to_string())
                .unwrap_or_else(|| provider_id.to_string())
        });

    app.status_text = Some(format!("Starting OAuth login for {}…", provider_name));
    tui.pop_overlay(); // close the provider selector overlay

    // Send progress updates through the agent event channel.
    // ProgressMessage with empty tool_name sets app.status_text (visible to user).
    let tx = app.event_tx.clone();
    let pid = provider_id.to_string();
    let pname = provider_name.clone();

    let tx2 = tx.clone();
    let tx3 = tx.clone();
    let tx4 = tx.clone();

    app.pending_oauth_provider = Some(pid.clone());

    let handle = tokio::spawn(async move {
        let oauth_provider = match crate::provider::oauth::get(&pid) {
            Some(p) => p,
            None => {
                let _ = tx.send(yoagent::types::AgentEvent::ProgressMessage {
                    tool_call_id: String::new(),
                    tool_name: String::new(),
                    text: format!(
                        "OAuth login failed: No OAuth provider registered for '{}'",
                        pid
                    ),
                });
                return;
            }
        };

        let mut callbacks = crate::provider::oauth::OAuthLoginCallbacks {
            on_device_code: Box::new(move |info: crate::provider::oauth::DeviceCodeInfo| {
                let device_msg = format!(
                    "Open {} and enter code: {}",
                    info.verification_uri, info.user_code
                );
                // Show as status AND as a persistent chat message via ToolExecutionEnd
                let _ = tx.send(yoagent::types::AgentEvent::ProgressMessage {
                    tool_call_id: String::new(),
                    tool_name: String::new(),
                    text: device_msg,
                });
            }),
            on_prompt: Box::new(
                move |prompt: crate::provider::oauth::OAuthPrompt| match prompt {
                    crate::provider::oauth::OAuthPrompt::Text {
                        message,
                        placeholder: _,
                        allow_empty: _,
                    } => {
                        // Log the prompt so users see it; empty response = default (github.com)
                        let _ = tx2.send(yoagent::types::AgentEvent::ProgressMessage {
                            tool_call_id: String::new(),
                            tool_name: String::new(),
                            text: format!("{} (empty = github.com)", message),
                        });
                        // For now, accept empty — GitHub Enterprise users need to
                        // set enterprise_url in credentials manually or via config.
                        Ok(String::new())
                    }
                },
            ),
            on_progress: Box::new(move |msg: String| {
                let _ = tx3.send(yoagent::types::AgentEvent::ProgressMessage {
                    tool_call_id: String::new(),
                    tool_name: String::new(),
                    text: format!("[OAuth] {}", msg),
                });
            }),
            signal: None,
        };

        match oauth_provider.login(&mut callbacks).await {
            Ok(credentials) => {
                let cred = crate::provider::auth::AuthCredential::Oauth {
                    access: credentials.access.clone(),
                    refresh: Some(credentials.refresh.clone()),
                    expires: Some(credentials.expires),
                    enterprise_url: credentials.enterprise_url.clone(),
                };
                match crate::provider::auth::login_oauth(&pid, &cred) {
                    Ok(_) => {
                        let _ = tx4.send(yoagent::types::AgentEvent::ProgressMessage {
                            tool_call_id: String::new(),
                            tool_name: String::new(),
                            text: format!("✓ Logged in to {} via OAuth", pname),
                        });
                    }
                    Err(e) => {
                        let _ = tx4.send(yoagent::types::AgentEvent::ProgressMessage {
                            tool_call_id: String::new(),
                            tool_name: String::new(),
                            text: format!("Failed to save OAuth credentials: {}", e),
                        });
                    }
                }
            }
            Err(e) => {
                let _ = tx4.send(yoagent::types::AgentEvent::ProgressMessage {
                    tool_call_id: String::new(),
                    tool_name: String::new(),
                    text: format!("OAuth login failed: {}", e),
                });
            }
        }
    });
    app.oauth_join_handle = Some(handle);
}

/// Show the auth type selector overlay ("Use a subscription" vs "Use an API key").
/// Matches pi's showLoginAuthTypeSelector behavior.
pub fn show_auth_type_selector(app: &mut App, tui: &mut TUI) {
    // Build simple two-option selector
    let signal = app.overlay_result_signal.clone();

    let mut items = vec![crate::tui::components::select_list::SelectItem::new(
        "api_key",
        "Use an API key",
    )];
    // Add OAuth option if any OAuth providers are registered
    let has_oauth = !crate::provider::oauth::list_ids().is_empty();
    if has_oauth {
        items.push(crate::tui::components::select_list::SelectItem::new(
            "oauth",
            "Use a subscription",
        ));
    }

    let filtered_indices: Vec<usize> = (0..items.len()).collect();
    let selected_index: usize = 0;

    struct AuthTypeOverlay {
        items: Vec<crate::tui::components::select_list::SelectItem>,
        selected_index: usize,
        filtered_indices: Vec<usize>,
        signal: std::rc::Rc<std::cell::RefCell<Option<OverlayResult>>>,
    }

    impl crate::tui::Component for AuthTypeOverlay {
        fn render(&mut self, width: usize) -> Vec<String> {
            let theme = crate::agent::ui::theme::current_theme();
            let mut lines = Vec::new();

            lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));
            lines.push(String::new());
            lines.push(format!(
                "  {}",
                theme.bold(&theme.fg("accent", "Select authentication method:"))
            ));
            lines.push(String::new());

            for (i, &item_idx) in self.filtered_indices.iter().enumerate() {
                let item = &self.items[item_idx];
                let is_selected = i == self.selected_index;
                let prefix = if is_selected {
                    theme.fg("accent", "→ ")
                } else {
                    "  ".to_string()
                };
                let text = if is_selected {
                    theme.fg("accent", &item.label)
                } else {
                    theme.fg("text", &item.label)
                };
                lines.push(format!("{}{}", prefix, text));
            }

            lines.push(String::new());
            lines.push(format!("  {}", theme.dim("Enter: select · Esc: cancel")));
            lines.push(String::new());
            lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));

            lines
        }

        fn handle_input(&mut self, key: &crossterm::event::KeyEvent) -> bool {
            let kb = crate::tui::keybindings::get_keybindings();

            if kb.matches(key, crate::tui::keybindings::ACTION_SELECT_UP) {
                if self.filtered_indices.is_empty() {
                    return true;
                }
                self.selected_index = if self.selected_index == 0 {
                    self.filtered_indices.len() - 1
                } else {
                    self.selected_index - 1
                };
                return true;
            }

            if kb.matches(key, crate::tui::keybindings::ACTION_SELECT_DOWN) {
                if self.filtered_indices.is_empty() {
                    return true;
                }
                self.selected_index = if self.selected_index >= self.filtered_indices.len() - 1 {
                    0
                } else {
                    self.selected_index + 1
                };
                return true;
            }

            if kb.matches(key, crate::tui::keybindings::ACTION_SELECT_CONFIRM) {
                if let Some(&idx) = self.filtered_indices.get(self.selected_index) {
                    let value = self.items[idx].value.clone();
                    let auth_type = match value.as_str() {
                        "oauth" => AuthType::OAuth,
                        _ => AuthType::ApiKey,
                    };
                    *self.signal.borrow_mut() =
                        Some(OverlayResult::LoginAuthTypeSelected(auth_type));
                }
                return true;
            }

            if kb.matches(key, crate::tui::keybindings::ACTION_SELECT_CANCEL) {
                // Cancel — just close overlay
                return true;
            }

            false
        }
    }

    let overlay = AuthTypeOverlay {
        items,
        selected_index,
        filtered_indices,
        signal: signal.clone(),
    };

    tui.show_positioned_overlay(Box::new(overlay), crate::tui::OverlayPosition::Bottom);
}

/// Show auth type selector or go directly to provider list depending on
/// which auth types are available. Matches pi's logic.
pub fn show_auth_type_or_provider_selector(app: &mut App, tui: &mut TUI) {
    let providers = app.registry.list_providers();
    if providers.is_empty() {
        app.status_text = Some("No providers available for login.".into());
        return;
    }
    // Check if any OAuth providers are registered (from OAuth registry)
    let has_oauth = !crate::provider::oauth::list_ids().is_empty();
    let has_api_key = providers.iter().any(|(_, _)| true);
    if has_oauth && has_api_key {
        show_auth_type_selector(app, tui);
    } else if has_oauth {
        show_login_provider_selector(app, tui, Some(AuthType::OAuth));
    } else {
        show_login_provider_selector(app, tui, Some(AuthType::ApiKey));
    }
}

/// Show the logout provider selector overlay.
/// Shows only providers with stored credentials (matching pi's getLogoutProviderOptions).
pub fn show_logout_provider_selector(app: &mut App, tui: &mut TUI) {
    use crate::agent::ui::components::oauth_selector::{
        AuthSelectorProvider, AuthType, OAuthSelector, SelectorMode,
    };

    // Get providers that have stored credentials
    let logged_in = auth::list_logged_in().unwrap_or_default();

    if logged_in.is_empty() {
        app.status_text = Some(
            "No stored credentials to remove. /logout only removes credentials saved by /login; \
             environment variables and models.json config are unchanged."
                .into(),
        );
        return;
    }

    let mut providers: Vec<AuthSelectorProvider> = logged_in
        .into_iter()
        .filter_map(|id| {
            app.registry
                .list_providers()
                .into_iter()
                .find(|(pid, _)| pid == &id)
                .map(|(pid, name)| AuthSelectorProvider {
                    id: pid,
                    name,
                    auth_type: AuthType::ApiKey,
                })
        })
        .collect();

    // Sort alphabetically by name for consistent display.
    providers.sort_by_key(|a| a.name.to_lowercase());

    if providers.is_empty() {
        // Providers with stored credentials may not be in registry anymore
        app.status_text = Some("No registered providers with stored credentials.".into());
        return;
    }

    let signal = app.overlay_result_signal.clone();
    let mut selector = OAuthSelector::new(
        providers,
        |provider_id| app.registry.auth_status_for_provider(provider_id),
        SelectorMode::Logout,
    );

    selector.on_select(move |provider_id: String| {
        *signal.borrow_mut() = Some(OverlayResult::LogoutProviderSelected(provider_id));
    });
    selector.on_cancel(|| {});

    tui.show_positioned_overlay(Box::new(selector), crate::tui::OverlayPosition::Bottom);
}

/// Post-login completion: auto-select default model for the provider.
/// Matches pi's completeProviderAuthentication logic for API key login.
pub(crate) fn complete_login(app: &mut App, provider_id: &str, _auth_type: AuthType) {
    // Try to select the default model for this provider
    let available_models = app.registry.list_model_provider_tuples();
    let provider_models: Vec<&str> = available_models
        .iter()
        .filter(|(pid, _, _)| pid == provider_id)
        .map(|(_, mid, _)| mid.as_str())
        .collect();

    if provider_models.is_empty() {
        app.status_text = Some(format!(
            "Saved API key for {provider_id}. No models available for this provider. Use /model to select a model."
        ));
        return;
    }

    // If current model is unknown or doesn't belong to this provider, select first available
    let current_provider = app
        .registry
        .provider_for_model(&app.model, Some(&app.current_provider))
        .unwrap_or_default();

    if current_provider != provider_id || !app.available_models.contains(&app.model) {
        let first_model = provider_models[0];
        app.model = first_model.to_string();
        app.current_provider = provider_id.to_string();
        let model = app.model.clone();
        app.record_model_change(&model);
        app.status_text = Some(format!(
            "Saved API key for {provider_id}. Selected {first_model}."
        ));
    } else {
        app.status_text = Some(format!("Saved API key for {provider_id}."));
    }
}
