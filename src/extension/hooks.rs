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
pub fn run_before_hooks(tool_name: &str, args: &serde_json::Value) -> Option<BeforeToolCallResult> {
    let map = EXTENSION_HOOKS.read().ok()?;
    let map = map.as_ref()?;
    if let Some(set) = map.get(tool_name) {
        for hook in &set.before {
            if let Some(result) = hook(args)
                && result.block
            {
                return Some(result);
            }
        }
    }
    None
}

/// Run all registered after-hooks for a tool. Returns merged overrides.
pub fn run_after_hooks(
    tool_name: &str,
    result: &yoagent::types::ToolResult,
    is_error: bool,
) -> Option<AfterToolCallResult> {
    let map = EXTENSION_HOOKS.read().ok()?;
    let map = map.as_ref()?;
    let mut merged: Option<AfterToolCallResult> = None;
    if let Some(set) = map.get(tool_name) {
        for hook in &set.after {
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
    }
    merged
}
