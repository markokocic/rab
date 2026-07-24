//! Java language adapter.

use tree_sitter::{Language, Node};

use crate::extensions::tree_sitter::adapter::{
    node_range, node_signature, node_text, query_captures, AdapterEntry, ByteRange, Callee,
    ExtractedFile, Symbol, SymbolKind,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".java"],
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
            "class_declaration" | "enum_declaration" | "record_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Class, name: name.clone(),
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: true, parent_class: None,
                    });
                    if let Some(body) = child.child_by_field_name("body") {
                        java_walk_class_body(body, source, &mut symbols, &name);
                    }
                }
            }
            "interface_declaration" | "annotation_type_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Interface, name: node_text(nn, source).to_string(),
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: true, parent_class: None,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(ExtractedFile { symbols, imports: Vec::new(), exports: Vec::new() })
}

fn find_callees(source: &str, lang: &Language, range: &ByteRange) -> Vec<Callee> {
    query_captures(source, lang, "(method_invocation name: (identifier) @callee)", "callee", Some(range))
}

fn java_walk_class_body(body: Node, source: &str, symbols: &mut Vec<Symbol>, parent: &str) {
    for i in 0..body.named_child_count() as u32 {
        let Some(child) = body.named_child(i) else { continue };
        match child.kind() {
            "method_declaration" | "constructor_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Method, name: node_text(nn, source).to_string(),
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: true, parent_class: Some(parent.to_string()),
                    });
                }
            }
            "field_declaration" => {
                for j in 0..child.named_child_count() as u32 {
                    if let Some(decl) = child.named_child(j)
                        && decl.kind() == "variable_declarator"
                            && let Some(nn) = decl.child_by_field_name("name") {
                                symbols.push(Symbol {
                                    kind: SymbolKind::Variable, name: node_text(nn, source).to_string(),
                                    range: node_range(decl), signature: node_signature(child, source),
                                    is_exported: true, parent_class: Some(parent.to_string()),
                                });
                            }
                }
            }
            "class_declaration" | "interface_declaration" | "enum_declaration" | "record_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: if child.kind() == "interface_declaration" { SymbolKind::Interface } else { SymbolKind::Class },
                        name: name.clone(), range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true, parent_class: Some(parent.to_string()),
                    });
                    if let Some(body) = child.child_by_field_name("body") {
                        java_walk_class_body(body, source, symbols, &name);
                    }
                }
            }
            _ => {}
        }
    }
}
