//! Unified built-in extension providing all built-in tools and slash commands.
//!
//! Consolidates the former ReadExtension, WriteExtension, BashExtension,
//! EditExtension, and CommandsExtension into a single Extension implementation.

use crate::agent::extension::{Extension, SlashCommand, ToolDefinition};
use crate::builtin::bash::{BashToolOptions, make_bash_tool};
use crate::builtin::commands::make_commands;
use crate::builtin::edit::{EditOperations, make_edit_tool};
use crate::builtin::read::{ReadOperations, make_read_tool};
use crate::builtin::write::{WriteOperations, make_write_tool};

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Options for constructing a BuiltinExtension.
pub struct BuiltinOptions {
    pub cwd: PathBuf,
    pub read_operations: Arc<dyn ReadOperations>,
    pub write_operations: Arc<dyn WriteOperations>,
    pub edit_operations: Arc<dyn EditOperations>,
    pub bash_options: BashToolOptions,
}

impl Default for BuiltinOptions {
    fn default() -> Self {
        Self {
            cwd: PathBuf::from("."),
            read_operations: Arc::new(crate::builtin::read::DefaultReadOperations),
            write_operations: Arc::new(crate::builtin::write::DefaultWriteOperations),
            edit_operations: Arc::new(crate::builtin::edit::DefaultEditOperations),
            bash_options: BashToolOptions::default(),
        }
    }
}

/// Single extension that provides all built-in tools and slash commands.
pub struct BuiltinExtension {
    cwd: PathBuf,
    read_operations: Arc<dyn ReadOperations>,
    write_operations: Arc<dyn WriteOperations>,
    edit_operations: Arc<dyn EditOperations>,
    bash_options: BashToolOptions,
    available_models: Mutex<Vec<String>>,
    provider_models: Mutex<Vec<(String, String)>>,
}

impl BuiltinExtension {
    pub fn new(cwd: PathBuf) -> Self {
        Self::with_options(BuiltinOptions {
            cwd,
            ..BuiltinOptions::default()
        })
    }

    pub fn with_options(options: BuiltinOptions) -> Self {
        Self {
            cwd: options.cwd,
            read_operations: options.read_operations,
            write_operations: options.write_operations,
            edit_operations: options.edit_operations,
            bash_options: options.bash_options,
            available_models: Mutex::new(Vec::new()),
            provider_models: Mutex::new(Vec::new()),
        }
    }

    /// Set custom read operations (e.g. for SSH targets).
    pub fn with_read_operations(mut self, ops: Arc<dyn ReadOperations>) -> Self {
        self.read_operations = ops;
        self
    }

    /// Set custom write operations (e.g. for SSH targets).
    pub fn with_write_operations(mut self, ops: Arc<dyn WriteOperations>) -> Self {
        self.write_operations = ops;
        self
    }

    /// Set custom edit operations (e.g. for SSH targets).
    pub fn with_edit_operations(mut self, ops: Arc<dyn EditOperations>) -> Self {
        self.edit_operations = ops;
        self
    }

    /// Set bash tool options.
    pub fn with_bash_options(mut self, options: BashToolOptions) -> Self {
        self.bash_options = options;
        self
    }

    /// Set available models (builder style).
    pub fn with_available_models(self, models: Vec<String>) -> Self {
        if let Ok(mut guard) = self.available_models.lock() {
            *guard = models;
        }
        self
    }

    /// Set provider models (builder style).
    pub fn with_provider_models(self, models: Vec<(String, String)>) -> Self {
        if let Ok(mut guard) = self.provider_models.lock() {
            *guard = models;
        }
        self
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
}

impl Extension for BuiltinExtension {
    fn name(&self) -> Cow<'static, str> {
        "builtin".into()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn default_state(&self) -> crate::agent::ExtensionDefault {
        crate::agent::ExtensionDefault::Builtin
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            make_read_tool(self.cwd.clone(), self.read_operations.clone()),
            make_write_tool(self.cwd.clone(), self.write_operations.clone()),
            make_edit_tool(self.cwd.clone(), self.edit_operations.clone()),
            make_bash_tool(
                self.cwd.clone(),
                self.bash_options.shell_path.clone(),
                self.bash_options.command_prefix.clone(),
                self.bash_options.operations.clone(),
            ),
        ]
    }

    fn commands(&self) -> Vec<SlashCommand> {
        make_commands(&self.available_models, &self.provider_models)
    }
}
