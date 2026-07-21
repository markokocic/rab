//! Tree-sitter syntax validation extension.
//!
//! Provides pre-write/pre-edit syntax validation hooks and semantic code
//! analysis tools using tree-sitter grammars.
//!
//! Currently a skeleton: hooks validate nothing (always pass). The full
//! implementation is planned in `tree-sitter.md`.

use crate::agent::extension::{BeforeHook, Extension, ExtensionDefault, HookRegistration};
use std::borrow::Cow;

/// Tree-sitter extension: validates syntax on write/edit, provides semantic tools.
pub struct TreeSitterExtension;

impl TreeSitterExtension {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TreeSitterExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl Extension for TreeSitterExtension {
    fn name(&self) -> Cow<'static, str> {
        "tree-sitter".into()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn default_state(&self) -> ExtensionDefault {
        ExtensionDefault::Disabled
    }

    fn tool_hooks(&self) -> Vec<HookRegistration> {
        // Hardcoded always-ok validators — real implementation coming next.
        let write_hook: BeforeHook = std::sync::Arc::new(|_args| {
            // TODO: validate content with tree-sitter grammar
            None // always pass
        });

        let edit_hook: BeforeHook = std::sync::Arc::new(|_args| {
            // TODO: validate resulting content with tree-sitter grammar
            None // always pass
        });

        vec![
            HookRegistration {
                tool_name: "write",
                before: Some(write_hook),
                after: None,
            },
            HookRegistration {
                tool_name: "edit",
                before: Some(edit_hook),
                after: None,
            },
        ]
    }
}
