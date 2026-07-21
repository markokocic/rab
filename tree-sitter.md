# Tree-sitter Extension — Implementation Plan

## Overview

Port pi-tree-sitter's syntax validation and semantic code analysis from
TypeScript/Node.js to a Rust native extension using the `tree-sitter` crate.
Inspired by dirge's `syntax_validator.rs` and semantic adapter architecture.

## Current State

A skeleton extension (`src/extensions/tree_sitter/`) with:
- Extension struct registered and loaded
- `tool_hooks()` returning `BeforeHook`s for `write` and `edit` — both hardcoded to always pass
- General hook mechanism: `Extension::tool_hooks()` → `HookRegistration` applied to all tool definitions

## Phase 1 — Dependencies & Grammar Loading

### Cargo.toml additions

```toml
# Core tree-sitter runtime
tree-sitter = "0.25"

# Language grammars (each behind a feature flag)
tree-sitter-rust = { version = "0.23", optional = true }
tree-sitter-typescript = { version = "0.23", optional = true }
tree-sitter-python = { version = "0.23", optional = true }
tree-sitter-go = { version = "0.23", optional = true }
tree-sitter-java = { version = "0.23", optional = true }
tree-sitter-c = { version = "0.23", optional = true }
tree-sitter-cpp = { version = "0.23", optional = true }
tree-sitter-ruby = { version = "0.23", optional = true }
tree-sitter-bash = { version = "0.23", optional = true }
tree-sitter-json = { version = "0.23", optional = true }
tree-sitter-elixir = { version = "0.23", optional = true }
tree-sitter-haskell = { version = "0.23", optional = true }
```

### Features

```toml
[features]
default = []
semantic-rust = ["tree-sitter-rust"]
semantic-ts = ["tree-sitter-typescript"]
semantic-python = ["tree-sitter-python"]
semantic-go = ["tree-sitter-go"]
semantic-java = ["tree-sitter-java"]
semantic-c = ["tree-sitter-c"]
semantic-cpp = ["tree-sitter-cpp"]
semantic-ruby = ["tree-sitter-ruby"]
semantic-bash = ["tree-sitter-bash"]
semantic-json = ["tree-sitter-json"]
semantic-elixir = ["tree-sitter-elixir"]
semantic-haskell = ["tree-sitter-haskell"]
```

### Implementation — grammar.rs

```rust
// Feature-gated language resolution — mirrors dirge's approach.
fn language_for_path(path: &Path) -> Option<tree_sitter::Language> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        #[cfg(feature = "semantic-rust")]
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        #[cfg(feature = "semantic-ts")]
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" =>
            Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        #[cfg(feature = "semantic-python")]
        "py" | "pyi" => Some(tree_sitter_python::LANGUAGE.into()),
        // ... per extension ...
        _ => None,
    }
}
```

### Files to create

| File | Purpose |
|------|---------|
| `src/extensions/tree_sitter/grammar.rs` | `language_for_path()`, extension→language map |

## Phase 2 — Syntax Validation (write-time guard)

### Implementation — validator.rs

Port dirge's `syntax_validator.rs` directly:

1. **`SyntaxError` struct** — line, column, snippet, is_missing, expected token
2. **`collect_errors()`** — walk tree-sitter tree, collect ERROR/MISSING nodes (capped at 10)
3. **`check_syntax()`** — parse content, return errors
4. **`format_errors()`** — produce actionable message for the LLM
5. **`DelimiterBalance` scanner** — comment/string-aware delimiter counting for languages without
   tree-sitter grammars (Clojure, Scheme, Janet, etc.)
6. **`validate_or_repair()`** — on delimiter imbalance, try auto-append closing delimiters and
   re-validate before giving up

### Integration with hook system

Replace the hardcoded `None` (always pass) hooks in `TreeSitterExtension::tool_hooks()`:

```rust
fn tool_hooks(&self) -> Vec<HookRegistration> {
    let write_hook: BeforeHook = Arc::new(|args| {
        let path = args["path"].as_str()?;
        let content = args["content"].as_str()?;
        match check_syntax(Path::new(path), content) {
            Ok(()) => None,
            Err(errors) => Some(BeforeToolCallResult {
                block: true,
                reason: format_errors(path, content, &errors),
            }),
        }
    });
    // Similar for edit (read file, apply edits, validate result)
    ...
}
```

### Files to create

| File | Purpose |
|------|---------|
| `src/extensions/tree_sitter/validator.rs` | `check_syntax()`, `format_errors()`, delimiter scanner, auto-repair |
| `src/extensions/tree_sitter/types.rs` | `SyntaxError`, `ByteRange`, `Symbol` types |

### Testing

Port dirge's test suite:
- Balanced code per language yields no errors
- Broken code returns structured errors
- Delimiter scanner correctly ignores strings/comments/char-literals
- Auto-repair closes trailing truncations, refuses mid-file stray openers

## Phase 3 — Per-Language Symbol Extraction

### Architecture

Each language gets a `LangConfig`:

```rust
pub struct LangConfig {
    pub extensions: &'static [&'static str],
    pub extract: fn(&str, tree_sitter::Language) -> ExtractedFile,
    pub find_callees: fn(&str, tree_sitter::Language, ByteRange) -> Vec<Callee>,
}
```

### Files to create

