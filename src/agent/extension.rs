/// Extension trait - all capability (built-in or user-provided) comes through this.
use crate::tui::Theme;
use std::borrow::Cow;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

// ── Tool call hooks (matching pi's beforeToolCall / afterToolCall) ──

/// Result returned from `before_tool_call` (matching pi's `BeforeToolCallResult`).
/// Returning `{ block: true }` prevents execution; `reason` becomes the error text.
pub struct BeforeToolCallResult {
    /// If true, the tool execution is blocked.
    pub block: bool,
    /// Error message shown when `block` is true. If empty, a default message is used.
    pub reason: String,
}

/// Partial override returned from `after_tool_call` (matching pi's `AfterToolCallResult`).
/// Merge semantics are field-by-field: provided fields replace the original; omitted fields keep their values.
pub struct AfterToolCallResult {
    /// If provided, replaces the tool result content array in full.
    pub content: Option<Vec<yoagent::types::Content>>,
    /// If provided, replaces the tool result details value in full.
    pub details: Option<serde_json::Value>,
    /// If provided, replaces the tool result error flag.
    pub is_error: Option<bool>,
}

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
    /// Called before tool execution, after argument validation (matching pi's `beforeToolCall`).
    /// Return `Some(BeforeToolCallResult { block: true, reason: "..." })` to block execution.
    pub before_tool_call: Option<fn(&serde_json::Value) -> Option<BeforeToolCallResult>>,
    /// Called after tool execution, before the result is returned (matching pi's `afterToolCall`).
    pub after_tool_call:
        Option<fn(&yoagent::types::ToolResult, bool) -> Option<AfterToolCallResult>>,
}

// ── Generic argument type coercion & validation ─────────────────

/// Coerce a single JSON value to match a JSON Schema type (modifies in place).
/// This handles common LLM mistakes: sending numbers as strings, booleans as strings, etc.
pub fn coerce_primitive_by_type(schema_type: &str, value: &mut serde_json::Value) {
    match schema_type {
        "string" => {
            if value.is_number() || value.is_boolean() {
                *value = serde_json::Value::String(match value {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => unreachable!(),
                });
            } else if value.is_null() {
                *value = serde_json::Value::String(String::new());
            }
        }
        "number" => {
            if let Some(s) = value.as_str() {
                if let Ok(n) = s.parse::<f64>() {
                    *value = serde_json::json!(n);
                }
            } else if value.is_boolean() {
                *value = serde_json::json!(if value.as_bool().unwrap() { 1.0 } else { 0.0 });
            } else if value.is_null() {
                *value = serde_json::json!(0.0);
            }
        }
        "integer" => {
            if let Some(s) = value.as_str() {
                if let Ok(n) = s.parse::<f64>() {
                    *value = serde_json::json!(n as i64);
                }
            } else if value.is_boolean() {
                *value = serde_json::json!(if value.as_bool().unwrap() { 1i64 } else { 0i64 });
            } else if value.is_null() {
                *value = serde_json::json!(0i64);
            } else if let Some(n) = value.as_f64() {
                *value = serde_json::json!(n as i64);
            }
        }
        "boolean" => {
            if let Some(s) = value.as_str() {
                match s.trim().to_lowercase().as_str() {
                    "true" | "1" | "yes" | "on" => *value = serde_json::Value::Bool(true),
                    "false" | "0" | "no" | "off" => *value = serde_json::Value::Bool(false),
                    _ => {} // Leave as-is if unrecognized
                }
            } else if value.is_number() {
                *value = serde_json::Value::Bool(value.as_f64().unwrap_or(0.0) != 0.0);
            } else if value.is_null() {
                *value = serde_json::Value::Bool(false);
            }
        }
        "array" => {
            if !value.is_array() && !value.is_null() {
                let v = std::mem::take(value);
                *value = serde_json::Value::Array(vec![v]);
            } else if value.is_null() {
                *value = serde_json::Value::Array(vec![]);
            }
        }
        _ => {}
    }
}

/// Recursively coerce tool arguments to match a JSON Schema (modifies in place).
pub fn coerce_with_json_schema(schema: &serde_json::Value, args: &mut serde_json::Value) {
    if !args.is_object() {
        return;
    }
    let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
        return;
    };
    for (key, prop_schema) in properties {
        if args.get(key).is_none() {
            continue;
        }
        let type_str = match resolve_schema_type(prop_schema) {
            Some(t) => t,
            None => continue,
        };
        let arg_value = args.get_mut(key).unwrap();
        coerce_primitive_by_type(type_str, arg_value);
        if type_str == "object" {
            coerce_with_json_schema(prop_schema, arg_value);
        }
        if type_str == "array"
            && let Some(items_schema) = prop_schema.get("items")
            && let Some(arr) = arg_value.as_array_mut()
        {
            for item in arr.iter_mut() {
                coerce_with_json_schema(items_schema, item);
            }
        }
    }
}

