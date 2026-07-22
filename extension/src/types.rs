//! Core types shared across the extension system.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::coerce::{coerce_with_json_schema, validate_tool_arguments};
use crate::hooks::{run_after_hooks, run_before_hooks};
use crate::traits::ToolRenderer;

// ── Tool hook types ────────────────────────────────────────────

/// A before-tool-call hook: receives parsed/validated arguments, returns
/// optionally blocks execution. Matching pi's `beforeToolCall`.
pub type BeforeHook = Arc<dyn Fn(&serde_json::Value) -> Option<BeforeToolCallResult> + Send + Sync>;

/// An after-tool-call hook: receives the result and error flag, returns
/// optional overrides. Matching pi's `afterToolCall`.
pub type AfterHook =
    Arc<dyn Fn(&yoagent::types::ToolResult, bool) -> Option<AfterToolCallResult> + Send + Sync>;

/// A tool hook registration: pairs a tool name with optional before/after hooks.
pub struct HookRegistration {
    pub tool_name: &'static str,
    pub before: Option<BeforeHook>,
    pub after: Option<AfterHook>,
}

/// Result returned from `before_tool_call` (matching pi's `BeforeToolCallResult`).
pub struct BeforeToolCallResult {
    pub block: bool,
    pub reason: String,
}

/// Partial override returned from `after_tool_call` (matching pi's `AfterToolCallResult`).
pub struct AfterToolCallResult {
    pub content: Option<Vec<yoagent::types::Content>>,
    pub details: Option<serde_json::Value>,
    pub is_error: Option<bool>,
}

// ── Tool definition ─────────────────────────────────────────────

/// A tool bundled with its prompt metadata.
///
/// Mirrors pi's `ToolDefinition` which carries `promptSnippet`,
/// `promptGuidelines` and `prepareArguments` directly on the tool definition.
pub struct ToolDefinition {
    pub tool: Box<dyn yoagent::types::AgentTool>,
    /// One-line snippet for the "Available tools" section of the system prompt.
    pub snippet: &'static str,
    /// Guideline bullets for the "Guidelines" section of the system prompt.
    pub guidelines: &'static [&'static str],
    /// Optional pre-processing of raw LLM arguments before execute().
    pub prepare_arguments: Option<fn(serde_json::Value) -> Result<serde_json::Value, String>>,
    /// Called before tool execution, after argument validation.
    pub before_tool_call: Option<fn(&serde_json::Value) -> Option<BeforeToolCallResult>>,
    /// Called after tool execution, before the result is returned.
    pub after_tool_call:
        Option<fn(&yoagent::types::ToolResult, bool) -> Option<AfterToolCallResult>>,
    /// Tool-specific renderer for the TUI.
    pub renderer: Option<Arc<dyn ToolRenderer>>,
}

#[async_trait::async_trait]
impl yoagent::types::AgentTool for ToolDefinition {
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
        let mut params = match self.prepare_arguments {
            Some(prepare) => prepare(params).map_err(yoagent::types::ToolError::InvalidArgs)?,
            None => params,
        };
        // Step 1: type coercion
        let schema = self.tool.parameters_schema();
        coerce_with_json_schema(&schema, &mut params);

        // Step 2: validate against schema
        let tool_name = self.tool.name();
        validate_tool_arguments(tool_name, &schema, &params)
            .map_err(yoagent::types::ToolError::InvalidArgs)?;

        // Step 3: before_tool_call hook
        if let Some(ref hook) = self.before_tool_call
            && let Some(result) = hook(&params)
            && result.block
        {
            let reason = if result.reason.is_empty() {
                format!("Tool {} execution blocked", tool_name)
            } else {
                result.reason
            };
            return Err(yoagent::types::ToolError::Failed(reason));
        }

        // Step 3b: extension before-execution hooks
        if let Some(result) = run_before_hooks(tool_name, &params)
            && result.block
        {
            let reason = if result.reason.is_empty() {
                format!("Tool {} execution blocked", tool_name)
            } else {
                result.reason
            };
            return Err(yoagent::types::ToolError::Failed(reason));
        }

        // Step 4: execute the inner tool
        let (mut tool_result, mut is_error) = match self.tool.execute(params, ctx).await {
            Ok(r) => (r, false),
            Err(e) => {
                let err_text = e.to_string();
                (
                    yoagent::types::ToolResult {
                        content: vec![yoagent::types::Content::Text { text: err_text }],
                        details: serde_json::Value::Null,
                    },
                    true,
                )
            }
        };

