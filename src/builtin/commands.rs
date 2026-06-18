use crate::extension::{AutocompleteItem, CommandHandler, CommandResult, Extension, SlashCommand};
use std::borrow::Cow;

/// Built-in commands extension — provides /quit, /model, and other core commands.
/// Uses the same Extension trait as all other extensions, making built-in
/// commands indistinguishable from user-provided commands.
pub struct CommandsExtension {
    /// Available model identifiers (provider/id), e.g. ["deepseek-v4-flash", "deepseek-v4-pro"]
    available_models: Vec<String>,
}

impl CommandsExtension {
    pub fn new(available_models: Vec<String>) -> Self {
        Self { available_models }
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
