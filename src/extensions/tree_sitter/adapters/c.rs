//! C language adapter.

use tree_sitter::{Language, Node};

use crate::extensions::tree_sitter::adapter::{
    node_range, node_signature, node_text, query_captures, AdapterEntry, ByteRange, Callee,
    ExtractedFile, Symbol, SymbolKind,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".c", ".h"],
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
            "function_definition" => {
                if let Some(name) = c_func_name(child, source) {
                    symbols.push(Symbol {
                        kind: SymbolKind::Function, name,
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: true, parent_class: None,
                    });
                }
            }
            "struct_specifier" | "union_specifier" | "enum_specifier" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Class, name: node_text(nn, source).to_string(),
                        range: node_range(child), signature: node_signature(child, source),
                        is_exported: true, parent_class: None,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(ExtractedFile { symbols })
}

fn find_callees(source: &str, lang: &Language, range: &ByteRange) -> Vec<Callee> {
    query_captures(source, lang, "(call_expression function: (identifier) @callee)", "callee", Some(range))
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
                && c.kind() == "identifier" {
                    return Some(node_text(c, source).to_string());
                }
        }
        break;
    }
    None
}
