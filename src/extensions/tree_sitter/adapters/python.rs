//! Python language adapter.

use tree_sitter::Node;

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, extracted_file,
    named_children, node_range, node_signature, node_text, parse_source, query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".py", ".pyi"],
    extract,
    find_callees,
};

fn extract(source: &str, parser: &mut tree_sitter::Parser) -> Result<ExtractedFile, String> {
    let tree = parse_source(source, parser)?;
    let root = tree.root_node();

    let mut symbols = Vec::new();

    for child in named_children(root) {
        match child.kind() {
            "function_definition" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Function,
                        name: name.clone(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: py_is_exported(&name),
                        parent_class: None,
                    });
                }
            }
            "decorated_definition" => {
                if let Some(fn_node) = child.child_by_field_name("definition")
                    && fn_node.kind() == "function_definition"
                    && let Some(nn) = fn_node.child_by_field_name("name")
                {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Function,
                        name: name.clone(),
                        range: node_range(child),
                        signature: node_signature(fn_node, source),
                        is_exported: py_is_exported(&name),
                        parent_class: None,
                    });
                }
            }
            "class_definition" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    let name = node_text(nn, source).to_string();
                    symbols.push(Symbol {
                        kind: SymbolKind::Class,
                        name: name.clone(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: py_is_exported(&name),
                        parent_class: None,
                    });
                    if let Some(body) = child.child_by_field_name("body") {
                        py_class_body(body, source, &mut symbols, &name);
                    }
                }
            }
            _ => {}
        }
    }

    Ok(extracted_file(symbols))
}

fn find_callees(source: &str, parser: &mut tree_sitter::Parser, range: &ByteRange) -> Vec<Callee> {
    let queries = [
        "(call function: (identifier) @callee)",
        "(call function: (attribute attribute: (identifier) @callee))",
    ];
    let mut results = Vec::new();
    for q in &queries {
        results.extend(query_captures(parser, source, q, "callee", Some(range)));
    }
    results
}

fn py_is_exported(name: &str) -> bool {
    !name.starts_with('_') || (name.starts_with("__") && name.ends_with("__"))
}

fn py_class_body(body: Node, source: &str, symbols: &mut Vec<Symbol>, class_name: &str) {
    for i in 0..body.named_child_count() as u32 {
        let Some(child) = body.named_child(i) else {
            continue;
        };
        match child.kind() {
            "function_definition" => {
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
            "decorated_definition" => {
                if let Some(inner) = child.child_by_field_name("definition")
                    && inner.kind() == "function_definition"
                    && let Some(nn) = inner.child_by_field_name("name")
                {
                    symbols.push(Symbol {
                        kind: SymbolKind::Method,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(inner, source),
                        is_exported: false,
                        parent_class: Some(class_name.to_string()),
                    });
                }
            }
            _ => {}
        }
    }
}