// ── Schema validation (matching pi's validateToolArguments) ──────

/// Extracts the effective JSON Schema type from a property schema.
/// Returns `None` when the schema has no recognizable type.
fn resolve_schema_type(schema: &serde_json::Value) -> Option<&str> {
    let type_val = schema.get("type")?;
    if type_val.is_string() {
        return type_val.as_str();
    }
    if type_val.is_array() {
        // Use the first non-null type (handles ["string", "null"])
        return type_val
            .as_array()
            .and_then(|arr| arr.iter().find_map(|t| t.as_str().filter(|&s| s != "null")));
    }
    None
}

/// Check whether a JSON value matches a JSON Schema type.
fn matches_json_type(value: &serde_json::Value, schema_type: &str) -> bool {
    match schema_type {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true, // unknown type — don't reject
    }
}

/// Check whether a value matches at least one of the schema's types (handles ["string", "null"]).
fn value_matches_schema_types(schema: &serde_json::Value, value: &serde_json::Value) -> bool {
    let type_val = match schema.get("type") {
        Some(t) => t,
        None => return true,
    };
    if type_val.is_string() {
        return matches_json_type(value, type_val.as_str().unwrap());
    }
    if let Some(types) = type_val.as_array() {
        return types
            .iter()
            .filter_map(|t| t.as_str())
            .any(|t| matches_json_type(value, t));
    }
    true
}

/// Recursively collect validation errors for a value against a JSON Schema.
/// Path format matches pi's formatValidationPath: "root", "edits", "edits.0.oldText".
fn collect_validation_errors(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
    errors: &mut Vec<ValidationError>,
) {
    // Root must be an object — every tool schema is "type": "object"
    if (path.is_empty() || path == "root")
        && let Some(schema_type) = resolve_schema_type(schema)
        && schema_type == "object"
        && !value.is_object()
    {
        errors.push(ValidationError {
            path: path.to_string(),
            message: "Expected object".to_string(),
        });
        return;
    }

    // Not an object — only check type (won't recurse)
    if !value.is_object()
        && let Some(schema_type) = resolve_schema_type(schema)
        && !matches_json_type(value, schema_type)
    {
        let expected = if schema_type == "integer" {
            "integer"
        } else {
            schema_type
        };
        errors.push(ValidationError {
            path: path.to_string(),
            message: format!("Expected {}", expected),
        });
        return;
    }

    if !value.is_object() {
        return;
    }

    let obj = value.as_object().unwrap();
    let properties = schema.get("properties").and_then(|p| p.as_object());
    let known_keys: std::collections::HashSet<&str> = properties
        .map(|p| p.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();

    // Check required properties
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for required_val in required {
            if let Some(required_key) = required_val.as_str()
                && !obj.contains_key(required_key)
            {
                let err_path = if path.is_empty() || path == "root" {
                    required_key.to_string()
                } else {
                    format!("{}.{}", path, required_key)
                };
                errors.push(ValidationError {
                    path: err_path,
                    message: "Required".to_string(),
                });
            }
        }
    }

    // Check additionalProperties
    if schema.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
        for key in obj.keys() {
            if !known_keys.contains(key.as_str()) {
                let err_path = if path.is_empty() || path == "root" {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                errors.push(ValidationError {
                    path: err_path,
                    message: "must NOT have additional properties".to_string(),
                });
            }
        }
    }

    // Validate each property
    if let Some(props) = properties {
        for (key, prop_schema) in props {
            if let Some(val) = value.get(key) {
                let child_path = if path.is_empty() || path == "root" {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                validate_property(prop_schema, val, &child_path, errors);
            }
        }
    }
}

/// Validate a single property value against its schema, recursing into objects/arrays.
fn validate_property(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
    errors: &mut Vec<ValidationError>,
) {
    // Check type match
    if !value_matches_schema_types(schema, value) {
        let schema_type = resolve_schema_type(schema).unwrap_or("unknown");
        let expected = if schema_type == "integer" {
            "integer"
        } else {
            schema_type
        };
        errors.push(ValidationError {
            path: path.to_string(),
            message: format!("Expected {}", expected),
        });
        return; // Don't recurse into wrong-typed values
    }

    // Recurse into objects
    if value.is_object() {
        // Only recurse if the schema also describes an object
        let schema_type = resolve_schema_type(schema);
        if schema_type == Some("object") {
            collect_validation_errors(schema, value, path, errors);
        }
        return;
    }

    // Recurse into array items
    if let Some(arr) = value.as_array()
        && resolve_schema_type(schema) == Some("array")
        && let Some(items_schema) = schema.get("items")
    {
        for (i, item) in arr.iter().enumerate() {
            let item_path = format!("{}.{}", path, i);
            validate_property(items_schema, item, &item_path, errors);
        }
    }
}

/// A single validation error, matching pi's TypeBox error structure.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Path to the field, e.g. "edits.0.oldText" or "root"
    pub path: String,
    /// Error message, e.g. "Required" or "must NOT have additional properties"
    pub message: String,
}

