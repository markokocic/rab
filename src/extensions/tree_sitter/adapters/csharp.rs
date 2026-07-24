//! C# language adapter.

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, node_range, node_signature,
    node_text, query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".cs"],
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
            "class_declaration" | "struct_declaration" => {
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
                        for j in 0..body.named_child_count() as u32 {
                            if let Some(m) = body.named_child(j)
                                && m.kind() == "method_declaration"
                                && let Some(mn) = m.child_by_field_name("name")
                            {
                                symbols.push(Symbol {
                                    kind: SymbolKind::Method,
                                    name: node_text(mn, source).to_string(),
                                    range: node_range(m),
                                    signature: node_signature(m, source),
                                    is_exported: false,
                                    parent_class: Some(name.clone()),
                                });
                            }
                        }
                    }
                }
            }
            "interface_declaration" => {
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
            "enum_declaration" => {
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
            "namespace_declaration" => {
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
        "(invocation_expression function: (identifier) @callee)",
        "callee",
        Some(range),
    )
}
