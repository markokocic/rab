use crate::agent::extension::{
    AutocompleteItem, CommandHandler, CommandResult, Extension, SlashCommand,
};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};

/// Built-in commands extension - provides all 22 pi slash commands.
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
                description: "Select model (opens selector UI)".to_string(),
                handler: Box::new(ModelCommand {
                    available_models: self.available_models.clone(),
                }),
            },
            SlashCommand {
                name: "settings".to_string(),
                description: "Open settings menu".to_string(),
                handler: Box::new(SettingsCommand),
            },
            SlashCommand {
                name: "scoped-models".to_string(),
                description: "Enable/disable models for cycling".to_string(),
                handler: Box::new(ScopedModelsCommand {
                    available_models: self.available_models.clone(),
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
                name: "share".to_string(),
                description: "Share session as a secret GitHub gist".to_string(),
                handler: Box::new(ShareCommand),
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
                handler: Box::new(SessionInfoCommand {
                    info: self.session_info.clone(),
                }),
            },
            SlashCommand {
                name: "changelog".to_string(),
                description: "Show changelog entries".to_string(),
                handler: Box::new(ChangelogCommand),
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
                name: "trust".to_string(),
                description: "Save project trust decision for future sessions".to_string(),
                handler: Box::new(TrustCommand),
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
            Ok(CommandResult::Info(format!(
                "Available models: {}\nUsage: /model <name>",
                self.available_models.join(", ")
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

// ── /export ───────────────────────────────────────────────────────

struct ExportCommand;

impl CommandHandler for ExportCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let path = args.trim();
        Ok(CommandResult::ExportSession {
            path: if path.is_empty() {
                None
            } else {
                Some(path.to_string())
            },
        })
    }
}

// ── /import ───────────────────────────────────────────────────────

struct ImportCommand;

impl CommandHandler for ImportCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let path = args.trim();
        if path.is_empty() {
            Ok(CommandResult::Info(
                "Usage: /import <path-to-jsonl> - import and resume a session from a JSONL file"
                    .to_string(),
            ))
        } else {
            Ok(CommandResult::ImportSession {
                path: path.to_string(),
            })
        }
    }
}

// ── /share ────────────────────────────────────────────────────────

struct ShareCommand;

impl CommandHandler for ShareCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::ShareSession)
    }
}

// ── /copy ─────────────────────────────────────────────────────────

struct CopyCommand;

impl CommandHandler for CopyCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::CopyLastMessage)
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

// ── /name ─────────────────────────────────────────────────────────

struct NameCommand;

impl CommandHandler for NameCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let name = args.trim();
        if name.is_empty() {
            return Ok(CommandResult::Info(
                "Usage: /name <name> - set session display name".to_string(),
            ));
        }
        Ok(CommandResult::SessionNamed {
            name: name.to_string(),
        })
    }
}

// ── /changelog ────────────────────────────────────────────────────

struct ChangelogCommand;

impl CommandHandler for ChangelogCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::ShowChangelog)
    }
}

// ── /fork ─────────────────────────────────────────────────────────

struct ForkCommand;

impl CommandHandler for ForkCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let msg = args.trim();
        Ok(CommandResult::ForkSession {
            message_id: if msg.is_empty() {
                None
            } else {
                Some(msg.to_string())
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

// ── /tree ─────────────────────────────────────────────────────────

struct TreeCommand;

impl CommandHandler for TreeCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::SessionTree)
    }
}

// ── /trust ────────────────────────────────────────────────────────

struct TrustCommand;

impl CommandHandler for TrustCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let decision = args.trim();
        if decision.is_empty() {
            Ok(CommandResult::Info(
                "Usage: /trust <auto|always|never> - save project trust decision for future sessions"
                    .to_string(),
            ))
        } else {
            Ok(CommandResult::TrustDecision {
                decision: decision.to_string(),
            })
        }
    }
}

// ── /login ────────────────────────────────────────────────────────

struct LoginCommand;

impl CommandHandler for LoginCommand {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult> {
        let provider = args.trim();
        Ok(CommandResult::Login {
            provider: if provider.is_empty() {
                None
            } else {
                Some(provider.to_string())
            },
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

// ── /compact ──────────────────────────────────────────────────────

struct CompactCommand;

impl CommandHandler for CompactCommand {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::CompactSession)
    }
}
