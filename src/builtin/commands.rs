use crate::builtin::export::{ExportCommand, ImportCommand};
use crate::extension::{AutocompleteItem, CommandHandler, CommandResult, SlashCommand};
use std::sync::Mutex;

/// Build all built-in slash commands.
pub(crate) fn make_commands(
    available_models: &Mutex<Vec<String>>,
    provider_models: &Mutex<Vec<(String, String)>>,
) -> Vec<SlashCommand> {
    // Snapshot the current model list so changes from /reload are visible.
    let models = available_models
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let provider_models_snap = provider_models
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
                provider_models: provider_models_snap.clone(),
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
            description: "Export session (HTML default, or specify path: .html/.jsonl)".to_string(),
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
            handler: Box::new(NameCommand),
        },
        SlashCommand {
            name: "session".to_string(),
            description: "Show session info and stats".to_string(),
            handler: Box::new(SessionInfoCommand),
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
            description: "Reload keybindings, extensions, skills, prompts, and themes".to_string(),
            handler: Box::new(ReloadCommand),
        },
        SlashCommand {
            name: "quit".to_string(),
            description: "Exit rab".to_string(),
            handler: Box::new(QuitCommand),
        },
        SlashCommand {
            name: "stop".to_string(),
            description:
                "Graceful stop: finish current turn, then stop. Queued messages preserved."
                    .to_string(),
            handler: Box::new(StopCommand),
        },
        SlashCommand {
            name: "extensions".to_string(),
            description: "Enable/disable extensions".to_string(),
            handler: Box::new(ExtensionsCommand),
        },
        SlashCommand {
            name: "nextTurn".to_string(),
            description: "Queue a message for the next agent run (when idle)".to_string(),
            handler: Box::new(NextTurnCommand),
        },
    ]
}

// ── /quit ─────────────────────────────────────────────────────────

struct QuitCommand;

impl CommandHandler for QuitCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::Quit)
    }
}

// ── /stop ────────────────────────────────────────────────────────

struct StopCommand;

impl CommandHandler for StopCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::Stop)
    }
}

// ── /nextTurn ────────────────────────────────────────────────────

struct NextTurnCommand;

impl CommandHandler for NextTurnCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let text = args.trim().to_string();
        Ok(CommandResult::NextTurn { text })
    }
}

// ── /extensions ────────────────────────────────────────────────────

struct ExtensionsCommand;

impl CommandHandler for ExtensionsCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::OpenExtensions)
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
    provider_models: Vec<(String, String)>,
}

impl CommandHandler for ModelCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let input = args.trim();
        if input.is_empty() {
            Ok(CommandResult::OpenModelSelector)
        } else if let Some((provider, model_id)) = input.split_once('/') {
            let provider = provider.trim();
            let model_id = model_id.trim();
            if self
                .provider_models
                .iter()
                .any(|(p, m)| p == provider && m == model_id)
            {
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

/// Returns a sentinel CommandResult::SessionInfo with empty fields.
/// The actual data is filled in by app.rs from the live Session.
struct SessionInfoCommand;

impl CommandHandler for SessionInfoCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::SessionInfo {
            session_id: String::new(),
            file_path: None,
            name: None,
            message_count: 0,
            user_messages: 0,
            assistant_messages: 0,
            tool_calls: 0,
            tool_results: 0,
            total_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost: 0.0,
        })
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

struct NameCommand;

impl CommandHandler for NameCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let name = args.trim();
        if name.is_empty() {
            Ok(CommandResult::Info("Usage: /name <name>".to_string()))
        } else {
            Ok(CommandResult::SessionNamed {
                name: name.to_string(),
            })
        }
    }
}

// ── /login ────────────────────────────────────────────────────────

struct LoginCommand;

impl CommandHandler for LoginCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let args = args.trim();
        if args.is_empty() {
            return Ok(CommandResult::Login {
                provider: None,
                api_key: None,
            });
        }
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
