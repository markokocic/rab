pub mod file_search;
pub mod mcp;

/// Tree-sitter extension with WASM grammar support.
/// Disabled on Android (Termux) where cranelift-codegen hits a rustc parser bug.
#[cfg(not(target_os = "android"))]
pub mod tree_sitter;
