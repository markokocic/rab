use crate::agent::extension::{
    AutocompleteItem, CommandHandler, CommandResult, Extension, SlashCommand,
};
use crate::agent::session::Session;
use crate::agent::types::{
    message_is_assistant, message_is_tool_result, message_is_user, message_tool_call_count,
    message_usage,
};
use crate::builtin::export::{ExportCommand, ImportCommand};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use yoagent::types::AgentMessage;

/// Built-in commands extension - provides all 22 pi slash commands.
/// Uses the same Extension trait as all other extensions, making built-in
/// commands indistinguishable from user-provided commands.
pub struct CommandsExtension {
    /// Available model identifiers (e.g. "deepseek-v4-flash")
    /// Wrapped in Mutex so it can be updated via &self on /reload.
    available_models: std::sync::Mutex<Vec<String>>,
    /// Available (provider, model_id) pairs for "provider/model" completion.
    provider_models: std::sync::Mutex<Vec<(String, String)>>,
    /// Current session info for /session command.
    pub session_info: Arc<Mutex<Option<SessionInfoInternal>>>,
}

/// Session info passed to commands for display.
#[derive(Debug, Clone)]
pub struct SessionInfoInternal {
    pub session_id: String,
    pub file_path: Option<std::path::PathBuf>,
    pub name: Option<String>,
    pub message_count: usize,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost: f64,
}

impl CommandsExtension {
    pub fn new(available_models: Vec<String>, provider_models: Vec<(String, String)>) -> Self {
        Self {
            available_models: std::sync::Mutex::new(available_models),
            provider_models: std::sync::Mutex::new(provider_models),
            session_info: Arc::new(Mutex::new(None)),
        }
    }

    /// Update the set of available models (called on /reload after registry refresh).
    pub fn set_available_models(&self, models: Vec<String>) {
        if let Ok(mut guard) = self.available_models.lock() {
            *guard = models;
        }
    }

    /// Update the provider/model pairs (called on /reload after registry refresh).
    pub fn set_provider_models(&self, models: Vec<(String, String)>) {
        if let Ok(mut guard) = self.provider_models.lock() {
            *guard = models;
        }
    }

    /// Update the session info that /session will display.
    pub fn set_session_info(&self, info: SessionInfoInternal) {
        if let Ok(mut guard) = self.session_info.lock() {
            *guard = Some(info);
        }
    }
}

/// Compute session info from a Session.
pub fn compute_session_info(session: &Session) -> SessionInfoInternal {
    let entries = session.get_entries();
    let mut message_count: usize = 0;
    let mut user_messages: usize = 0;
    let mut assistant_messages: usize = 0;
    let mut tool_calls: usize = 0;
    let mut tool_results: usize = 0;
    let mut total_tokens: u64 = 0;
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_read_tokens: u64 = 0;
    let mut cache_write_tokens: u64 = 0;
    let mut cost: f64 = 0.0;

    for entry in entries {
        if let Some(llm_msg) = entry.message.as_llm() {
            message_count += 1;
            if message_is_user(&AgentMessage::Llm(llm_msg.clone())) {
                user_messages += 1;
            } else if message_is_assistant(&AgentMessage::Llm(llm_msg.clone())) {
                assistant_messages += 1;
                let tc_count = message_tool_call_count(&AgentMessage::Llm(llm_msg.clone()));
                tool_calls += tc_count;
            } else if message_is_tool_result(&AgentMessage::Llm(llm_msg.clone())) {
                tool_results += 1;
            }
            if let Some(usage) = message_usage(&AgentMessage::Llm(llm_msg.clone())) {
                input_tokens += usage.input;
                output_tokens += usage.output;
                cache_read_tokens += usage.cache_read;
                cache_write_tokens += usage.cache_write;
                total_tokens += usage.input + usage.output + usage.cache_read + usage.cache_write;
            }
            // Use pre-computed per-message cost (pi-style): computed at message
            // creation time via the model's cost config. Falls back to 0.0 for
            // sessions created before cost was stored.
            cost += session.entry_cost(&entry.id).map_or(0.0, |c| c.total);
        }
    }

    SessionInfoInternal {
        session_id: session.session_id().to_string(),
        file_path: session.session_file().map(|p| p.to_path_buf()),
        name: session.session_name().map(|s| s.to_string()),
        message_count,
        user_messages,
        assistant_messages,
        tool_calls,
        tool_results,
        total_tokens,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cost,
    }
}

