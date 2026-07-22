//! Global extension hook registry.
//!
//! Extensions can register before/after hooks on any tool. Hooks are
//! collected at startup via [`register_tool_hooks`] and invoked by
//! [`run_before_hooks`] / [`run_after_hooks`] during tool execution.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::extension::types::{
    AfterHook, AfterToolCallResult, BeforeHook, BeforeToolCallResult, HookRegistration,
};

struct ToolHookSet {
    before: Vec<BeforeHook>,
    after: Vec<AfterHook>,
}

/// Global registry mapping tool names → extension hooks.
static EXTENSION_HOOKS: RwLock<Option<HashMap<&'static str, ToolHookSet>>> = RwLock::new(None);

/// Clear all registered extension hooks.
pub fn clear_tool_hooks() {
    let mut map = EXTENSION_HOOKS.write().unwrap();
    *map = None;
}

/// Register extension hooks for a tool name (called during startup).
pub fn register_tool_hooks(registrations: &[HookRegistration]) {
    let mut map = EXTENSION_HOOKS.write().unwrap();
    let map = map.get_or_insert_with(HashMap::new);
    for reg in registrations {
        let entry = map.entry(reg.tool_name).or_insert_with(|| ToolHookSet {
            before: vec![],
            after: vec![],
        });
        if let Some(ref before) = reg.before {
            entry.before.push(before.clone());
        }
        if let Some(ref after) = reg.after {
            entry.after.push(after.clone());
        }
    }
}

/// Run all registered before-hooks for a tool. Returns the first blocking result.
///
/// Clones hook refs under the read lock and releases it before invocation,
/// so the hook system is reentrant-safe.
pub fn run_before_hooks(tool_name: &str, args: &serde_json::Value) -> Option<BeforeToolCallResult> {
    let hooks: Vec<BeforeHook> = {
        let map = EXTENSION_HOOKS.read().ok()?;
        let map = map.as_ref()?;
        map.get(tool_name)
            .map(|set| set.before.clone())
            .unwrap_or_default()
    };
    for hook in &hooks {
        if let Some(result) = hook(args)
            && result.block
        {
            return Some(result);
        }
    }
    None
}

