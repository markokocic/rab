//! Zig language adapter.

use tree_sitter::Language;

use crate::extensions::tree_sitter::adapter::{
    node_range, node_signature, node_text, query_captures, AdapterEntry, ByteRange, Callee,
    ExtractedFile, Symbol, SymbolKind,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".zig"],
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
        let t = child.kind();
        if t == "function_declaration" || t == "fn_prototype" {
            if let Some(nn) = child.child_by_field_name("name") {
                symbols.push(Symbol {
                    kind: SymbolKind::Function, name: node_text(nn, source).to_string(),
                    range: node_range(child), signature: node_signature(child, source),
                    is_exported: true, parent_class: None,
                });
            }
        } else if t == "variable_declaration" {
            if let Some(nn) = child.child_by_field_name("name") {
                symbols.push(Symbol {
                    kind: SymbolKind::Variable, name: node_text(nn, source).to_string(),
                    range: node_range(child), signature: node_signature(child, source),
                    is_exported: true, parent_class: None,
                });
            }
        } else if t == "container_declaration"
            && let Some(nn) = child.child_by_field_name("name") {
                symbols.push(Symbol {
                    kind: SymbolKind::Class, name: node_text(nn, source).to_string(),
                    range: node_range(child), signature: node_signature(child, source),
                    is_exported: true, parent_class: None,
                });
            }
    }
    Ok(ExtractedFile { symbols, imports: Vec::new(), exports: Vec::new() })
}

fn find_callees(source: &str, lang: &Language, range: &ByteRange) -> Vec<Callee> {
    query_captures(source, lang, "(call_expression function: (identifier) @callee)", "callee", Some(range))
}
