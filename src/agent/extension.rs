/// Extension trait - all capability (built-in or user-provided) comes through this.
use crate::agent::types::ToolCall;
use crate::tui::Theme;
use async_trait::async_trait;
use std::borrow::Cow;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
/// Reason a tool call was blocked.
#[derive(Debug, Clone)]
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
    /// Reload settings, extensions, keybindings, themes from disk.
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
        user_messages: usize,
        assistant_messages: usize,
        tool_calls: usize,
        tool_results: usize,
        total_tokens: u64,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
        cost: f64,
    },
    /// Open session selector UI.
    OpenSessionSelector,
    /// Name was set for the session.
    SessionNamed { name: String },
    /// Open settings menu.
    OpenSettings,
    /// Enable/disable models for cycling.
    ScopedModels,
    /// Export session (HTML default, or specify path).
    ExportSession { path: Option<String> },
    /// Import and resume a session from a JSONL file.
    ImportSession { path: String },
    /// Share session as a secret GitHub gist.
    ShareSession,
    /// Copy last agent message to clipboard.
    CopyLastMessage,
    /// Show changelog entries.
    ShowChangelog,
    /// Create a new fork from a previous user message.
    ForkSession { message_id: Option<String> },
    /// Duplicate the current session at the current position.
    CloneSession,
    /// Navigate session tree (switch branches).
    SessionTree,
    /// Save project trust decision.
    TrustDecision { decision: String },
    /// Configure provider authentication.
    Login { provider: Option<String> },
    /// Remove provider authentication.
    Logout { provider: Option<String> },
    /// Manually compact the session context.
    CompactSession,
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
    /// When true, the agent loop stops after this batch of tool calls
    /// (no more LLM calls). Pi-compatible: `terminate` on tool results.
    pub terminate: bool,
    /// Structured rendering details for the UI (pi-compatible).
    /// Carries data that should NOT be sent to the LLM but IS needed by
    /// tool renderers (e.g. diff output, patch data).
    pub details: Option<serde_json::Value>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            compact: None,
            is_error: false,
            terminate: false,
            details: None,
        }
    }

    pub fn ok_with_compact(content: impl Into<String>, compact: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            compact: Some(compact.into()),
            is_error: false,
            terminate: false,
            details: None,
        }
    }

    /// Create an ok result with structured details for UI rendering (pi-compatible).
    /// The `content` is sent to the LLM; `details` is used by the tool renderer only.
    pub fn ok_with_details(
        content: impl Into<String>,
        details: impl Into<serde_json::Value>,
    ) -> Self {
        Self {
            content: content.into(),
            compact: None,
            is_error: false,
            terminate: false,
            details: Some(details.into()),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            compact: None,
            is_error: true,
            terminate: false,
            details: None,
        }
    }

    /// Mark this tool output as terminal - the agent loop will stop after
    /// this batch of tool calls when ALL tools in the batch return terminate=true.
    pub fn with_terminate(mut self, terminate: bool) -> Self {
        self.terminate = terminate;
        self
    }
}

/// Context passed to ToolRenderer methods (matching pi's ToolRenderContext).
/// Carries all metadata about the tool execution that renderers may need.
#[derive(Debug, Clone)]
pub struct ToolRenderContext {
    pub expanded: bool,
    pub args_complete: bool,
    pub is_partial: bool,
    pub is_error: bool,
    /// Working directory for path resolution.
    pub cwd: String,
    /// Duration in seconds (bash).
    pub duration_secs: Option<f64>,
    /// Exit code (bash).
    pub exit_code: Option<i32>,
    /// Whether execution was cancelled (bash).
    pub cancelled: bool,
    /// Whether output was truncated (bash/read).
    pub was_truncated: bool,
    /// Path to full output file (bash).
    pub full_output_path: Option<String>,
    /// File path for syntax highlighting (read).
    pub file_path: Option<String>,
    /// Keybinding hint for the expand action, e.g. "C-O".
    pub expand_key: String,
    /// Structured rendering details from the tool execution (pi-compatible).
    /// Set by tool renderers for preview/actual diff data. Not sent to the LLM.
    pub details: Option<serde_json::Value>,
    /// Callback for renderers to request re-render (e.g. after async preview computation).
    /// Pi-compatible: `context.invalidate()` in renderCall/renderResult.
    /// Cloned from the original at context construction time.
    /// Uses a channel sender internally to bridge from async to UI thread.
    pub invalidate: Option<tokio::sync::mpsc::UnboundedSender<()>>,
}

/// Tool-specific rendering interface (matching pi's renderCall/renderResult pattern).
/// Each built-in tool implements this to provide its own visual representation.
pub trait ToolRenderer: Send + Sync {
    /// Render the tool call header/title.
    /// Returns ANSI-styled lines for the call portion (inside the colored box shell).
    fn render_call(
        &self,
        args: &serde_json::Value,
        width: usize,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Vec<String>;

    /// Render the tool result body.
    /// Returns lines to display as the result body, or empty vec for no result.
    /// When empty, only the call portion is shown (e.g. write success).
    fn render_result(
        &self,
        content: &str,
        width: usize,
        theme: &dyn Theme,
        ctx: &ToolRenderContext,
    ) -> Vec<String>;

    /// Whether this tool uses `renderShell: "self"` (controls its own framing).
    /// When true, ToolExecComponent does NOT wrap the tool in a colored background box.
    fn render_self(&self) -> bool {
        false
    }

    /// Optional hint for the background color key when `render_self()` is false.
    /// Return a theme key name (e.g. "toolPendingBg", "toolSuccessBg", "toolErrorBg")
    /// to override the default background selection. Return None to let the
    /// ToolExecComponent decide based on is_complete/is_error state.
    /// Used by edit tool to show success/error bg during preview.
    fn render_bg_key(&self) -> Option<&'static str> {
        None
    }
}

#[async_trait]
pub trait Extension: Send + Sync {
    fn name(&self) -> Cow<'static, str>;

    /// Tools this extension provides (LLM-callable).
    fn tools(&self) -> Vec<Box<dyn yoagent::types::AgentTool>> {
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

    /// Tool-specific renderer for the TUI.
    fn tool_renderer(&self, _name: &str) -> Option<Box<dyn ToolRenderer>> {
        None
    }

    /// Tool prompt snippets for the "Available tools" section.
    fn tool_snippets(&self) -> Vec<(String, Cow<'static, str>)> {
        vec![]
    }

    /// Tool prompt guidelines for the system prompt.
    fn tool_guidelines(&self) -> Vec<(String, String)> {
        vec![]
    }
}
