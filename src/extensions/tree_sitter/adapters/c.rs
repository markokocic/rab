//! C language adapter.

use tree_sitter::Node;

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, extracted_file,
    named_children, node_range, node_signature, node_text, parse_source, query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".c", ".h"],
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
            "struct_specifier" | "union_specifier" | "enum_specifier" => {
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

fn c_func_name(node: Node, source: &str) -> Option<String> {
    let decl = node.child_by_field_name("declarator")?;
    let mut cursor: Option<Node> = Some(decl);
    for _ in 0..5 {
        let n = cursor?;
        if let Some(nn) = n.child_by_field_name("name") {
            return Some(node_text(nn, source).to_string());
        }
        if let Some(inner) = n.child_by_field_name("declarator") {
            cursor = Some(inner);
            continue;
        }
        for j in 0..n.named_child_count() as u32 {
            if let Some(c) = n.named_child(j)
                && c.kind() == "identifier"
            {
                return Some(node_text(c, source).to_string());
            }
        }
        break;
    }
    None
}
