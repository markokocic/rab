//! TypeScript/JavaScript language adapter.

use tree_sitter::Node;

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Import, ImportKind, Symbol, SymbolKind,
    named_children, node_range, node_signature, node_text, parse_source, query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"],
    extract,
    find_callees,
};

fn extract(source: &str, parser: &mut tree_sitter::Parser) -> Result<ExtractedFile, String> {
    let tree = parse_source(source, parser)?;
    let root = tree.root_node();

    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut exports = Vec::new();

    for child in named_children(root) {
        match child.kind() {
            "import_statement" => {
                if let Some(imp) = ts_extract_import(child, source) {
                    imports.push(imp);
                }
            }
            "function_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Function,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: is_ts_exported(child),
                        parent_class: None,
                    });
                }
            }
            "class_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Class,
                        name: name.clone(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: is_ts_exported(child),
                        parent_class: None,
                    });
                    if let Some(body) = child.child_by_field_name("body") {
                        ts_class_body(body, source, &mut symbols, &name);
                    }
                }
            }
            "interface_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Interface,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: is_ts_exported(child),
                        parent_class: None,
                    });
                }
            }
            "type_alias_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Type,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: is_ts_exported(child),
                        parent_class: None,
                    });
                }
            }
            "export_statement" => {
                if let Some(decl) = child.child_by_field_name("declaration") {
                    ts_walk_export_decl(decl, source, &mut symbols);
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                ts_walk_var_decls(child, source, &mut symbols, false);
            }
            _ => {}
        }
        // Collect exports from top-level exported symbols
        if let Some(nn) = child.child_by_field_name("name")
            && is_ts_exported(child)
        {
            exports.push(node_text(nn, source).to_string());
        }
    }

    Ok(ExtractedFile {
        symbols,
        imports,
        exports,
    })
}

fn find_callees(source: &str, parser: &mut tree_sitter::Parser, range: &ByteRange) -> Vec<Callee> {
    query_captures(
        parser,
        source,
        "(call_expression function: (identifier) @callee)",
        "callee",
        Some(range),
    )
}

/// Extract import names+source from an `import_statement` node.
fn ts_extract_import(node: tree_sitter::Node, source: &str) -> Option<Import> {
    let source_node = node.child_by_field_name("source")?;
    let module_path = node_text(source_node, source);
    let module_path = module_path.trim_matches(&['\'', '"'][..]).to_string();

    let mut names = Vec::new();
    if let Some(clause) = node.child_by_field_name("import") {
        if let Some(name_node) = clause.child_by_field_name("name") {
            names.push(node_text(name_node, source).to_string());
        }
        for i in 0..clause.named_child_count() as u32 {
            if let Some(child) = clause.named_child(i)
                && child.kind() == "named_imports"
            {
                for j in 0..child.named_child_count() as u32 {
                    if let Some(spec) = child.named_child(j)
                        && spec.kind() == "import_specifier"
                        && let Some(n) = spec.child_by_field_name("name")
                    {
                        names.push(node_text(n, source).to_string());
                    }
                }
            }
        }
    }

    Some(Import {
        names,
        source: module_path,
        kind: ImportKind::Qualified,
    })
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn is_ts_exported(node: Node) -> bool {
    node.parent()
        .is_some_and(|p| p.kind() == "export_statement")
}

fn ts_walk_export_decl(node: Node, source: &str, symbols: &mut Vec<Symbol>) {
    match node.kind() {
        "function_declaration" => {
            if let Some(nn) = node.child_by_field_name("name") {
                symbols.push(Symbol {
                    kind: SymbolKind::Function,
                    name: node_text(nn, source).to_string(),
                    range: node_range(node),
                    signature: node_signature(node, source),
                    is_exported: true,
                    parent_class: None,
                });
            }
        }
        "class_declaration" => {
            if let Some(nn) = node.child_by_field_name("name") {
                let name = node_text(nn, source).to_string();
                symbols.push(Symbol {
                    kind: SymbolKind::Class,
                    name: name.clone(),
                    range: node_range(node),
                    signature: node_signature(node, source),
                    is_exported: true,
                    parent_class: None,
                });
                if let Some(body) = node.child_by_field_name("body") {
                    ts_class_body(body, source, symbols, &name);
                }
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            ts_walk_var_decls(node, source, symbols, true);
        }
        _ => {}
    }
}

fn ts_walk_var_decls(node: Node, source: &str, symbols: &mut Vec<Symbol>, exported: bool) {
    for j in 0..node.named_child_count() as u32 {
        let Some(decl) = node.named_child(j) else {
            continue;
        };
        if decl.kind() != "variable_declarator" {
            continue;
        }
        if let Some(nn) = decl.child_by_field_name("name") {
            let val = decl.child_by_field_name("value");
            let is_fn = val
                .is_some_and(|v| v.kind() == "arrow_function" || v.kind() == "function_expression");
            symbols.push(Symbol {
                kind: if is_fn {
                    SymbolKind::Function
                } else {
                    SymbolKind::Variable
                },
                name: node_text(nn, source).to_string(),
                range: node_range(decl),
                signature: String::new(),
                is_exported: exported,
                parent_class: None,
            });
        }
    }
}

fn ts_class_body(body: Node, source: &str, symbols: &mut Vec<Symbol>, class_name: &str) {
    for i in 0..body.named_child_count() as u32 {
        let Some(child) = body.named_child(i) else {
            continue;
        };
        if child.kind() != "method_definition" {
            continue;
        }
        if let Some(nn) = child.child_by_field_name("name") {
            symbols.push(Symbol {
                kind: SymbolKind::Method,
                name: node_text(nn, source).to_string(),
                range: node_range(child),
                signature: node_signature(child, source),
                is_exported: false,
                parent_class: Some(class_name.to_string()),
            });
        }
    }
}
