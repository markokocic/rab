//! Semantic tools: list_symbols, find_definition, find_callers, get_symbol_body, find_callees.
//!
//! Grammar download path is async; grammar loading/parsing is sync.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use yoagent::types::{AgentTool, ToolContext, ToolError, ToolResult};

use crate::extensions::tree_sitter::adapter::{Callee, Symbol};
use crate::extensions::tree_sitter::adapters::adapter_for_path;
use crate::extensions::tree_sitter::files::{find_project_files, read_file_safe};
use crate::extensions::tree_sitter::grammar::GrammarManager;

// ── Extract symbols from a file ─────────────────────────────────────────

async fn extract_file(
    path: &Path,
    manager: &GrammarManager,
) -> Result<Option<(String, Vec<Symbol>)>, String> {
    let entry =
        adapter_for_path(path).ok_or_else(|| format!("no adapter for {}", path.display()))?;
    let source =
        read_file_safe(path).ok_or_else(|| format!("could not read {}", path.display()))?;
    let ext = format!(
        ".{}",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    );

    manager
        .ensure(&ext)
        .await
        .map_err(|e| format!("ensure grammar {ext}: {e}"))?;

    let result = manager
        .with_parser(&ext, |parser| (entry.extract)(&source, parser))
        .map_err(|e| format!("with_parser {ext}: {e}"))?;

    Ok(result.map(|extracted| (path.display().to_string(), extracted.symbols)))
}

/// Extract callees for a specific symbol in a file.
async fn callees_for_symbol(
    path: &Path,
    sym: &Symbol,
    manager: &GrammarManager,
) -> Result<Option<Vec<Callee>>, String> {
    let entry =
        adapter_for_path(path).ok_or_else(|| format!("no adapter for {}", path.display()))?;
    let source =
        read_file_safe(path).ok_or_else(|| format!("could not read {}", path.display()))?;
    let ext = format!(
        ".{}",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    );

    manager
        .ensure(&ext)
        .await
        .map_err(|e| format!("ensure grammar {ext}: {e}"))?;

    let result = manager
        .with_parser(&ext, |parser| {
            Ok((entry.find_callees)(&source, parser, &sym.range))
        })
        .map_err(|e| format!("with_parser {ext}: {e}"))?;

    Ok(result)
}

// ── ListSymbolsTool ─────────────────────────────────────────────────────

pub struct ListSymbolsTool {
    manager: Arc<GrammarManager>,
}

impl ListSymbolsTool {
    pub fn new(manager: Arc<GrammarManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AgentTool for ListSymbolsTool {
    fn name(&self) -> &str {
        "list_symbols"
    }
    fn label(&self) -> &str {
        "List Symbols"
    }
    fn description(&self) -> &str {
        "List symbols (functions, classes, methods, etc.) in a file or across the project."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path. Omit for project-wide." },
                "kind": { "type": "string", "description": "Filter: function, class, method, interface, type, variable" }
            }
        })
    }
    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let filter_kind = params
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());
        if let Some(path_str) = params.get("path").and_then(|v| v.as_str()) {
            let path = Path::new(path_str);
            let syms = match extract_file(path, &self.manager).await {
                Ok(Some((_, s))) => s,
                Ok(None) => vec![],
                Err(e) => return Err(ToolError::Failed(e)),
            };
            let syms = filter_symbols(syms, filter_kind.as_deref());
            Ok(ToolResult {
                content: vec![yoagent::types::Content::Text {
                    text: format_symbols(&syms, path_str),
                }],
                details: serde_json::json!({"count": syms.len(), "label": "symbols", "fileCount": 1}),
            })
        } else {
            let cwd = std::env::current_dir().unwrap_or_default();
            let files = find_project_files(&cwd, 2000);
            let mut all_syms = Vec::new();
            for file in &files {
                if let Ok(Some((path_str, syms))) = extract_file(file, &self.manager).await {
                    let syms = filter_symbols(syms, filter_kind.as_deref());
                    if !syms.is_empty() {
                        all_syms.push((path_str, syms));
                    }
                }
            }
            let total: usize = all_syms.iter().map(|(_, s)| s.len()).sum();
            Ok(ToolResult {
                content: vec![yoagent::types::Content::Text {
                    text: format_all_symbols(&all_syms),
                }],
                details: serde_json::json!({"count": total, "label": "symbols", "fileCount": all_syms.len()}),
            })
        }
    }
}

// ── FindDefinitionTool ──────────────────────────────────────────────────

pub struct FindDefinitionTool {
    manager: Arc<GrammarManager>,
}

