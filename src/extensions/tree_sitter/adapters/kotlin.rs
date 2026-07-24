//! Kotlin language adapter.

use tree_sitter::Node;

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, extracted_file,
    named_children, node_range, node_signature, node_text, parse_source, query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".kt", ".kts"],
    extract,
    find_callees,
};

fn extract(source: &str, parser: &mut tree_sitter::Parser) -> Result<ExtractedFile, String> {
    let tree = parse_source(source, parser)?;
    let root = tree.root_node();

    let mut symbols = Vec::new();

    for child in named_children(root) {
        match child.kind() {
            "function_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Function,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: None,
                    });
                }
            }
            "class_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    let kind = if kt_is_interface(child, source) {
                        SymbolKind::Interface
                    } else {
                        SymbolKind::Class
                    };
                    symbols.push(Symbol {
                        kind,
                        name: name.clone(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: None,
                    });
                    kt_walk_class_bodies(child, source, &mut symbols, &name);
                }
            }
            "object_declaration" => {
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
                    kt_walk_class_bodies(child, source, &mut symbols, &name);
                }
            }
            "property_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Variable,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: None,
                    });
                }
            }
            "type_alias" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Type,
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
    let queries = [
        "(call_expression (expression (primary_expression (identifier) @callee)))",
        "(call_expression (navigation_expression (navigation_suffix (simple_identifier) @callee)))",
    ];
    let mut results = Vec::new();
    for q in &queries {
        results.extend(query_captures(parser, source, q, "callee", Some(range)));
    }
    results
}

fn kt_is_interface(node: Node, source: &str) -> bool {
    for i in 0..node.child_count() as u32 {
        if let Some(c) = node.child(i)
            && !c.is_named()
        {
            let token = &source[c.start_byte()..c.end_byte()];
            if token == "interface" {
                return true;
            }
            if token == "class" || token == "enum" {
                return false;
            }
        }
    }
    false
}

fn kt_walk_class_bodies(node: Node, source: &str, symbols: &mut Vec<Symbol>, parent: &str) {
    for body in named_children(node) {
        if body.kind() == "class_body" || body.kind() == "enum_class_body" {
            kt_walk_class_body(body, source, symbols, parent);
        }
    }
}

fn kt_walk_class_body(body: Node, source: &str, symbols: &mut Vec<Symbol>, parent: &str) {
    for child in named_children(body) {
        match child.kind() {
            "function_declaration" => {
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
            "property_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Variable,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: Some(parent.to_string()),
                    });
                }
            }
            "companion_object" => {
                let nn = child.child_by_field_name("name");
                let comp_name = nn
                    .map(|n| node_text(n, source).to_string())
                    .unwrap_or_else(|| "Companion".to_string());
                symbols.push(Symbol {
                    kind: SymbolKind::Class,
                    name: comp_name.clone(),
                    range: node_range(child),
                    signature: node_signature(child, source),
                    is_exported: true,
                    parent_class: Some(parent.to_string()),
                });
                kt_walk_class_bodies(child, source, symbols, &comp_name);
            }
            "class_declaration" | "object_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Class,
                        name: name.clone(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: Some(parent.to_string()),
                    });
                    kt_walk_class_bodies(child, source, symbols, &name);
                }
            }
            "type_alias" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Type,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: Some(parent.to_string()),
                    });
                }
            }
            _ => {}
        }
    }
}
