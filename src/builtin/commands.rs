use crate::extension::{AutocompleteItem, CommandHandler, CommandResult, Extension, SlashCommand};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};

/// Built-in commands extension — provides /quit, /model, and other core commands.
/// Uses the same Extension trait as all other extensions, making built-in
/// commands indistinguishable from user-provided commands.
pub struct CommandsExtension {
    /// Available model identifiers (provider/id), e.g. ["deepseek-v4-flash", "deepseek-v4-pro"]
    available_models: Vec<String>,
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
}

impl CommandsExtension {
    pub fn new(available_models: Vec<String>) -> Self {
        Self {
            available_models,
            session_info: Arc::new(Mutex::new(None)),
        }
    }

    /// Update the session info that /session will display.
    pub fn set_session_info(&self, info: SessionInfoInternal) {
        if let Ok(mut guard) = self.session_info.lock() {
            *guard = Some(info);
        }
    }
}

impl Extension for CommandsExtension {
    fn name(&self) -> Cow<'static, str> {
        "commands".into()
    }

    fn commands(&self) -> Vec<SlashCommand> {
        vec![
            SlashCommand {
                name: "quit".to_string(),
                description: "Exit rab".to_string(),
                handler: Box::new(QuitCommand),
            },
            SlashCommand {
                name: "model".to_string(),
                description: "Switch model".to_string(),
                handler: Box::new(ModelCommand {
                    available_models: self.available_models.clone(),
                }),
            },
            SlashCommand {
                name: "hotkeys".to_string(),
                description: "Show keyboard shortcuts".to_string(),
                handler: Box::new(HotkeysCommand),
            },
            SlashCommand {
                name: "reload".to_string(),
                description: "Reload settings and auth from disk".to_string(),
                handler: Box::new(ReloadCommand),
            },
            SlashCommand {
                name: "new".to_string(),
                description: "Start a new session (clear conversation)".to_string(),
                handler: Box::new(NewCommand),
            },
            SlashCommand {
                name: "resume".to_string(),
                description: "Resume a different session".to_string(),
                handler: Box::new(ResumeCommand),
            },
            SlashCommand {
                name: "session".to_string(),
                description: "Show session info".to_string(),
                handler: Box::new(SessionInfoCommand {
                    info: self.session_info.clone(),
                }),
            },
            SlashCommand {
                name: "name".to_string(),
                description: "Set session display name".to_string(),
                handler: Box::new(NameCommand),
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

// ── /model ────────────────────────────────────────────────────────

struct ModelCommand {
    available_models: Vec<String>,
}

impl CommandHandler for ModelCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let model = args.trim();
        if model.is_empty() {
            let list = self.available_models.join(", ");
            Ok(CommandResult::Info(format!(
                "Available models: {}\nUsage: /model <name>",
                list
            )))
        } else {
            // Validate model exists
            if self.available_models.iter().any(|m| m == model) {
                Ok(CommandResult::ModelChanged(model.to_string()))
            } else {
                Ok(CommandResult::Info(format!(
                    "Unknown model: {}. Available: {}",
                    model,
                    self.available_models.join(", ")
                )))
            }
        }
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

// ── /hotkeys ──────────────────────────────────────────────────────

struct HotkeysCommand;

impl CommandHandler for HotkeysCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::ShowHelp)
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

// ── /resume ───────────────────────────────────────────────────────

struct ResumeCommand;

impl CommandHandler for ResumeCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::OpenSessionSelector)
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
            }),
            None => Ok(CommandResult::Info(
                "No active session (use --no-session?)".to_string(),
            )),
        }
    }
}

// ── /name ─────────────────────────────────────────────────────────

struct NameCommand;

impl CommandHandler for NameCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let name = args.trim();
        if name.is_empty() {
            return Ok(CommandResult::Info(
                "Usage: /name <name> — set session display name".to_string(),
            ));
        }
        Ok(CommandResult::SessionNamed {
            name: name.to_string(),
        })
    }
}
