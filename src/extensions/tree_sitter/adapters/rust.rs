//! Rust language adapter.

use tree_sitter::{Language, Node};

use crate::extensions::tree_sitter::adapter::{
    node_range, node_signature, node_text, query_captures, AdapterEntry, ByteRange, Callee,
    ExtractedFile, Symbol, SymbolKind,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".rs"],
    extract,
    find_callees,
};

fn extract(source: &str, lang: &Language) -> Result<ExtractedFile, String> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(lang)
        .map_err(|e| format!("set_language: {e}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or("parse returned None")?;
    let root = tree.root_node();

    let mut symbols = Vec::new();

    for i in 0..root.named_child_count() as u32 {
        let Some(item) = root.named_child(i) else { continue };

        match item.kind() {
            "function_item" | "function_signature_item" => {
                if let Some(name) = rs_ident_child(item, source) {
                    symbols.push(Symbol {
                        kind: SymbolKind::Function,
                        name,
                        range: node_range(item),
                        signature: node_signature(item, source),
                        is_exported: rs_is_pub(item),
                        parent_class: None,
                    });
                }
            }
            "struct_item" | "enum_item" | "union_item" => {
                if let Some(name) = rs_ident_child(item, source) {
                    symbols.push(Symbol {
                        kind: SymbolKind::Class,
                        name,
                        range: node_range(item),
                        signature: first_line(node_text(item, source)),
                        is_exported: rs_is_pub(item),
                        parent_class: None,
                    });
                }
            }
            "trait_item" => {
                if let Some(name) = rs_ident_child(item, source) {
                    symbols.push(Symbol {
                        kind: SymbolKind::Interface,
                        name,
                        range: node_range(item),
                        signature: first_line(node_text(item, source)),
                        is_exported: rs_is_pub(item),
                        parent_class: None,
                    });
                }
            }
            "type_item" => {
                if let Some(name) = rs_ident_child(item, source) {
                    symbols.push(Symbol {
                        kind: SymbolKind::Type,
                        name,
                        range: node_range(item),
                        signature: node_signature(item, source),
                        is_exported: rs_is_pub(item),
                        parent_class: None,
                    });
                }
            }
            "impl_item" => {
                let type_node = item.child_by_field_name("type");
                let target = type_node.and_then(|n| rs_type_leaf_name(n, source));
                if let Some(target) = target {
                    for j in 0..item.named_child_count() as u32 {
                        let Some(body) = item.named_child(j) else { continue };
                        if body.kind() == "declaration_list" {
                            rs_impl_body(body, source, &mut symbols, &target);
                        }
                    }
                }
            }
            "const_item" | "static_item" => {
                if let Some(name) = rs_ident_child(item, source) {
                    symbols.push(Symbol {
                        kind: SymbolKind::Variable,
                        name,
                        range: node_range(item),
                        signature: node_signature(item, source),
                        is_exported: rs_is_pub(item),
                        parent_class: None,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(ExtractedFile { symbols, imports: Vec::new(), exports: Vec::new() })
}

fn find_callees(source: &str, lang: &Language, range: &ByteRange) -> Vec<Callee> {
    let queries = [
        "(call_expression function: (identifier) @callee)",
        "(call_expression function: (field_expression field: (field_identifier) @callee))",
        "(macro_invocation macro: (identifier) @callee)",
    ];
    let mut results = Vec::new();
    for q in &queries {
        results.extend(query_captures(source, lang, q, "callee", Some(range)));
    }
    results
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).to_string()
}

fn rs_ident_child(node: Node, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(c) = node.named_child(i)
            && (c.kind() == "identifier" || c.kind() == "type_identifier") {
                return Some(node_text(c, source).to_string());
            }
    }
    None
}

fn rs_is_pub(node: Node) -> bool {
    for i in 0..node.named_child_count() as u32 {
        if let Some(c) = node.named_child(i)
            && c.kind() == "visibility_modifier" {
                return true;
            }
    }
    false
}

fn rs_type_leaf_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(node_text(node, source).to_string()),
        "generic_type" | "scoped_type_identifier" => {
            for i in 0..node.named_child_count() as u32 {
                if let Some(c) = node.named_child(i)
                    && let Some(name) = rs_type_leaf_name(c, source) {
                        return Some(name);
                    }
            }
            None
        }
        _ => None,
    }
}

fn rs_impl_body(body: Node, source: &str, symbols: &mut Vec<Symbol>, target: &str) {
    for i in 0..body.named_child_count() as u32 {
        let Some(child) = body.named_child(i) else { continue };
        if child.kind() != "function_item" {
            continue;
        }
        if let Some(name) = rs_ident_child(child, source) {
            symbols.push(Symbol {
                kind: SymbolKind::Method,
                name,
                range: node_range(child),
                signature: node_signature(child, source),
                is_exported: rs_is_pub(child),
                parent_class: Some(target.to_string()),
            });
        }
    }
}