/// Validate tool arguments against its JSON Schema (matching pi's validateToolArguments).
///
/// Returns `Ok(())` on success, or `Err` with pi-compatible format:
/// ```text
/// Validation failed for tool "edit":
///   - path: Required
///   - edits[0].oldText: Required
///
/// Received arguments:
/// {
///   "path": "/foo.txt"
/// }
/// ```
pub fn validate_tool_arguments(
    tool_name: &str,
    schema: &serde_json::Value,
    args: &serde_json::Value,
) -> Result<(), String> {
    let mut errors: Vec<ValidationError> = Vec::new();
    collect_validation_errors(schema, args, "root", &mut errors);

    if errors.is_empty() {
        return Ok(());
    }

    let error_lines: Vec<String> = errors
        .iter()
        .map(|e| format!("  - {}: {}", e.path, e.message))
        .collect();

    let pretty_args =
        serde_json::to_string_pretty(args).unwrap_or_else(|_| "<unprintable>".to_string());

    Err(format!(
        "Validation failed for tool \"{tool_name}\":\n{}\n\nReceived arguments:\n{pretty_args}",
        error_lines.join("\n"),
    ))
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
        let mut params = match self.prepare_arguments {
            Some(prepare) => prepare(params).map_err(yoagent::types::ToolError::InvalidArgs)?,
            None => params,
        };
        // Step 1: type coercion (matching pi's Value.Convert + coerceWithJsonSchema)
        let schema = self.tool.parameters_schema();
        coerce_with_json_schema(&schema, &mut params);

        // Step 2: validate against schema (matching pi's validateToolArguments)
        let tool_name = self.tool.name();
        validate_tool_arguments(tool_name, &schema, &params)
            .map_err(yoagent::types::ToolError::InvalidArgs)?;

        // Step 3: before_tool_call hook (matching pi's beforeToolCall)
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

        // Step 5: after_tool_call hook (matching pi's afterToolCall)
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

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── coerce_primitive_by_type ────────────────────────────────────

    #[test]
    fn test_coerce_string_from_number() {
        let mut v = serde_json::json!(42);
        coerce_primitive_by_type("string", &mut v);
        assert_eq!(v, serde_json::json!("42"));
    }

    #[test]
    fn test_coerce_string_from_boolean() {
        let mut v = serde_json::json!(true);
        coerce_primitive_by_type("string", &mut v);
        assert_eq!(v, serde_json::json!("true"));
    }

    #[test]
    fn test_coerce_string_from_null() {
        let mut v = serde_json::json!(null);
        coerce_primitive_by_type("string", &mut v);
        assert_eq!(v, serde_json::json!(""));
    }

    #[test]
    fn test_coerce_string_unchanged() {
        let mut v = serde_json::json!("hello");
        coerce_primitive_by_type("string", &mut v);
        assert_eq!(v, serde_json::json!("hello"));
    }

    #[test]
    fn test_coerce_number_from_string() {
        let mut v = serde_json::json!("3.14");
        coerce_primitive_by_type("number", &mut v);
        assert_eq!(v, serde_json::json!(3.14));
    }

    #[test]
    fn test_coerce_number_from_boolean() {
        let mut v = serde_json::json!(true);
        coerce_primitive_by_type("number", &mut v);
        assert_eq!(v, serde_json::json!(1.0));
    }

    #[test]
    fn test_coerce_number_from_null() {
        let mut v = serde_json::json!(null);
        coerce_primitive_by_type("number", &mut v);
        assert_eq!(v, serde_json::json!(0.0));
    }

    #[test]
    fn test_coerce_integer_from_string() {
        let mut v = serde_json::json!("7");
        coerce_primitive_by_type("integer", &mut v);
        assert_eq!(v, serde_json::json!(7i64));
    }

    #[test]
    fn test_coerce_integer_from_float() {
        let mut v = serde_json::json!(3.9);
        coerce_primitive_by_type("integer", &mut v);
        assert_eq!(v, serde_json::json!(3i64));
    }

    #[test]
    fn test_coerce_integer_from_boolean() {
        let mut v = serde_json::json!(false);
        coerce_primitive_by_type("integer", &mut v);
        assert_eq!(v, serde_json::json!(0i64));
    }

    #[test]
    fn test_coerce_boolean_from_string_true() {
        let mut v = serde_json::json!("true");
        coerce_primitive_by_type("boolean", &mut v);
        assert_eq!(v, serde_json::json!(true));
    }

    #[test]
    fn test_coerce_boolean_from_string_yes() {
        let mut v = serde_json::json!("yes");
        coerce_primitive_by_type("boolean", &mut v);
        assert_eq!(v, serde_json::json!(true));
    }

    #[test]
    fn test_coerce_boolean_from_number() {
        let mut v = serde_json::json!(1);
        coerce_primitive_by_type("boolean", &mut v);
        assert_eq!(v, serde_json::json!(true));
    }

    #[test]
    fn test_coerce_boolean_from_null() {
        let mut v = serde_json::json!(null);
        coerce_primitive_by_type("boolean", &mut v);
        assert_eq!(v, serde_json::json!(false));
    }

    #[test]
    fn test_coerce_array_from_scalar() {
        let mut v = serde_json::json!("single");
        coerce_primitive_by_type("array", &mut v);
        assert_eq!(v, serde_json::json!(["single"]));
    }

    #[test]
    fn test_coerce_array_from_null() {
        let mut v = serde_json::json!(null);
        coerce_primitive_by_type("array", &mut v);
        assert_eq!(v, serde_json::json!([]));
    }

    #[test]
    fn test_coerce_array_unchanged() {
        let mut v = serde_json::json!([1, 2, 3]);
        coerce_primitive_by_type("array", &mut v);
        assert_eq!(v, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn test_coerce_unknown_type_does_nothing() {
        let mut v = serde_json::json!(42);
        coerce_primitive_by_type("widget", &mut v);
        assert_eq!(v, serde_json::json!(42));
    }

    // ── coerce_with_json_schema ─────────────────────────────────────

    #[test]
    fn test_coerce_schema_string_from_number() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            }
        });
        let mut args = serde_json::json!({"name": 42});
        coerce_with_json_schema(&schema, &mut args);
        assert_eq!(args, serde_json::json!({"name": "42"}));
    }

    #[test]
    fn test_coerce_schema_nested_object() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "metadata": {
                    "type": "object",
                    "properties": {
                        "count": {"type": "integer"}
                    }
                }
            }
        });
        let mut args = serde_json::json!({"metadata": {"count": "5"}});
        coerce_with_json_schema(&schema, &mut args);
        assert_eq!(args, serde_json::json!({"metadata": {"count": 5i64}}));
    }

    #[test]
    fn test_coerce_schema_array_items() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "integer"}
                        }
                    }
                }
            }
        });
        let mut args = serde_json::json!({"items": [{"id": "3"}, {"id": "7"}]});
        coerce_with_json_schema(&schema, &mut args);
        assert_eq!(
            args,
            serde_json::json!({"items": [{"id": 3i64}, {"id": 7i64}]})
        );
    }

    #[test]
    fn test_coerce_schema_non_object_skipped() {
        let schema = serde_json::json!({"type": "string"});
        let mut args = serde_json::json!("hello");
        coerce_with_json_schema(&schema, &mut args);
        assert_eq!(args, serde_json::json!("hello"));
    }

    // ── validate_tool_arguments ─────────────────────────────────────

    #[test]
    fn test_validate_valid_args() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        });
        let args = serde_json::json!({"path": "/tmp/foo.txt"});
        assert!(validate_tool_arguments("test", &schema, &args).is_ok());
    }

    #[test]
    fn test_validate_missing_required() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        });
        let args = serde_json::json!({});
        let err = validate_tool_arguments("test", &schema, &args).unwrap_err();
        assert!(err.contains("Required"));
        assert!(err.contains("test"));
    }

    #[test]
    fn test_validate_wrong_type() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer"}
            }
        });
        let args = serde_json::json!({"count": "not-a-number"});
        let err = validate_tool_arguments("test", &schema, &args).unwrap_err();
        assert!(err.contains("Expected integer"));
    }

    #[test]
    fn test_validate_additional_properties() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "additionalProperties": false
        });
        let args = serde_json::json!({"name": "alice", "extra": "bad"});
        let err = validate_tool_arguments("test", &schema, &args).unwrap_err();
        assert!(err.contains("must NOT have additional properties"));
    }

    #[test]
    fn test_validate_not_an_object() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {}
        });
        let args = serde_json::json!("a string, not an object");
        let err = validate_tool_arguments("test", &schema, &args).unwrap_err();
        assert!(err.contains("Expected object"));
    }

    #[test]
    fn test_validate_array_item_types() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "tags": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            }
        });
        let args = serde_json::json!({"tags": [1, 2, 3]});
        let err = validate_tool_arguments("test", &schema, &args).unwrap_err();
        assert!(err.contains("Expected string"));
    }

    // ── Cancel ──────────────────────────────────────────────────────

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
