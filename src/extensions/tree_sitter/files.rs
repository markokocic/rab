//! File discovery utilities — find project files matching known extensions.

use std::path::Path;

use crate::extensions::tree_sitter::adapters::all_extensions;

/// Directories to skip when walking the project tree.
const IGNORE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".svn",
    ".hg",
    "target",
    "build",
    "dist",
    ".next",
    ".cache",
    "__pycache__",
    "venv",
    ".venv",
    ".tox",
    "vendor",
    ".bundle",
    "elm-stuff",
    ".gradle",
    "coverage",
];

/// Find all files with known extensions under `dir`, up to `max_files`.
pub fn find_project_files(dir: &Path, max_files: usize) -> Vec<std::path::PathBuf> {
    let exts = all_extensions();
    let mut results = Vec::new();

    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        if results.len() >= max_files {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if results.len() >= max_files {
                break;
            }
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !IGNORE_DIRS.contains(&name) && !name.starts_with('.') {
                    dirs.push(path);
                }
            } else if path.is_file()
                && let Some(ext) = path.extension().and_then(|e| e.to_str())
            {
                let ext = format!(".{ext}");
                if exts.iter().any(|e| *e == ext) {
                    results.push(path);
                }
            }
        }
    }

    results
}

/// Read file content, return None on error.
pub fn read_file_safe(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}
