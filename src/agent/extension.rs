/// Extension trait - all capability (built-in or user-provided) comes through this.
use crate::tui::Theme;
use serde_json::Value;
use std::borrow::Cow;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A tool bundled with its prompt metadata.
///
/// Mirrors pi's `ToolDefinition` which carries `promptSnippet`,
/// `promptGuidelines` and `prepareArguments` directly on the tool definition.
pub struct ToolWithMeta {
    pub tool: Box<dyn yoagent::types::AgentTool>,
    /// One-line snippet for the "Available tools" section of the system prompt.
    pub snippet: &'static str,
    /// Guideline bullets for the "Guidelines" section of the system prompt.
    pub guidelines: &'static [&'static str],
    /// Optional pre-processing of raw LLM arguments before execute().
    /// Receives raw arguments, returns normalized arguments or an error message.
    pub prepare_arguments: Option<fn(serde_json::Value) -> Result<serde_json::Value, String>>,
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

/// Human-readable label for a JSON value's type.
fn json_type_label(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Recursively remove null-valued keys from objects.
/// Null is never valid data — removing it is equivalent to
/// omitting the key, which tools already handle gracefully.
fn strip_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let cleaned: serde_json::Map<String, Value> = map
                .into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect();
            Value::Object(cleaned)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(strip_nulls).collect()),
        other => other,
    }
}

#[async_trait::async_trait]
impl yoagent::types::AgentTool for ToolWithMeta {
    fn name(&self) -> &str {
        self.tool.name()
    }

    fn label(&self) -> &str {
        self.tool.label()
    }

    fn description(&self) -> &str {
        self.tool.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.tool.parameters_schema()
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: yoagent::types::ToolContext,
    ) -> std::result::Result<yoagent::types::ToolResult, yoagent::types::ToolError> {
        // 1. Shape guard: every tool expects an object at the root.
        if !params.is_object() {
            return Err(yoagent::types::ToolError::InvalidArgs(format!(
                "Expected object arguments for tool '{}', got {}",
                self.name(),
                json_type_label(&params),
            )));
        }

        // 2. Null stripping: null-valued optional keys are never valid.
        let params = strip_nulls(params);

        // 3. Per-tool argument normalization.
        let params = match self.prepare_arguments {
            Some(prepare) => prepare(params).map_err(yoagent::types::ToolError::InvalidArgs)?,
            None => params,
        };

        // 4. Delegate to inner tool.
        self.tool.execute(params, ctx).await
    }
}

pub trait Extension: Send + Sync {
    fn name(&self) -> Cow<'static, str>;

    /// Tools this extension provides (LLM-callable), each with its own prompt metadata.
    fn tools(&self) -> Vec<ToolWithMeta> {
        vec![]
    }

    /// Slash commands this extension provides (e.g. `/quit`, `/model`).
    /// Built-in commands and extension commands use the same interface.
    fn commands(&self) -> Vec<SlashCommand> {
        vec![]
    }

    /// Tool-specific renderer for the TUI.
    fn tool_renderer(&self, _name: &str) -> Option<Box<dyn ToolRenderer>> {
        None
    }

    /// Skills this extension provides (AgentSkills-compatible).
    /// Merged into the session's skill set for /skill:name expansion and system prompt.
    fn skills(&self) -> yoagent::skills::SkillSet {
        yoagent::skills::SkillSet::empty()
    }
}
