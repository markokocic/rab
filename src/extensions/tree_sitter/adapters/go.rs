//! Go language adapter.

use tree_sitter::{Language, Node};

use crate::extensions::tree_sitter::adapter::{
    node_range, node_signature, node_text, query_captures, AdapterEntry, ByteRange, Callee,
    ExtractedFile, Symbol, SymbolKind,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".go"],
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
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Function, name: name.clone(),
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: go_is_exported(&name), parent_class: None,
                    });
                }
            }
            "method_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    let parent_class = child.child_by_field_name("receiver")
                        .and_then(|r| go_receiver_type(r, source));
                    symbols.push(Symbol {
                        kind: SymbolKind::Method, name: name.clone(),
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: go_is_exported(&name), parent_class,
                    });
                }
            }
            "type_spec" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    let type_node = child.child_by_field_name("type");
                    let kind = type_node.map(|t| t.kind()).unwrap_or("");
                    let sk = match kind {
                        "struct_type" => SymbolKind::Class,
                        "interface_type" => SymbolKind::Interface,
                        _ => SymbolKind::Type,
                    };
                    symbols.push(Symbol {
                        kind: sk, name: name.clone(),
                        range: node_range(child),
                        signature: first_line(node_text(child, source)),
                        is_exported: go_is_exported(&name), parent_class: None,
                    });
                }
            }
            "var_declaration" | "const_declaration" => go_walk_specs(child, source, &mut symbols),
            _ => {}
        }
    }

    Ok(ExtractedFile { symbols, imports: Vec::new(), exports: Vec::new() })
}

fn find_callees(source: &str, lang: &Language, range: &ByteRange) -> Vec<Callee> {
    let queries = [
        "(call_expression function: (identifier) @callee)",
        "(call_expression function: (selector_expression field: (field_identifier) @callee))",
    ];
    let mut results = Vec::new();
    for q in &queries {
        results.extend(query_captures(source, lang, q, "callee", Some(range)));
    }
    results
}

fn first_line(s: &str) -> String { s.lines().next().unwrap_or(s).to_string() }
fn go_is_exported(name: &str) -> bool { name.starts_with(|c: char| c.is_uppercase()) }

fn go_receiver_type(node: Node, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(c) = node.named_child(i)
            && c.kind() == "type_identifier" {
                return Some(node_text(c, source).to_string());
            }
    }
    None
}

fn go_walk_specs(node: Node, source: &str, symbols: &mut Vec<Symbol>) {
    for i in 0..node.named_child_count() as u32 {
        let Some(child) = node.named_child(i) else { continue };
        match child.kind() {
            "var_spec_list" | "const_spec_list" => go_walk_specs(child, source, symbols),
            "var_spec" | "const_spec" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Variable, name,
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: false, parent_class: None,
                    });
                }
            }
            _ => {}
        }
    }
}
