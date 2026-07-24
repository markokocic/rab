//! Elixir language adapter.

use tree_sitter::Node;

use crate::extensions::tree_sitter::adapter::{
    AdapterEntry, ByteRange, Callee, ExtractedFile, Symbol, SymbolKind, extracted_file, node_range,
    node_signature, node_text, parse_source, query_captures,
};

pub(super) const ENTRY: AdapterEntry = AdapterEntry {
    extensions: &[".ex", ".exs"],
    extract,
    find_callees,
};

const DEF_KWS: &[&str] = &[
    "def",
    "defp",
    "defmacro",
    "defmacrop",
    "defguard",
    "defguardp",
];
const MOD_KWS: &[&str] = &["defmodule", "defprotocol", "defimpl"];

fn extract(source: &str, parser: &mut tree_sitter::Parser) -> Result<ExtractedFile, String> {
    let tree = parse_source(source, parser)?;
    let root = tree.root_node();
    let mut symbols = Vec::new();
    elixir_walk_block(root, source, &mut symbols, None);
    Ok(extracted_file(symbols))
}

fn find_callees(source: &str, parser: &mut tree_sitter::Parser, range: &ByteRange) -> Vec<Callee> {
    query_captures(
        parser,
        source,
        "(call target: (identifier) @callee)",
        "callee",
        Some(range),
    )
}

fn elixir_kw<'a>(call: Node, source: &'a str) -> Option<&'a str> {
    let target = call.child_by_field_name("target")?;
    if target.kind() == "identifier" {
        Some(node_text(target, source))
    } else {
        None
    }
}

fn elixir_func_name(call: Node, source: &str) -> Option<String> {
    let args = call.child_by_field_name("arguments")?;
    let first = args.named_child(0)?;
    if first.kind() == "identifier" || first.kind() == "call" {
        Some(node_text(first, source).to_string())
    } else {
        None
    }
}

fn elixir_mod_name(call: Node, source: &str) -> Option<String> {
    let args = call.child_by_field_name("arguments")?;
    let first = args.named_child(0)?;
    match first.kind() {
        "alias" | "identifier" => Some(node_text(first, source).to_string()),
        _ => {
            for i in 0..first.named_child_count() as u32 {
                if let Some(c) = first.named_child(i)
                    && (c.kind() == "alias" || c.kind() == "identifier")
                {
                    return Some(node_text(c, source).to_string());
                }
            }
            None
        }
    }
}

fn elixir_do_block(call: Node) -> Option<Node> {
    for i in 0..call.named_child_count() as u32 {
        if let Some(c) = call.named_child(i)
            && c.kind() == "do_block"
        {
            return Some(c);
        }
    }
    None
}

fn elixir_walk_block(block: Node, source: &str, symbols: &mut Vec<Symbol>, parent: Option<&str>) {
    for i in 0..block.named_child_count() as u32 {
        let Some(c) = block.named_child(i) else {
            continue;
        };
        if c.kind() != "call" {
            continue;
        }
        let Some(kw) = elixir_kw(c, source) else {
            continue;
        };

        if MOD_KWS.contains(&kw) {
            if let Some(name) = elixir_mod_name(c, source) {
                let kind = if kw == "defprotocol" {
                    SymbolKind::Interface
                } else {
                    SymbolKind::Class
                };
                symbols.push(Symbol {
                    kind,
                    name: name.clone(),
                    range: node_range(c),
                    signature: node_signature(c, source),
                    is_exported: true,
                    parent_class: parent.map(|s| s.to_string()),
                });
                if let Some(db) = elixir_do_block(c) {
                    elixir_walk_block(db, source, symbols, Some(&name));
                }
            }
        } else if DEF_KWS.contains(&kw)
            && let Some(name) = elixir_func_name(c, source)
        {
            symbols.push(Symbol {
                kind: SymbolKind::Function,
                name,
                range: node_range(c),
                signature: node_signature(c, source),
                is_exported: !kw.ends_with('p'),
                parent_class: parent.map(|s| s.to_string()),
            });
        }
    }
}
