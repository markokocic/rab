pub mod bash;
pub mod commands;
pub mod edit;
pub mod file_mutation_queue;
pub mod read;
pub mod write;

use std::path::{Path, PathBuf};

/// Resolve a path (relative or absolute) against a working directory.
/// Expands `~` to the user's home directory.
pub fn resolve_path(path: &str, cwd: &Path) -> PathBuf {
    // Expand ~ prefix to home directory
    let expanded = if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~")) {
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

/// Wrap a styled path string in an OSC 8 hyperlink if the terminal supports it.
/// The `raw_path` is resolved against `cwd` to produce the file:// URL.
pub fn link_path(styled_text: &str, raw_path: &str, cwd: &Path) -> String {
    if !crate::tui::components::markdown::hyperlinks_supported() {
        return styled_text.to_string();
    }
    let abs_path = resolve_path(raw_path, cwd);
    let url = urlencoding(abs_path.to_string_lossy().as_ref());
    crate::tui::components::markdown::hyperlink(styled_text, &format!("file://{}", url))
}

/// Decode a base64-encoded string to raw bytes.
pub fn base64_decode(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(input)
}

/// URL-encode a file path for use in a file:// URL.
fn urlencoding(path: &str) -> String {
    let mut result = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_path_absolute() {
        let cwd = Path::new("/home/user");
        let result = resolve_path("/tmp/foo", cwd);
        assert_eq!(result, Path::new("/tmp/foo"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let cwd = Path::new("/home/user");
        let result = resolve_path("foo/bar", cwd);
        assert_eq!(result, Path::new("/home/user/foo/bar"));
    }

    #[test]
    fn test_resolve_path_tilde() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let cwd = Path::new("/tmp");
        let result = resolve_path("~/foo", cwd);
        assert_eq!(result, Path::new(&format!("{}/foo", home)));
    }

    #[test]
    fn test_resolve_path_tilde_only() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let cwd = Path::new("/tmp");
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
    fn test_urlencoding() {
        let result = urlencoding("/home/user/file.txt");
        assert_eq!(result, "/home/user/file.txt");
    }

    #[test]
    fn test_urlencoding_spaces() {
        let result = urlencoding("/home/user/my file.txt");
        assert_eq!(result, "/home/user/my%20file.txt");
    }
}