impl Extension for CommandsExtension {
    fn name(&self) -> Cow<'static, str> {
        "commands".into()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn commands(&self) -> Vec<SlashCommand> {
        // Snapshot the current model list so changes from /reload are visible.
        let models = self
            .available_models
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let provider_models = self
            .provider_models
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        vec![
            SlashCommand {
                name: "settings".to_string(),
                description: "Open settings menu".to_string(),
                handler: Box::new(SettingsCommand),
            },
            SlashCommand {
                name: "model".to_string(),
                description: "Select model (opens selector UI)".to_string(),
                handler: Box::new(ModelCommand {
                    available_models: models.clone(),
                    provider_models: provider_models.clone(),
                }),
            },
            SlashCommand {
                name: "scoped-models".to_string(),
                description: "Enable/disable models for cycling".to_string(),
                handler: Box::new(ScopedModelsCommand {
                    available_models: models,
                }),
            },
            SlashCommand {
                name: "export".to_string(),
                description: "Export session (HTML default, or specify path: .html/.jsonl)"
                    .to_string(),
                handler: Box::new(ExportCommand),
            },
            SlashCommand {
                name: "import".to_string(),
                description: "Import and resume a session from a JSONL file".to_string(),
                handler: Box::new(ImportCommand),
            },
            SlashCommand {
                name: "copy".to_string(),
                description: "Copy last agent message to clipboard".to_string(),
                handler: Box::new(CopyCommand),
            },
            SlashCommand {
                name: "name".to_string(),
                description: "Set session display name".to_string(),
                handler: Box::new(NameCommand {
                    session_info: self.session_info.clone(),
                }),
            },
            SlashCommand {
                name: "session".to_string(),
                description: "Show session info and stats".to_string(),
                handler: Box::new(SessionInfoCommand {
                    info: self.session_info.clone(),
                }),
            },
            SlashCommand {
                name: "hotkeys".to_string(),
                description: "Show all keyboard shortcuts".to_string(),
                handler: Box::new(HotkeysCommand),
            },
            SlashCommand {
                name: "fork".to_string(),
                description: "Create a new fork from a previous user message".to_string(),
                handler: Box::new(ForkCommand),
            },
            SlashCommand {
                name: "clone".to_string(),
                description: "Duplicate the current session at the current position".to_string(),
                handler: Box::new(CloneCommand),
            },
            SlashCommand {
                name: "tree".to_string(),
                description: "Navigate session tree (switch branches)".to_string(),
                handler: Box::new(TreeCommand),
            },
            SlashCommand {
                name: "login".to_string(),
                description: "Configure provider authentication".to_string(),
                handler: Box::new(LoginCommand),
            },
            SlashCommand {
                name: "logout".to_string(),
                description: "Remove provider authentication".to_string(),
                handler: Box::new(LogoutCommand),
            },
            SlashCommand {
                name: "new".to_string(),
                description: "Start a new session".to_string(),
                handler: Box::new(NewCommand),
            },
            SlashCommand {
                name: "compact".to_string(),
                description: "Manually compact the session context".to_string(),
                handler: Box::new(CompactCommand),
            },
            SlashCommand {
                name: "resume".to_string(),
                description: "Resume a different session".to_string(),
                handler: Box::new(ResumeCommand),
            },
            SlashCommand {
                name: "reload".to_string(),
                description: "Reload keybindings, extensions, skills, prompts, and themes"
                    .to_string(),
                handler: Box::new(ReloadCommand),
            },
            SlashCommand {
                name: "quit".to_string(),
                description: "Exit rab".to_string(),
                handler: Box::new(QuitCommand),
            },
        ]
    }
}

