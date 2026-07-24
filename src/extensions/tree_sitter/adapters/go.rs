//! Go language adapter.

use tree_sitter::Node;

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, extracted_file, first_line,
    named_children, node_range, node_signature, node_text, parse_source, query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".go"],
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
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Function,
                        name: name.clone(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: go_is_exported(&name),
                        parent_class: None,
                    });
                }
            }
            "method_declaration" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    let parent_class = child
                        .child_by_field_name("receiver")
                        .and_then(|r| go_receiver_type(r, source));
                    symbols.push(Symbol {
                        kind: SymbolKind::Method,
                        name: name.clone(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: go_is_exported(&name),
                        parent_class,
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
                        kind: sk,
                        name: name.clone(),
                        range: node_range(child),
                        signature: first_line(node_text(child, source)),
                        is_exported: go_is_exported(&name),
                        parent_class: None,
                    });
                }
            }
            "var_declaration" | "const_declaration" => go_walk_specs(child, source, &mut symbols),
            _ => {}
        }
    }

    Ok(extracted_file(symbols))
}

fn find_callees(source: &str, parser: &mut tree_sitter::Parser, range: &ByteRange) -> Vec<Callee> {
    let queries = [
        "(call_expression function: (identifier) @callee)",
        "(call_expression function: (selector_expression field: (field_identifier) @callee))",
    ];
    let mut results = Vec::new();
    for q in &queries {
        results.extend(query_captures(parser, source, q, "callee", Some(range)));
    }
    results
}

fn go_is_exported(name: &str) -> bool {
    name.starts_with(|c: char| c.is_uppercase())
}

fn go_receiver_type(node: Node, source: &str) -> Option<String> {
    for c in named_children(node) {
        if c.kind() == "type_identifier" {
            return Some(node_text(c, source).to_string());
        }
    }
    None
}

fn go_walk_specs(node: Node, source: &str, symbols: &mut Vec<Symbol>) {
    for child in named_children(node) {
        match child.kind() {
            "var_spec_list" | "const_spec_list" => go_walk_specs(child, source, symbols),
            "var_spec" | "const_spec" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Variable,
                        name,
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: false,
                        parent_class: None,
                    });
                }
            }
            _ => {}
        }
    }
}
