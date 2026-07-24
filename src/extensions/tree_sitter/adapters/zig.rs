//! Zig language adapter.

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, extracted_file,
    named_children, node_range, node_signature, node_text, parse_source, query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".zig"],
    extract,
    find_callees,
};

fn extract(source: &str, parser: &mut tree_sitter::Parser) -> Result<ExtractedFile, String> {
    let tree = parse_source(source, parser)?;
    let root = tree.root_node();
    let mut symbols = Vec::new();

    for child in named_children(root) {
        let t = child.kind();
        if t == "function_declaration" || t == "fn_prototype" {
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
        } else if t == "variable_declaration" {
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
        } else if t == "container_declaration"
            && let Some(nn) = child.child_by_field_name("name")
        {
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
