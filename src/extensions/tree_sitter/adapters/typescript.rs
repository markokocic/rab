//! TypeScript/JavaScript language adapter.

use tree_sitter::{Language, Node};

use crate::extensions::tree_sitter::adapter::{
    node_range, node_signature, node_text, query_captures, AdapterEntry, ByteRange, Callee,
    ExtractedFile, Symbol, SymbolKind,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"],
    extract,
    find_callees,
};

fn extract(source: &str, lang: &Language) -> Result<ExtractedFile, String> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(lang).map_err(|e| format!("set_language: {e}"))?;
    let tree = parser.parse(source, None).ok_or("parse returned None")?;
    let root = tree.root_node();

    let mut symbols = Vec::new();

    for i in 0..root.named_child_count() as u32 {
        let Some(child) = root.named_child(i) else { continue };
        match child.kind() {
            "function_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Function, name: node_text(nn, source).to_string(),
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: is_ts_exported(child), parent_class: None,
                    });
                }
            }
            "class_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Class, name: name.clone(),
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: is_ts_exported(child), parent_class: None,
                    });
                    if let Some(body) = child.child_by_field_name("body") {
                        ts_class_body(body, source, &mut symbols, &name);
                    }
                }
            }
            "interface_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Interface, name: node_text(nn, source).to_string(),
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: is_ts_exported(child), parent_class: None,
                    });
                }
            }
            "type_alias_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Type, name: node_text(nn, source).to_string(),
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: is_ts_exported(child), parent_class: None,
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
    }

    Ok(ExtractedFile { symbols })
}

fn find_callees(source: &str, lang: &Language, range: &ByteRange) -> Vec<Callee> {
    query_captures(
        source, lang,
        "(call_expression function: (identifier) @callee)",
        "callee", Some(range),
    )
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn is_ts_exported(node: Node) -> bool {
    node.parent().is_some_and(|p| p.kind() == "export_statement")
}

fn ts_walk_export_decl(node: Node, source: &str, symbols: &mut Vec<Symbol>) {
    match node.kind() {
        "function_declaration" => {
            if let Some(nn) = node.child_by_field_name("name") {
                symbols.push(Symbol {
                    kind: SymbolKind::Function, name: node_text(nn, source).to_string(),
                    range: node_range(node), signature: node_signature(node, source),
                    is_exported: true, parent_class: None,
                });
            }
        }
        "class_declaration" => {
            if let Some(nn) = node.child_by_field_name("name") {
                let name = node_text(nn, source).to_string();
                symbols.push(Symbol {
                    kind: SymbolKind::Class, name: name.clone(),
                    range: node_range(node), signature: node_signature(node, source),
                    is_exported: true, parent_class: None,
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
        let Some(decl) = node.named_child(j) else { continue };
        if decl.kind() != "variable_declarator" { continue; }
        if let Some(nn) = decl.child_by_field_name("name") {
            let val = decl.child_by_field_name("value");
            let is_fn = val.is_some_and(|v| v.kind() == "arrow_function" || v.kind() == "function_expression");
            symbols.push(Symbol {
                kind: if is_fn { SymbolKind::Function } else { SymbolKind::Variable },
                name: node_text(nn, source).to_string(),
                range: node_range(decl),
                signature: String::new(),
                is_exported: exported, parent_class: None,
            });
        }
    }
}

fn ts_class_body(body: Node, source: &str, symbols: &mut Vec<Symbol>, class_name: &str) {
    for i in 0..body.named_child_count() as u32 {
        let Some(child) = body.named_child(i) else { continue };
        if child.kind() != "method_definition" { continue; }
        if let Some(nn) = child.child_by_field_name("name") {
            symbols.push(Symbol {
                kind: SymbolKind::Method, name: node_text(nn, source).to_string(),
                range: node_range(child), signature: node_signature(child, source),
                is_exported: false, parent_class: Some(class_name.to_string()),
            });
        }
    }
}