/// Run all registered after-hooks for a tool. Returns merged overrides.
///
/// Clones hook refs under the read lock and releases it before invocation,
/// so the hook system is reentrant-safe.
pub fn run_after_hooks(
    tool_name: &str,
    result: &yoagent::types::ToolResult,
    is_error: bool,
) -> Option<AfterToolCallResult> {
    let hooks: Vec<AfterHook> = {
        let map = EXTENSION_HOOKS.read().ok()?;
        let map = map.as_ref()?;
        map.get(tool_name)
            .map(|set| set.after.clone())
            .unwrap_or_default()
    };
    let mut merged: Option<AfterToolCallResult> = None;
    for hook in &hooks {
        if let Some(override_result) = hook(result, is_error) {
            let m = merged.get_or_insert(AfterToolCallResult {
                content: None,
                details: None,
                is_error: None,
            });
            if let Some(content) = override_result.content {
                m.content = Some(content);
            }
            if let Some(details) = override_result.details {
                m.details = Some(details);
            }
            if let Some(err) = override_result.is_error {
                m.is_error = Some(err);
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn blocking_before() -> BeforeHook {
        Arc::new(|_: &serde_json::Value| {
            Some(BeforeToolCallResult {
                block: true,
                reason: "blocked by test".to_string(),
            })
        })
    }

    fn nonblocking_before() -> BeforeHook {
        Arc::new(|_: &serde_json::Value| None)
    }

    fn content_after() -> AfterHook {
        Arc::new(|_: &yoagent::types::ToolResult, _: bool| {
            Some(AfterToolCallResult {
                content: Some(vec![yoagent::types::Content::Text {
                    text: "overridden".to_string(),
                }]),
                details: None,
                is_error: None,
            })
        })
    }

    fn err_flag_after() -> AfterHook {
        Arc::new(|_: &yoagent::types::ToolResult, is_error: bool| {
            Some(AfterToolCallResult {
                content: None,
                details: None,
                is_error: Some(!is_error),
            })
        })
    }

    fn make_result() -> yoagent::types::ToolResult {
        yoagent::types::ToolResult {
            content: vec![yoagent::types::Content::Text {
                text: "original".to_string(),
            }],
            details: serde_json::Value::Null,
        }
    }

    /// All hook tests run sequentially (not in parallel) because they share
    /// the global EXTENSION_HOOKS static. Rust runs tests across modules in
    /// parallel by default, so we keep all hook assertions in one test.
    #[test]
    fn hook_lifecycle() {
        // Start clean
        clear_tool_hooks();

        // No hooks returns None
        let args = serde_json::json!({"key": "value"});
        assert!(run_before_hooks("any-tool", &args).is_none());
        assert!(run_after_hooks("any-tool", &make_result(), false).is_none());

        // Register a blocking before-hook
        clear_tool_hooks();
        register_tool_hooks(&[HookRegistration {
            tool_name: "test-tool",
            before: Some(blocking_before()),
            after: None,
        }]);
        let result = run_before_hooks("test-tool", &serde_json::json!({}));
        assert!(result.is_some());
        assert!(result.unwrap().block);

        // Non-blocking before-hook passes through
        clear_tool_hooks();
        register_tool_hooks(&[HookRegistration {
            tool_name: "test-tool",
            before: Some(nonblocking_before()),
            after: None,
        }]);
        assert!(run_before_hooks("test-tool", &serde_json::json!({})).is_none());

        // After-hook overrides content
        clear_tool_hooks();
        register_tool_hooks(&[HookRegistration {
            tool_name: "test-tool",
            before: None,
            after: Some(content_after()),
        }]);
        let overridden = run_after_hooks("test-tool", &make_result(), false);
        assert!(overridden.is_some());
        let o = overridden.unwrap();
        assert!(o.content.is_some());
        let content = o.content.unwrap();
        match &content[0] {
            yoagent::types::Content::Text { text } => {
                assert_eq!(text, "overridden");
            }
            _ => panic!("Expected Text content"),
        }

        // Hooks are isolated by tool name
        clear_tool_hooks();
        register_tool_hooks(&[HookRegistration {
            tool_name: "tool-a",
            before: Some(blocking_before()),
            after: None,
        }]);
        assert!(run_before_hooks("tool-b", &serde_json::json!({})).is_none());
        assert!(run_before_hooks("tool-a", &serde_json::json!({})).is_some());

        // Clear removes all hooks
        clear_tool_hooks();
        register_tool_hooks(&[HookRegistration {
            tool_name: "test-tool",
            before: Some(blocking_before()),
            after: None,
        }]);
        assert!(run_before_hooks("test-tool", &serde_json::json!({})).is_some());
        clear_tool_hooks();
        assert!(run_before_hooks("test-tool", &serde_json::json!({})).is_none());

        // Multiple before-hooks: first blocker wins
        clear_tool_hooks();
        register_tool_hooks(&[
            HookRegistration {
                tool_name: "test-tool",
                before: Some(nonblocking_before()),
                after: None,
            },
            HookRegistration {
                tool_name: "test-tool",
                before: Some(blocking_before()),
                after: None,
            },
        ]);
        let result = run_before_hooks("test-tool", &serde_json::json!({}));
        assert!(result.is_some());
        assert!(result.unwrap().block);

        // After-hook on error flag
        clear_tool_hooks();
        register_tool_hooks(&[HookRegistration {
            tool_name: "test-tool",
            before: None,
            after: Some(err_flag_after()),
        }]);
        let overridden = run_after_hooks("test-tool", &make_result(), false);
        assert!(overridden.is_some());
        assert_eq!(overridden.unwrap().is_error, Some(true));

        // Clean up
        clear_tool_hooks();
    }
}
