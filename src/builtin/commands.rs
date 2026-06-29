use crate::agent::extension::{
    AutocompleteItem, CommandHandler, CommandResult, Extension, SlashCommand,
};
use crate::agent::session::Session;
use crate::agent::types::{
    message_is_assistant, message_is_tool_result, message_is_user, message_tool_call_count,
    message_usage,
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
    let cache_read_tokens: u64 = 0;
    let mut cache_write_tokens: u64 = 0;
    let mut cost: f64 = 0.0;

    for entry in entries {
        if let super::super::agent::session::SessionEntry::Message(m) = entry {
            message_count += 1;
            if message_is_user(&m.message) {
                user_messages += 1;
            } else if message_is_assistant(&m.message) {
                assistant_messages += 1;
                let tc_count = message_tool_call_count(&m.message);
                tool_calls += tc_count;
            } else if message_is_tool_result(&m.message) {
                tool_results += 1;
            }
            if let Some(usage) = message_usage(&m.message) {
                let inp = usage.input;
                let outp = usage.output;
                let cache = usage.cache_read;
                input_tokens += inp;
                output_tokens += outp;
                total_tokens += inp + outp;
                cache_write_tokens += cache;
                // Rough cost estimate: $2/M input, $8/M output (deepseek pricing)
                cost += inp as f64 * 2.0 / 1_000_000.0 + outp as f64 * 8.0 / 1_000_000.0;
            }
        }
    }

    SessionInfoInternal {
        session_id: session.session_id(),
        file_path: session.session_file(),
        name: session.session_name(),
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

    fn commands(&self) -> Vec<SlashCommand> {
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
                    available_models: self.available_models.clone(),
                }),
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
                handler: Box::new(command_not_implemented_handler("export")),
            },
            SlashCommand {
                name: "import".to_string(),
                description: "Import and resume a session from a JSONL file".to_string(),
                handler: Box::new(command_not_implemented_handler("import")),
            },
            SlashCommand {
                name: "share".to_string(),
                description: "Share session as a secret GitHub gist".to_string(),
                handler: Box::new(command_not_implemented_handler("share")),
            },
            SlashCommand {
                name: "copy".to_string(),
                description: "Copy last agent message to clipboard".to_string(),
                handler: Box::new(command_not_implemented_handler("copy")),
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
                handler: Box::new(command_not_implemented_handler("changelog")),
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
                handler: Box::new(command_not_implemented_handler("clone")),
            },
            SlashCommand {
                name: "tree".to_string(),
                description: "Navigate session tree (switch branches)".to_string(),
                handler: Box::new(command_not_implemented_handler("tree")),
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
        let args = args.trim();
        if args.is_empty() {
            return Ok(CommandResult::Info(
                "Usage: /login <provider> [api-key]\n  Provide the API key as argument to store it.\n  If omitted, you'll be prompted.".into()
            ));
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

// ── Not Implemented handler factory ──────────────────────────────

struct NotImplementedHandler {
    name: String,
}

impl CommandHandler for NotImplementedHandler {
    fn execute(&self, _args: &str) -> anyhow::Result<CommandResult> {
        Ok(CommandResult::Info(format!(
            "/{} - not implemented yet.",
            self.name
        )))
    }
}

fn command_not_implemented_handler(name: &str) -> NotImplementedHandler {
    NotImplementedHandler {
        name: name.to_string(),
    }
}