// ── /quit ─────────────────────────────────────────────────────────

struct QuitCommand;

impl CommandHandler for QuitCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::Quit)
    }
}

// ── /resume ──────────────────────────────────────────────────────

struct ResumeCommand;

impl CommandHandler for ResumeCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::OpenSessionSelector)
    }
}

// ── /model ────────────────────────────────────────────────────────

struct ModelCommand {
    available_models: Vec<String>,
    provider_models: Vec<(String, String)>, // (provider, model_id) pairs
}

impl CommandHandler for ModelCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let input = args.trim();
        if input.is_empty() {
            Ok(CommandResult::OpenModelSelector)
        } else if let Some((provider, model_id)) = input.split_once('/') {
            // Handle "provider/model" format
            let provider = provider.trim();
            let model_id = model_id.trim();
            if self
                .provider_models
                .iter()
                .any(|(p, m)| p == provider && m == model_id)
            {
                // Return ModelChanged with "provider/model" so app can parse both
                Ok(CommandResult::ModelChanged(format!(
                    "{}/{}",
                    provider, model_id
                )))
            } else {
                Ok(CommandResult::Info(format!(
                    "Unknown provider/model: {}. Use 'provider/model' format or just model name",
                    input
                )))
            }
        } else if self.available_models.iter().any(|m| m == input) {
            // Plain model name - resolve provider automatically
            Ok(CommandResult::ModelChanged(input.to_string()))
        } else {
            Ok(CommandResult::Info(format!(
                "Unknown model: {}. Available: {}",
                input,
                self.available_models.join(", ")
            )))
        }
    }

    fn argument_completions(&self, prefix: &str) -> Vec<AutocompleteItem> {
        let lower = prefix.to_lowercase();
        let has_slash = prefix.contains('/');

        if has_slash {
            // Complete "provider/model" format
            self.provider_models
                .iter()
                .map(|(p, m)| format!("{}/{}", p, m))
                .filter(|full_id| full_id.to_lowercase().contains(&lower))
                .map(|full_id| AutocompleteItem {
                    value: full_id.clone(),
                    label: full_id,
                    description: None,
                })
                .collect()
        } else {
            // Complete by model name or "provider/" prefix
            let mut items: Vec<AutocompleteItem> = self
                .available_models
                .iter()
                .filter(|m| m.to_lowercase().contains(&lower))
                .map(|m| AutocompleteItem {
                    value: m.clone(),
                    label: m.clone(),
                    description: None,
                })
                .collect();
            // Also offer provider/ prefix completions
            let providers: std::collections::HashSet<String> = self
                .provider_models
                .iter()
                .map(|(p, _)| p.clone())
                .collect();
            for provider in &providers {
                if provider.to_lowercase().contains(&lower) {
                    items.push(AutocompleteItem {
                        value: format!("{}/", provider),
                        label: format!("{}/", provider),
                        description: None,
                    });
                }
            }
            items
        }
    }
}

// ── /settings ─────────────────────────────────────────────────────

struct SettingsCommand;

impl CommandHandler for SettingsCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::OpenSettings)
    }
}

// ── /scoped-models ────────────────────────────────────────────────

struct ScopedModelsCommand {
    available_models: Vec<String>,
}

impl CommandHandler for ScopedModelsCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::ScopedModels)
    }

    fn argument_completions(&self, prefix: &str) -> Vec<AutocompleteItem> {
        let lower = prefix.to_lowercase();
        self.available_models
            .iter()
            .filter(|m| m.to_lowercase().contains(&lower))
            .map(|m| AutocompleteItem {
                value: m.clone(),
                label: m.clone(),
                description: None,
            })
            .collect()
    }
}

// ── /reload ───────────────────────────────────────────────────────

struct ReloadCommand;

impl CommandHandler for ReloadCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::Reloaded)
    }
}

// ── /new ──────────────────────────────────────────────────────────

struct NewCommand;

impl CommandHandler for NewCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::NewSession)
    }
}

