//! Java language adapter.

use tree_sitter::Node;

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, extracted_file,
    named_children, node_range, node_signature, node_text, parse_source, query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".java"],
    extract,
    find_callees,
};

fn extract(source: &str, parser: &mut tree_sitter::Parser) -> Result<ExtractedFile, String> {
    let tree = parse_source(source, parser)?;
    let root = tree.root_node();

    let mut symbols = Vec::new();

    for child in named_children(root) {
        match child.kind() {
            "class_declaration" | "enum_declaration" | "record_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Class,
                        name: name.clone(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: None,
                    });
                    if let Some(body) = child.child_by_field_name("body") {
                        java_walk_class_body(body, source, &mut symbols, &name);
                    }
                }
            }
            "interface_declaration" | "annotation_type_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Interface,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: None,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(extracted_file(symbols))
}

fn find_callees(source: &str, parser: &mut tree_sitter::Parser, range: &ByteRange) -> Vec<Callee> {
    query_captures(
        parser,
        source,
        "(method_invocation name: (identifier) @callee)",
        "callee",
        Some(range),
    )
}

fn java_walk_class_body(body: Node, source: &str, symbols: &mut Vec<Symbol>, parent: &str) {
    for i in 0..body.named_child_count() as u32 {
        let Some(child) = body.named_child(i) else {
            continue;
        };
        match child.kind() {
            "method_declaration" | "constructor_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Method,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: Some(parent.to_string()),
                    });
                }
            }
            "field_declaration" => {
                for j in 0..child.named_child_count() as u32 {
                    if let Some(decl) = child.named_child(j)
                        && decl.kind() == "variable_declarator"
                        && let Some(nn) = decl.child_by_field_name("name")
                    {
                        symbols.push(Symbol {
                            kind: SymbolKind::Variable,
                            name: node_text(nn, source).to_string(),
                            range: node_range(decl),
                            signature: node_signature(child, source),
                            is_exported: true,
                            parent_class: Some(parent.to_string()),
                        });
                    }
                }
            }
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: if child.kind() == "interface_declaration" {
                            SymbolKind::Interface
                        } else {
                            SymbolKind::Class
                        },
                        name: name.clone(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: Some(parent.to_string()),
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
