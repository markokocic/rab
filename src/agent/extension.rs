/// Extension trait - all capability (built-in or user-provided) comes through this.
use crate::agent::types::ToolCall;
use async_trait::async_trait;
use std::borrow::Cow;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::mpsc::UnboundedSender;

/// Reason a tool call was blocked.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum BlockReason {
    Security(String),
    Policy(String),
    Other(String),
}

/// An autocomplete item for slash command arguments.
#[derive(Debug, Clone)]
pub struct AutocompleteItem {
    /// The value to insert when selected.
    pub value: String,
    /// Display label.
    pub label: String,
    /// Optional description.
    pub description: Option<String>,
}

/// A slash command handler (built-in or extension-provided).
/// Commands use the same Extension trait as tools - built-ins and
/// user extensions register commands through a uniform interface.
pub trait CommandHandler: Send + Sync {
    /// Execute the command with the given arguments string.
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult>;

    /// Get argument completions for autocomplete.
    /// Called when user types `/cmd ` - returns matching autocomplete items.
    fn argument_completions(&self, _prefix: &str) -> Vec<AutocompleteItem> {
        vec![]
    }
}

/// Result of executing a slash command.
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Command handled, show this info message.
    Info(String),
    /// Command caused a quit request.
    Quit,
    /// Command switched the model (new model name).
    ModelChanged(String),
    /// Show keyboard shortcuts help overlay.
    ShowHelp,
    /// Reload settings and auth from disk.
    Reloaded,
    /// Start a new session (clear conversation).
    NewSession,
    /// Switch to a different session file.
    SessionSwitched { path: std::path::PathBuf },
    /// Show session info (ID, file, messages, tokens, cost).
    SessionInfo {
        session_id: String,
        file_path: Option<std::path::PathBuf>,
        name: Option<String>,
        message_count: usize,
    },
    /// Open session selector UI.
    OpenSessionSelector,
    /// Name was set for the session.
    SessionNamed { name: String },
}

/// A registered slash command.
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub handler: Box<dyn CommandHandler>,
}

/// Simple cancellation token for tool execution.
/// Shared between the agent loop and tool execution to signal cancellation.
#[derive(Debug, Clone)]
pub struct Cancel {
    flag: Arc<AtomicBool>,
}

impl Cancel {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    /// Request cancellation.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    /// Check if cancelled, returning an error if so.
    pub fn check(&self) -> anyhow::Result<()> {
        if self.is_cancelled() {
            Err(anyhow::anyhow!("Operation cancelled"))
        } else {
            Ok(())
        }
    }
}

impl Default for Cancel {
    fn default() -> Self {
        Self::new()
    }
}

/// Output from a tool execution, carrying both the full content (shown in expanded
/// mode / sent to the LLM) and an optional compact label for collapsed UI display.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Full content sent to the LLM and shown when expanded.
    pub content: String,
    /// Compact label shown in collapsed mode (e.g. `read docs docs/README.md`).
    /// When `None`, the full content is always shown.
    pub compact: Option<String>,
    /// Whether the result is an error.
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            compact: None,
            is_error: false,
        }
    }

    pub fn ok_with_compact(content: impl Into<String>, compact: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            compact: Some(compact.into()),
            is_error: false,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            compact: None,
            is_error: true,
        }
    }
}

/// An LLM-callable tool.
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;
    #[allow(dead_code)]
    fn label(&self) -> &str;

    /// Custom rendering for the tool call display.
    /// Returns ANSI-styled text for the tool call header (name + args).
    /// When None, a default rendering is used.
    fn render_call(&self) -> Option<fn(&serde_json::Value) -> String> {
        None
    }

    /// Custom rendering for the tool result display.
    /// Returns ANSI-styled text for the tool result body.
    /// When None, a default rendering is used.
    fn render_result(&self) -> Option<fn(&str, bool) -> String> {
        None
    }

    /// Guidelines for the system prompt specific to this tool.
    fn prompt_guidelines(&self) -> Vec<String> {
        vec![]
    }

    /// Execute the tool. Returns output carrying both the full content (sent to LLM)
    /// and an optional compact label for collapsed UI display.
    ///
    /// If `on_update` is provided, the tool may send intermediate `ToolOutput` updates
    /// during long-running operations (e.g. bash streaming).
    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
        cancel: Cancel,
        on_update: Option<UnboundedSender<ToolOutput>>,
    ) -> anyhow::Result<ToolOutput>;
}

#[async_trait]
#[allow(dead_code)]
pub trait Extension: Send + Sync {
    fn name(&self) -> Cow<'static, str>;

    /// Tools this extension provides (LLM-callable).
    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![]
    }

    /// Slash commands this extension provides (e.g. `/quit`, `/model`).
    /// Built-in commands and extension commands use the same interface.
    fn commands(&self) -> Vec<SlashCommand> {
        vec![]
    }

    /// Called before any tool executes. Return Some(reason) to block.
    async fn before_tool_call(&self, _tc: &ToolCall) -> Option<BlockReason> {
        None
    }

    /// Called after a tool executes. Return Some(text) to replace result.
    async fn after_tool_call(&self, _tc: &ToolCall, _result: &str) -> Option<String> {
        None
    }
}
