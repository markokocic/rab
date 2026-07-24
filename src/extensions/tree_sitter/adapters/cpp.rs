//! C++ language adapter.

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, c_func_name,
    extracted_file, named_children, node_range, node_signature, node_text, parse_source,
    query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".cpp", ".cc", ".cxx", ".hpp", ".hh", ".hxx", ".h"],
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
                if let Some(name) = c_func_name(child, source) {
                    symbols.push(Symbol {
                        kind: SymbolKind::Function,
                        name,
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: None,
                    });
                }
            }
            "class_specifier" | "struct_specifier" | "union_specifier" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Class,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: None,
                    });
                }
            }
            "enum_specifier" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Class,
                        name: node_text(nn, source).to_string(),
                        range: node_range(child),
                        signature: node_signature(child, source),
                        is_exported: true,
                        parent_class: None,
                    });
                }
            }
            "namespace_definition" => {
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
        "(call_expression function: (identifier) @callee)",
        "callee",
        Some(range),
    )
}