// ── /session ──────────────────────────────────────────────────────

struct SessionInfoCommand {
    info: Arc<Mutex<Option<SessionInfoInternal>>>,
}

impl CommandHandler for SessionInfoCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        let info = self.info.lock().unwrap();
        match info.as_ref() {
            Some(si) => Ok(CommandResult::SessionInfo {
                session_id: si.session_id.clone(),
                file_path: si.file_path.clone(),
                name: si.name.clone(),
                message_count: si.message_count,
                user_messages: si.user_messages,
                assistant_messages: si.assistant_messages,
                tool_calls: si.tool_calls,
                tool_results: si.tool_results,
                total_tokens: si.total_tokens,
                input_tokens: si.input_tokens,
                output_tokens: si.output_tokens,
                cache_read_tokens: si.cache_read_tokens,
                cache_write_tokens: si.cache_write_tokens,
                cost: si.cost,
            }),
            None => Ok(CommandResult::Info(
                "No active session (use --no-session?)".to_string(),
            )),
        }
    }
}

// ── /copy ────────────────────────────────────────────────────────

struct CopyCommand;

impl CommandHandler for CopyCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::CopyLastMessage)
    }
}

// ── /name ─────────────────────────────────────────────────────────

struct NameCommand {
    session_info: Arc<Mutex<Option<SessionInfoInternal>>>,
}

impl CommandHandler for NameCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let name = args.trim();
        if name.is_empty() {
            let info = self.session_info.lock().unwrap();
            let current_name = info
                .as_ref()
                .and_then(|si| si.name.as_deref())
                .filter(|n| !n.is_empty());
            return match current_name {
                Some(n) => Ok(CommandResult::Info(format!("Session name: {}", n))),
                None => Ok(CommandResult::Info("Usage: /name <name>".to_string())),
            };
        }
        Ok(CommandResult::SessionNamed {
            name: name.to_string(),
        })
    }
}

// ── /login ────────────────────────────────────────────────────────

struct LoginCommand;

impl CommandHandler for LoginCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let args = args.trim();
        if args.is_empty() {
            // No args — show provider selector
            return Ok(CommandResult::Login {
                provider: None,
                api_key: None,
            });
        }
        // Split on first space: provider [api-key]
        let (provider, api_key) = match args.split_once(' ') {
            Some((p, key)) => (p.trim().to_string(), Some(key.trim().to_string())),
            None => (args.to_string(), None),
        };
        Ok(CommandResult::Login {
            provider: Some(provider),
            api_key,
        })
    }
}

// ── /logout ───────────────────────────────────────────────────────

struct LogoutCommand;

impl CommandHandler for LogoutCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let provider = args.trim();
        Ok(CommandResult::Logout {
            provider: if provider.is_empty() {
                None
            } else {
                Some(provider.to_string())
            },
        })
    }
}

// ── /clone ────────────────────────────────────────────────────────

struct CloneCommand;

impl CommandHandler for CloneCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::CloneSession)
    }
}

// ── /hotkeys ──────────────────────────────────────────────────────

struct HotkeysCommand;

impl CommandHandler for HotkeysCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::ShowHelp)
    }
}

// ── /fork ─────────────────────────────────────────────────────────

struct ForkCommand;

impl CommandHandler for ForkCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let msg_id = args.trim();
        if msg_id.is_empty() {
            Ok(CommandResult::ForkSession { message_id: None })
        } else {
            Ok(CommandResult::ForkSession {
                message_id: Some(msg_id.to_string()),
            })
        }
    }
}

// ── /tree ────────────────────────────────────────────────────────

struct TreeCommand;

impl CommandHandler for TreeCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::SessionTree)
    }
}

// ── /compact ──────────────────────────────────────────────────────

struct CompactCommand;

impl CommandHandler for CompactCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let custom_instructions = if args.trim().is_empty() {
            None
        } else {
            Some(args.trim().to_string())
        };
        Ok(CommandResult::CompactSession(custom_instructions))
    }
}