impl FindDefinitionTool {
    pub fn new(manager: Arc<GrammarManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AgentTool for FindDefinitionTool {
    fn name(&self) -> &str {
        "find_definition"
    }
    fn label(&self) -> &str {
        "Find Definition"
    }
    fn description(&self) -> &str {
        "Find where a symbol (function, class, type, etc.) is defined across the project."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object", "required": ["name"],
            "properties": { "name": { "type": "string", "description": "Name of the symbol to find" } }
        })
    }
    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'name'".into()))?;
        let hits = find_symbols_globally(name, &self.manager)
            .await
            .map_err(ToolError::Failed)?;
        if hits.is_empty() {
            return Ok(ToolResult {
                content: vec![yoagent::types::Content::Text {
                    text: format!("No definition found for '{name}'"),
                }],
                details: serde_json::json!({"count": 0, "label": "definitions", "name": name}),
            });
        }
        let mut lines = format!("Found {} definition(s) for '{name}':", hits.len());
        for (path, sym) in &hits {
            lines.push_str(&format!(
                "\n  {path}:{} [{}] {}",
                sym.range.start_line, sym.kind, sym.signature
            ));
        }
        Ok(ToolResult {
            content: vec![yoagent::types::Content::Text { text: lines }],
            details: serde_json::json!({"count": hits.len(), "label": "definitions", "name": name}),
        })
    }
}

// ── GetSymbolBodyTool ───────────────────────────────────────────────────

pub struct GetSymbolBodyTool {
    manager: Arc<GrammarManager>,
}

impl GetSymbolBodyTool {
    pub fn new(manager: Arc<GrammarManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AgentTool for GetSymbolBodyTool {
    fn name(&self) -> &str {
        "get_symbol_body"
    }
    fn label(&self) -> &str {
        "Get Symbol Body"
    }
    fn description(&self) -> &str {
        "Get the full source code of a named symbol from a file. Extracts by AST byte range."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object", "required": ["path", "name"],
            "properties": {
                "path": { "type": "string", "description": "Path to the file" },
                "name": { "type": "string", "description": "Name of the symbol" }
            }
        })
    }
    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let path_str = params["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'path'".into()))?;
        let name = params["name"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'name'".into()))?;
        let path = Path::new(path_str);

        let syms = match extract_file(path, &self.manager).await {
            Ok(Some((_, s))) => s,
            Ok(None) => {
                return Err(ToolError::Failed(format!(
                    "Symbol '{name}' not found in {path_str}"
                )));
            }
            Err(e) => return Err(ToolError::Failed(e)),
        };
        let sym = syms
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| ToolError::Failed(format!("Symbol '{name}' not found in {path_str}")))?;

        let source = read_file_safe(path)
            .ok_or_else(|| ToolError::Failed(format!("Could not read {path_str}")))?;
        let body = &source[sym.range.start_byte..sym.range.end_byte];
        let line_count = body.lines().count();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        Ok(ToolResult {
            content: vec![yoagent::types::Content::Text {
                text: format!("Symbol: {name} in {path_str}\n\n{body}"),
            }],
            details: serde_json::json!({"body": body, "name": name, "path": path_str, "lineCount": line_count, "language": ext}),
        })
    }
}

// ── FindCallersTool ─────────────────────────────────────────────────────

pub struct FindCallersTool {
    manager: Arc<GrammarManager>,
}

impl FindCallersTool {
    pub fn new(manager: Arc<GrammarManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AgentTool for FindCallersTool {
    fn name(&self) -> &str {
        "find_callers"
    }
    fn label(&self) -> &str {
        "Find Callers"
    }
    fn description(&self) -> &str {
        "Find all call sites of a function or method across the project. Uses AST queries."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object", "required": ["name"],
            "properties": {
                "name": { "type": "string", "description": "Name of the function/method" },
                "path": { "type": "string", "description": "Directory to search (defaults to cwd)" }
            }
        })
    }
    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'name'".into()))?;
        let cwd = std::env::current_dir().unwrap_or_default();
        let search_dir = params
            .get("path")
            .and_then(|v| v.as_str())
            .map(Path::new)
            .unwrap_or(&cwd);

        let files = find_project_files(search_dir, 2000);
        let mut callers: Vec<(String, Symbol)> = Vec::new();

        for file in &files {
            let Ok(Some((path_str, syms))) = extract_file(file, &self.manager).await else {
                continue;
            };
            for sym in &syms {
                if sym.name == name {
                    continue;
                }
                if let Ok(Some(callees)) = callees_for_symbol(file, sym, &self.manager).await
                    && callees.iter().any(|c| c.name == name)
                {
                    callers.push((path_str.clone(), sym.clone()));
                }
            }
        }

