//! Ruby language adapter.

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, node_range, node_signature,
    node_text, query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".rb"],
    extract,
    find_callees,
};

fn extract(source: &str, parser: &mut tree_sitter::Parser) -> Result<ExtractedFile, String> {
    let tree = parser.parse(source, None).ok_or("parse returned None")?;
    let root = tree.root_node();
    let mut symbols = Vec::new();

    for i in 0..root.named_child_count() as u32 {
        let Some(child) = root.named_child(i) else {
            continue;
        };
        match child.kind() {
            "method" => {
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
            "class" => {
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
                }
            }
            "module" => {
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
            "singleton_method" => {
                if let Some(nn) = child.child_by_field_name("name") {
                    symbols.push(Symbol {
                        kind: SymbolKind::Method,
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
    Ok(ExtractedFile {
        symbols,
        imports: Vec::new(),
        exports: Vec::new(),
    })
}

fn find_callees(source: &str, parser: &mut tree_sitter::Parser, range: &ByteRange) -> Vec<Callee> {
    query_captures(
        parser,
        source,
        "(call method: (identifier) @callee)",
        "callee",
        Some(range),
    )
}