        // Step 5: after_tool_call hook
        if let Some(ref hook) = self.after_tool_call
            && let Some(override_result) = hook(&tool_result, is_error)
        {
            if let Some(content) = override_result.content {
                tool_result.content = content;
            }
            if let Some(details) = override_result.details {
                tool_result.details = details;
            }
            if let Some(err) = override_result.is_error {
                is_error = err;
            }
        }

        // Step 5b: extension after-execution hooks
        if let Some(override_result) = run_after_hooks(tool_name, &tool_result, is_error) {
            if let Some(content) = override_result.content {
                tool_result.content = content;
            }
            if let Some(details) = override_result.details {
                tool_result.details = details;
            }
            if let Some(err) = override_result.is_error {
                is_error = err;
            }
        }

        if is_error {
            let error_text: String = tool_result
                .content
                .iter()
                .filter_map(|c| {
                    if let yoagent::types::Content::Text { text } = c {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            Err(yoagent::types::ToolError::Failed(error_text))
        } else {
            Ok(tool_result)
        }
    }
}

// ── Slash commands ─────────────────────────────────────────────

/// An autocomplete item for slash command arguments.
#[derive(Debug, Clone)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// A slash command handler (built-in or extension-provided).
pub trait CommandHandler: Send + Sync {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult>;

    fn argument_completions(&self, _prefix: &str) -> Vec<AutocompleteItem> {
        vec![]
    }
}

/// Result of executing a slash command.
#[derive(Debug, Clone)]
pub enum CommandResult {
    Info(String),
    Quit,
    ModelChanged(String),
    ShowHelp,
    Reloaded,
    NewSession,
    SessionSwitched {
        path: std::path::PathBuf,
    },
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
    OpenSessionSelector,
    SessionNamed {
        name: String,
    },
    OpenSettings,
    OpenExtensions,
    OpenModelSelector,
    ScopedModels,
    ExportSession {
        path: Option<String>,
    },
    ImportSession {
        path: String,
    },
    ShareSession,
    CopyLastMessage,
    ShowChangelog,
    ForkSession {
        message_id: Option<String>,
    },
    CloneSession,
    SessionTree,
    TrustDecision {
        decision: String,
    },
    Login {
        provider: Option<String>,
        api_key: Option<String>,
    },
    Logout {
        provider: Option<String>,
    },
    CompactSession(Option<String>),
    Stop,
    NextTurn {
        text: String,
    },
}

/// A registered slash command.
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub handler: Box<dyn CommandHandler>,
}

// ── Cancellation token ─────────────────────────────────────────

/// Simple cancellation token for tool execution.
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

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

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

// ── Tool render context ────────────────────────────────────────

/// Context passed to ToolRenderer methods (matching pi's ToolRenderContext).
#[derive(Debug, Clone)]
pub struct ToolRenderContext {
    pub expanded: bool,
    pub args_complete: bool,
    pub is_partial: bool,
    pub is_error: bool,
    pub tool_call_id: String,
    pub execution_started: bool,
    pub cwd: String,
    pub duration_secs: Option<f64>,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub was_truncated: bool,
    pub full_output_path: Option<String>,
    pub file_path: Option<String>,
    pub expand_key: String,
    pub details: Option<serde_json::Value>,
    pub state: std::rc::Rc<std::cell::RefCell<serde_json::Value>>,
    pub invalidate: Option<tokio::sync::mpsc::UnboundedSender<()>>,
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cancel_new_not_cancelled() {
        let cancel = Cancel::new();
        assert!(!cancel.is_cancelled());
        cancel.check().unwrap();
    }

    #[test]
    fn test_cancel_after_cancel() {
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(cancel.is_cancelled());
        assert!(cancel.check().is_err());
    }

    #[test]
    fn test_cancel_default_not_cancelled() {
        let cancel = Cancel::default();
        assert!(!cancel.is_cancelled());
    }

    #[test]
    fn test_cancel_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<Cancel>();
        assert_sync::<Cancel>();
    }
}

// ── Tests for ToolDefinition AgentTool impl ────────────────────

#[cfg(test)]
mod tool_impl_tests {
    use crate::coerce::coerce_primitive_by_type;

    #[test]
    fn test_tool_definition_coerce_delegation() {
        let mut v = serde_json::json!(42);
        coerce_primitive_by_type("string", &mut v);
        assert_eq!(v, serde_json::json!("42"));
    }
}