```
src/extensions/tree_sitter/languages/
├── mod.rs           # Registry (LANGUAGES array) + lookup functions
├── rust.rs          # Rust symbol extraction
├── typescript.rs    # TypeScript/JavaScript
├── python.rs        # Python
├── go.rs            # Go
├── java.rs          # Java
├── ...              # Per-language: ruby, c, cpp, bash, json, elixir, haskell
```

### Common helpers (extract.rs)

```rust
pub fn node_text(node: Node, source: &str) -> String { ... }
pub fn node_range(node: Node) -> ByteRange { ... }
pub fn sig(node: Node, source: &str) -> String { ... }
pub fn query_captures(...) -> Vec<Callee> { ... }
```

### Symbol kinds

```rust
pub enum SymbolKind {
    Function, Class, Method, Interface, Type, Variable,
}
```

### Per-language logic (port from pi-tree-sitter/src/languages.ts)

Each language implements AST walking to extract:
- Top-level declarations (functions, classes, interfaces, type aliases, variables)
- Class/body members (methods, nested classes)
- Export detection
- Callee extraction via tree-sitter S-expression queries

Core languages (MVP):
1. **Rust** — function_item, struct_item, impl_item, trait_item
2. **TypeScript** — function_declaration, class_declaration, interface_declaration
3. **Python** — function_definition, class_definition, decorated_definition
4. **Go** — function_declaration, method_declaration, type_spec
5. **Java** — class_declaration, method_declaration, field_declaration
6. **C/C++** — function_definition, struct_specifier, class_specifier
7. **Ruby** — method, class, module
8. **Bash** — function_definition
9. **Elixir** — def, defmodule, defprotocol

## Phase 4 — Semantic Tools

### Tool definitions (5 tools, matching pi-tree-sitter)

| Tool | Description |
|------|-------------|
| `list_symbols` | List symbols in a file or across the project |
| `find_definition` | Find where a symbol is defined |
| `find_callers` | Find call sites of a function/method |
| `get_symbol_body` | Get full source of a named symbol |
| `find_callees` | Find all callees of a symbol |

### Implementation pattern

Each tool:
1. Implements `AgentTool` trait
2. Uses `extract_file()` or `extract_all_files()` to parse code
3. Filters/sorts symbols by the query parameters
4. Returns formatted text result
5. Has a `ToolRenderer` for TUI display

### Project file discovery

Adapt pi-tree-sitter's `files.ts` — walk project directory, skip
`node_modules`/`.git`/`target`/`build`, collect files with known extensions.

```rust
pub fn find_project_files(dir: &Path, max_files: usize) -> Vec<PathBuf> { ... }
```

### Files to create

| File | Purpose |
|------|---------|
| `src/extensions/tree_sitter/tools.rs` | Tool structs + `AgentTool` impls |
| `src/extensions/tree_sitter/files.rs` | `find_project_files()`, `read_file_safe()` |

## Phase 5 — TUI Renderers

Port pi-tree-sitter's `renderCall`/`renderResult` functions for each tool:

| Tool | Collapsed | Expanded |
|------|-----------|----------|
| `list_symbols` | "✓ N symbols across M files" | Full symbol list |
| `find_definition` | "✓ N definitions for 'name'" | File:line for each |
| `get_symbol_body` | "✓ name (N lines) in path" | Syntax-highlighted body |
| `find_callers`/`find_callees` | "✓ N callers/callees for 'name'" | Full list |

Uses `src/tui/components/` (Text, StyledSegment highlight_code) for rendering.

### Files to create

| File | Purpose |
|------|---------|
| `src/extensions/tree_sitter/renderer.rs` | Symbol tool renderers |

## Phase 6 — Extension Registration & Config

### Features in Cargo.toml

Create a `tree-sitter` feature group that enables all language grammars:

```toml
tree-sitter-full = [
    "semantic-rust", "semantic-ts", "semantic-python", "semantic-go",
    "semantic-java", "semantic-c", "semantic-cpp", "semantic-ruby",
    "semantic-bash", "semantic-json", "semantic-elixir", "semantic-haskell",
]
```

### Toggle via /extensions UI

The extension defaults to `Disabled` — users enable it via `/extensions` or
the config file.

## Implementation Order

```
Phase 1 ── Dependencies + grammar.rs            ~ 1 hour
Phase 2 ── validator.rs + types.rs              ~ 3 hours
Phase 3 ── extract.rs + languages/*.rs          ~ 4 hours (core set)
Phase 4 ── tools.rs + files.rs + tool hooks     ~ 3 hours
Phase 5 ── renderer.rs                          ~ 2 hours
Phase 6 ── Testing + integration                ~ 2 hours
```

**Total: ~15 hours for MVP (Rust + TS/JS + Python validation + semantic tools).**

## Future Work

- **WASM grammar loading** — fallback for languages without native Rust crates
  (Swift, Kotlin, Dart, etc.) via `web-tree-sitter` in a sidecar process
- **LSP integration** — use language servers for richer diagnostics beyond
  tree-sitter parse errors
- **Incremental parsing** — cache tree-sitter trees for edited files to avoid
  re-parsing the full file on every edit
- **Auto-insert missing delimiter** — borrow dirge's `repair_delimiters()` that
  appends missing closers and re-validates before writing
