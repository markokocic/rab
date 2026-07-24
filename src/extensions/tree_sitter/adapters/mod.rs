//! Language adapter registry — flat array (pi-tree-sitter style).

use std::path::Path;

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
    // C must come before C++ so that non-.h extensions route correctly.
    // .h is handled by adapter_for_path with content sniffing.
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
/// Does NOT handle `.h` sniffing — use [`adapter_for_path`] for that.
pub fn adapter_for_ext(ext: &str) -> Option<&'static AdapterEntry> {
    ADAPTERS.iter().find(|a| a.extensions.contains(&ext))
}

/// Find the adapter for a file, with `.h` content sniffing.
/// `.h` files are checked for C++-only tokens (`class`, `namespace`, `template`, `::`);
/// if found, the C++ adapter is used. Otherwise falls back to the C adapter.
pub fn adapter_for_path(path: &Path) -> Option<&'static AdapterEntry> {
    let ext = path.extension()?.to_str()?;
    let ext = format!(".{ext}");
    if ext != ".h" {
        return adapter_for_ext(&ext);
    }

    // Content sniffing for .h files
    let source = std::fs::read_to_string(path).ok()?;
    if looks_like_cpp_header(&source) {
        // Find the C++ adapter
        ADAPTERS.iter().find(|a| a.extensions.contains(&".hpp"))
    } else {
        adapter_for_ext(".c")
    }
}

/// Cheap sniff: scan the first 32 KiB for C++-only tokens.
fn looks_like_cpp_header(src: &str) -> bool {
    const SNIFF_BYTES: usize = 32 * 1024;
    let head = if src.len() > SNIFF_BYTES {
        let mut cut = SNIFF_BYTES;
        while cut > 0 && !src.is_char_boundary(cut) {
            cut -= 1;
        }
        &src[..cut]
    } else {
        src
    };
    let cleaned = strip_c_comments_and_strings(head);
    cleaned.contains("class ")
        || cleaned.contains("namespace ")
        || cleaned.contains("template<")
        || cleaned.contains("template <")
        || cleaned.contains("::")
}

/// Strip C/C++ block comments, line comments, and double-quoted strings
/// so tokens inside them don't false-trigger C++ detection.
fn strip_c_comments_and_strings(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // /* … */
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            out.push(' ');
            continue;
        }
        // // …
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            out.push(' ');
            continue;
        }
        // "…"
        if b == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            i = (i + 1).min(bytes.len());
            out.push(' ');
            continue;
        }
        out.push(b as char);
        i += 1;
    }
    out
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