        if callers.is_empty() {
            return Ok(ToolResult {
                content: vec![yoagent::types::Content::Text {
                    text: format!("No callers found for '{name}'"),
                }],
                details: serde_json::json!({"count": 0, "label": "callers", "name": name}),
            });
        }

        let mut lines = format!("{} caller(s) for '{name}':", callers.len());
        for (path, sym) in &callers {
            lines.push_str(&format!(
                "\n  {path}:{} [{}] {}",
                sym.range.start_line, sym.kind, sym.name
            ));
        }
        Ok(ToolResult {
            content: vec![yoagent::types::Content::Text { text: lines }],
            details: serde_json::json!({"count": callers.len(), "label": "callers", "name": name}),
        })
    }
}

// ── FindCalleesTool ─────────────────────────────────────────────────────

pub struct FindCalleesTool {
    manager: Arc<GrammarManager>,
}

impl FindCalleesTool {
    pub fn new(manager: Arc<GrammarManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AgentTool for FindCalleesTool {
    fn name(&self) -> &str {
        "find_callees"
    }
    fn label(&self) -> &str {
        "Find Callees"
    }
    fn description(&self) -> &str {
        "Find all functions/methods called by a given symbol. Uses AST queries."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object", "required": ["path", "name"],
            "properties": {
                "path": { "type": "string", "description": "Path to the file" },
                "name": { "type": "string", "description": "Name of the function/method" }
            }
        })
    }
    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let path_str = params["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'path'".into()))?;
        let name = params["name"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'name'".into()))?;
        let path = Path::new(path_str);

        let syms = match extract_file(path, &self.manager).await {
            Ok(Some((_, s))) => s,
            Ok(None) => {
                return Err(ToolError::Failed(format!(
                    "Symbol '{name}' not found in {path_str}"
                )));
            }
            Err(e) => return Err(ToolError::Failed(e)),
        };
        let sym = syms
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| ToolError::Failed(format!("Symbol '{name}' not found in {path_str}")))?;

        let callees = callees_for_symbol(path, sym, &self.manager)
            .await
            .map_err(ToolError::Failed)?
            .unwrap_or_default();
        if callees.is_empty() {
            return Ok(ToolResult {
                content: vec![yoagent::types::Content::Text {
                    text: format!("No callees found for '{name}'"),
                }],
                details: serde_json::json!({"count": 0, "label": "callees", "name": name}),
            });
        }

        let mut lines = format!("Callees of {name} in {path_str}:");
        for c in &callees {
            lines.push_str(&format!("\n  {}  {}", c.line, c.name));
        }
        Ok(ToolResult {
            content: vec![yoagent::types::Content::Text { text: lines }],
            details: serde_json::json!({"count": callees.len(), "label": "callees", "name": name}),
        })
    }
}

// ── Shared helpers ──────────────────────────────────────────────────────

async fn find_symbols_globally(
    name: &str,
    manager: &GrammarManager,
) -> Result<Vec<(String, Symbol)>, String> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let files = find_project_files(&cwd, 2000);
    let mut hits = Vec::new();
    for file in &files {
        if let Ok(Some((path_str, syms))) = extract_file(file, manager).await {
            for sym in syms {
                if sym.name == name {
                    hits.push((path_str.clone(), sym));
                }
            }
        }
    }
    Ok(hits)
}

fn filter_symbols(syms: Vec<Symbol>, kind: Option<&str>) -> Vec<Symbol> {
    match kind {
        Some(k) => syms
            .into_iter()
            .filter(|s| s.kind.to_string() == k)
            .collect(),
        None => syms,
    }
}

fn format_symbol(sym: &Symbol) -> String {
    let class = sym
        .parent_class
        .as_ref()
        .map(|c| format!(" [class: {c}]"))
        .unwrap_or_default();
    let export = if sym.is_exported { " (exported)" } else { "" };
    format!(
        "  {}-{} [{}] {}{}{}",
        sym.range.start_line, sym.range.end_line, sym.kind, sym.name, class, export
    )
}

fn format_symbols(syms: &[Symbol], file_path: &str) -> String {
    if syms.is_empty() {
        return format!("No symbols found in {file_path}");
    }
    let mut out = format!("## {file_path}");
    for sym in syms {
        out.push('\n');
        out.push_str(&format_symbol(sym));
    }
    out.push_str(&format!("\n\n{} symbols", syms.len()));
    out
}

fn format_all_symbols(all: &[(String, Vec<Symbol>)]) -> String {
    let mut out = String::new();
    let mut total = 0;
    for (path, syms) in all {
        out.push_str(&format!("## {path}\n"));
        for sym in syms {
            out.push_str(&format_symbol(sym));
            out.push('\n');
        }
        total += syms.len();
    }
    out.push_str(&format!("\n{total} symbols across {} files", all.len()));
    out
}
