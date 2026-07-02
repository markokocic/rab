//! Centralized path utilities for rab.
//!
//! Provides safe path handling across platforms, including:
//! - Canonicalization with Windows `\\?\` prefix stripping
//! - Path resolution (relative/absolute, `~` expansion)
//! - Display formatting (relative to cwd or home)
//!
//! Mirrors pi's `packages/coding-agent/src/utils/paths.ts`.

use std::path::{Path, PathBuf};

/// Canonicalize a path, stripping the Windows `\\?\` verbatim prefix if present.
///
/// On Windows, `std::fs::canonicalize` returns paths prefixed with `\\?\` (the
/// extended-length path prefix). This is technically correct but makes paths
/// harder to read and breaks string-based prefix matching (e.g., for relative
/// path display). This wrapper strips that prefix so paths remain portable.
///
/// Falls back to the raw path if canonicalization fails (e.g. the target does
/// not exist yet), matching pi's `canonicalizePath` behaviour.
pub fn canonicalize(path: &Path) -> PathBuf {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    strip_windows_verbatim(&canon)
}

/// Resolve a path (relative or absolute) against a working directory.
/// Expands `~` to the user's home directory.
pub fn resolve_path(path: &str, cwd: &Path) -> PathBuf {
    let expanded = if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix('~')) {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/"));
        if rest.is_empty() {
            home
        } else {
            home.join(rest.strip_prefix('/').unwrap_or(rest))
        }
    } else {
        Path::new(path).to_path_buf()
    };

    if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(&expanded)
    }
}

/// Shorten a path by replacing home directory with `~`.
pub fn shorten_path(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        path.replacen(&home, "~", 1)
    } else {
        path.to_string()
    }
}

/// Resolve, then canonicalize a path. Best for turning user-supplied paths
/// into clean, absolute, symlink-resolved paths suitable for comparison.
pub fn resolve_and_canonicalize(path: &str, cwd: &Path) -> PathBuf {
    canonicalize(&resolve_path(path, cwd))
}

/// Convert a `Path` to a `String`, stripping the Windows `\\?\` prefix.
/// Falls back to an empty string on non-UTF-8 paths.
pub fn path_to_string(path: &Path) -> String {
    let s = path.to_string_lossy();
    strip_verbatim_prefix(s.as_ref()).to_string()
}

/// Format a path for human-readable display, relative to cwd or home.
///
/// Order of preference:
/// 1. Relative to cwd  → `./relative/path`
/// 2. Relative to home → `~/relative/path`
/// 3. Raw absolute path
///
/// Both `path` and `cwd` should already be canonicalized (or at least
/// consistently normalized) so prefix matching works reliably across platforms.
pub fn format_for_display(path: &Path, cwd: &Path) -> String {
    let path_str = path_to_string(path);

    // Try relative to cwd first
    if let Some(rel) = strip_prefix_str(&path_str, &path_to_string(cwd)) {
        if rel.is_empty() {
            return path_str;
        }
        return format!("./{}", rel.trim_start_matches('/'));
    }

    // Try relative to home
    if let Ok(home) = std::env::var("HOME")
        && let Some(rel) = strip_prefix_str(&path_str, &home)
    {
        if rel.is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", rel.trim_start_matches('/'));
    }

    path_str
}

/// Format a path for display with an explicit home override (used in footer).
pub fn format_for_display_with_home(path: &Path, cwd: &Path, home: &str) -> String {
    let path_str = path_to_string(path);
    let cwd_str = path_to_string(cwd);

    // Try relative to cwd first
    if let Some(rel) = strip_prefix_str(&path_str, &cwd_str) {
        if rel.is_empty() {
            return path_str;
        }
        return format!("./{}", rel.trim_start_matches('/'));
    }

    // Try relative to home
    if let Some(rel) = strip_prefix_str(&path_str, home) {
        if rel.is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", rel.trim_start_matches('/'));
    }

    path_str
}

// ── Internal helpers ─────────────────────────────────────────────

