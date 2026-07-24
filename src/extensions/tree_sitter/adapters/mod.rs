//! Language adapter registry — flat array (pi-tree-sitter style).

mod bash;
mod c;
mod clojure;
mod cpp;
mod csharp;
mod dart;
mod elixir;
mod go;
mod java;
mod kotlin;
mod lua;
mod php;
mod python;
mod ruby;
mod rust;
mod scala;
mod swift;
mod typescript;
mod zig;

use crate::extensions::tree_sitter::adapter::AdapterEntry;

const ADAPTERS: &[AdapterEntry] = &[
    rust::ENTRY,
    typescript::ENTRY,
    python::ENTRY,
    go::ENTRY,
    java::ENTRY,
    kotlin::ENTRY,
    clojure::ENTRY,
    c::ENTRY,
    cpp::ENTRY,
    ruby::ENTRY,
    bash::ENTRY,
    lua::ENTRY,
    php::ENTRY,
    scala::ENTRY,
    swift::ENTRY,
    zig::ENTRY,
    elixir::ENTRY,
    csharp::ENTRY,
    dart::ENTRY,
];

/// Find the adapter for a file extension (e.g. ".rs").
pub fn adapter_for_ext(ext: &str) -> Option<&'static AdapterEntry> {
    ADAPTERS.iter().find(|a| a.extensions.contains(&ext))
}

/// All known extensions (with leading dot).
pub fn all_extensions() -> Vec<&'static str> {
    let mut set: Vec<&'static str> = Vec::new();
    for a in ADAPTERS {
        for e in a.extensions {
            if !set.contains(e) {
                set.push(e);
            }
        }
    }
    set
}
