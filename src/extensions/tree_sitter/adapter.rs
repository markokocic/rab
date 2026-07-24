//! Core types and helpers for language adapters.

#![allow(dead_code)]

use tree_sitter::StreamingIterator;

// ── Core types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Class,
    Method,
    Interface,
    Type,
    Variable,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Function => write!(f, "function"),
            Self::Class => write!(f, "class"),
            Self::Method => write!(f, "method"),
            Self::Interface => write!(f, "interface"),
            Self::Type => write!(f, "type"),
            Self::Variable => write!(f, "variable"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ByteRange {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
    pub range: ByteRange,
    pub signature: String,
    pub is_exported: bool,
    pub parent_class: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImportKind {
    /// C/C++ `#include` — local or system header.
    Header,
    /// Single-token module names: Go's "fmt", Python's os.path.
    Module,
    /// Fully-qualified: Java's java.util.List, Rust's std::sync::Arc, TS paths.
    Qualified,
}

#[derive(Debug, Clone)]
pub struct Import {
    pub names: Vec<String>,
    pub source: String,
    pub kind: ImportKind,
}

#[derive(Debug, Clone)]
pub struct ExtractedFile {
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    pub exports: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Callee {
    pub name: String,
    pub line: usize,
}

// ── Adapter entry (pi-tree-sitter style) ────────────────────────────────

/// A language adapter: pure functions that extract symbols and callees
/// from source code using a pre-configured tree-sitter parser.
pub struct AdapterEntry {
    pub extensions: &'static [&'static str],
    pub extract:
        fn(source: &str, parser: &mut tree_sitter::Parser) -> Result<ExtractedFile, String>,
    pub find_callees:
        fn(source: &str, parser: &mut tree_sitter::Parser, range: &ByteRange) -> Vec<Callee>,
}

// ── Helpers shared by adapters ──────────────────────────────────────────

/// Parse source and return the root tree. Errors if parse returns None.
pub fn parse_source(
    source: &str,
    parser: &mut tree_sitter::Parser,
) -> Result<tree_sitter::Tree, String> {
    parser
        .parse(source, None)
        .ok_or_else(|| "parse returned None".to_string())
}

/// Helper to iterate all named children of a node.
pub fn named_children(node: tree_sitter::Node) -> impl Iterator<Item = tree_sitter::Node> {
    (0..node.named_child_count()).filter_map(move |i| node.named_child(i as u32))
}

/// Convenience constructor for a Symbol — avoids repeating all 6 fields.
pub fn make_symbol(
    kind: SymbolKind,
    name: String,
    range: ByteRange,
    signature: String,
    is_exported: bool,
    parent_class: Option<String>,
) -> Symbol {
    Symbol {
        kind,
        name,
        range,
        signature,
        is_exported,
        parent_class,
    }
}

/// Convenience for an ExtractedFile with only symbols (no imports/exports).
pub fn extracted_file(symbols: Vec<Symbol>) -> ExtractedFile {
    ExtractedFile {
        symbols,
        imports: Vec::new(),
        exports: Vec::new(),
    }
}

/// First line of a multi-line string — used for signatures.
pub fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).to_string()
}

/// Extract a C/C++ function name from a function_definition declarator.
pub fn c_func_name(node: tree_sitter::Node, source: &str) -> Option<String> {
    let decl = node.child_by_field_name("declarator")?;
    let mut cursor: Option<tree_sitter::Node> = Some(decl);
    for _ in 0..5 {
        let n = cursor?;
        if let Some(nn) = n.child_by_field_name("name") {
            return Some(node_text(nn, source).to_string());
        }
        if let Some(inner) = n.child_by_field_name("declarator") {
            cursor = Some(inner);
            continue;
        }
        for j in 0..n.named_child_count() as u32 {
            if let Some(c) = n.named_child(j)
                && c.kind() == "identifier"
            {
                return Some(node_text(c, source).to_string());
            }
        }
        break;
    }
    None
}

/// Get the text of a tree-sitter node.
pub fn node_text<'a>(node: tree_sitter::Node, source: &'a str) -> &'a str {
    &source[node.start_byte()..node.end_byte()]
}

/// Get a `ByteRange` from a node.
pub fn node_range(node: tree_sitter::Node) -> ByteRange {
    ByteRange {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
    }
}

/// Signature: source from node start to just before the body (or end).
pub fn node_signature(node: tree_sitter::Node, source: &str) -> String {
    let body = node.child_by_field_name("body");
    let end = body.map(|b| b.start_byte()).unwrap_or(node.end_byte());
    source[node.start_byte()..end].trim().to_string()
}

/// Run a tree-sitter query on a pre-configured parser and return named captures.
pub fn query_captures(
    parser: &mut tree_sitter::Parser,
    source: &str,
    query_source: &str,
    capture_name: &str,
    range: Option<&ByteRange>,
) -> Vec<Callee> {
    let lang = match parser.language() {
        Some(l) => l,
        None => return vec![],
    };
    let query = match tree_sitter::Query::new(&lang, query_source) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    let capture_index = match query.capture_index_for_name(capture_name) {
        Some(i) => i,
        None => return vec![],
    };
    let Some(tree) = parser.parse(source, None) else {
        return vec![];
    };
    let root = tree.root_node();
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(&query, root, source.as_bytes());
    let mut results = Vec::new();
    while let Some(m) = matches.next() {
        for c in m.captures {
            if c.index != capture_index {
                continue;
            }
            if let Some(r) = range
                && (c.node.start_byte() < r.start_byte || c.node.start_byte() > r.end_byte)
            {
                continue;
            }
            let name = node_text(c.node, source).to_string();
            let line = c.node.start_position().row + 1;
            results.push(Callee { name, line });
        }
    }
    results
}