/// Strip the Windows `\\?\` verbatim prefix from a path.
#[inline]
fn strip_windows_verbatim(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    PathBuf::from(strip_verbatim_prefix(s.as_ref()))
}

/// Strip `\\?\` prefix from a string path on Windows; no-op elsewhere.
fn strip_verbatim_prefix(s: &str) -> &str {
    #[cfg(windows)]
    {
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return rest;
        }
    }
    s
}

/// Strip a prefix from a path string, ensuring we don't leave a bare `\\`
/// or mismatch on case (Windows is case-insensitive).
fn strip_prefix_str<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    #[cfg(windows)]
    {
        // Case-insensitive comparison on Windows
        if path.len() < prefix.len() {
            return None;
        }
        let path_prefix = &path[..prefix.len()];
        if !path_prefix.eq_ignore_ascii_case(prefix) {
            return None;
        }
        // Also strip a leading separator if present
        let rest = &path[prefix.len()..];
        if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') {
            return Some(rest);
        }
        None
    }
    #[cfg(not(windows))]
    {
        path.strip_prefix(prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_path_absolute() {
        let cwd = Path::new("/some/dir");
        let result = resolve_path("/tmp/foo", cwd);
        assert_eq!(result, Path::new("/tmp/foo"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let cwd = Path::new("/some/dir");
        let result = resolve_path("foo/bar", cwd);
        assert_eq!(result, Path::new("/some/dir/foo/bar"));
    }

    #[test]
    fn test_resolve_path_tilde() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let cwd = Path::new("/some/dir");
        let result = resolve_path("~/foo", cwd);
        assert_eq!(result, Path::new(&format!("{}/foo", home)));
    }

    #[test]
    fn test_resolve_path_tilde_only() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let cwd = Path::new("/some/dir");
        let result = resolve_path("~", cwd);
        assert_eq!(result, Path::new(&home));
    }

    #[test]
    fn test_shorten_path() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let result = shorten_path(&format!("{}/foo/bar", home));
        assert_eq!(result, "~/foo/bar");
    }

    #[test]
    fn test_shorten_path_no_match() {
        let result = shorten_path("/tmp/foo");
        assert_eq!(result, "/tmp/foo");
    }

    #[test]
    fn test_strip_verbatim_prefix_normal() {
        // On non-Windows, this is a no-op
        let path = Path::new("/foo/bar");
        let result = strip_windows_verbatim(path);
        assert_eq!(result, Path::new("/foo/bar"));
    }

    #[test]
    fn test_format_for_display_relative_to_cwd() {
        let path = Path::new("/project/sub/AGENTS.md");
        let cwd = Path::new("/project");
        let result = format_for_display(path, cwd);
        assert_eq!(result, "./sub/AGENTS.md");
    }

    #[test]
    fn test_format_for_display_relative_to_home() {
        let path = Path::new("/home/user/project/AGENTS.md");
        let cwd = Path::new("/other");
        let result = format_for_display(path, cwd);
        // If HOME is set, it should show ~/project/AGENTS.md
        if let Ok(home) = std::env::var("HOME") {
            if home == "/home/user" {
                assert_eq!(result, "~/project/AGENTS.md");
            }
        }
    }

    #[test]
    fn test_format_for_display_absolute_fallback() {
        let path = Path::new("/some/other/path");
        let cwd = Path::new("/project");
        let result = format_for_display(path, cwd);
        assert_eq!(result, "/some/other/path");
    }

    #[test]
    fn test_format_for_display_same_as_cwd() {
        let path = Path::new("/project");
        let cwd = Path::new("/project");
        let result = format_for_display(path, cwd);
        assert_eq!(result, "/project");
    }

    #[test]
    fn test_path_to_string() {
        let path = Path::new("/foo/bar");
        assert_eq!(path_to_string(path), "/foo/bar");
    }

    #[test]
    fn test_resolve_and_canonicalize_relative() {
        let cwd = Path::new("/");
        // /tmp should exist on most systems
        let result = resolve_and_canonicalize("tmp", cwd);
        assert_eq!(result, Path::new("/tmp"));
    }
}
