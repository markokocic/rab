//! Clojure language adapter.

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, node_range, node_text,
    query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".clj", ".cljs", ".cljc", ".bb", ".edn", ".cljd"],
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
        if child.kind() != "list_lit" {
            continue;
        }
        let Some(first) = child.named_child(0) else {
            continue;
        };
        if first.kind() != "sym_lit" {
            continue;
        }

        let head = node_text(first, source);
        let Some(name_node) = child.named_child(1) else {
            continue;
        };
        let name = node_text(name_node, source).to_string();

        match head {
            "defn" | "defn-" => {
                symbols.push(Symbol {
                    kind: SymbolKind::Function,
                    name,
                    range: node_range(child),
                    signature: first_line(node_text(child, source)),
                    is_exported: head != "defn-",
                    parent_class: None,
                });
            }
            "def" | "defonce" => {
                symbols.push(Symbol {
                    kind: SymbolKind::Variable,
                    name,
                    range: node_range(child),
                    signature: first_line(node_text(child, source)),
                    is_exported: true,
                    parent_class: None,
                });
            }
            "defprotocol" => {
                symbols.push(Symbol {
                    kind: SymbolKind::Interface,
                    name: name.clone(),
                    range: node_range(child),
                    signature: first_line(node_text(child, source)),
                    is_exported: true,
                    parent_class: None,
                });
                // Walk protocol methods
                for j in 2u32..child.named_child_count() as u32 {
                    if let Some(m) = child.named_child(j)
                        && m.kind() == "list_lit"
                        && let Some(mn) = m.named_child(0)
                        && mn.kind() == "sym_lit"
                    {
                        symbols.push(Symbol {
                            kind: SymbolKind::Method,
                            name: node_text(mn, source).to_string(),
                            range: node_range(m),
                            signature: String::new(),
                            is_exported: true,
                            parent_class: Some(name.clone()),
                        });
                    }
                }
            }
            "defrecord" | "deftype" => {
                symbols.push(Symbol {
                    kind: SymbolKind::Class,
                    name,
                    range: node_range(child),
                    signature: first_line(node_text(child, source)),
                    is_exported: true,
                    parent_class: None,
                });
            }
            "defmethod" => {
                let dispatch = child
                    .named_child(2)
                    .map(|n| node_text(n, source).to_string());
                symbols.push(Symbol {
                    kind: SymbolKind::Method,
                    name,
                    range: node_range(child),
                    signature: first_line(node_text(child, source)),
                    is_exported: true,
                    parent_class: dispatch,
                });
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
        "(list_lit (sym_lit) @callee)",
        "callee",
        Some(range),
    )
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).to_string()
}
