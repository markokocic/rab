pub mod bash;
pub mod commands;
pub mod edit;
pub mod export;
pub mod extension;
pub mod file_mutation_queue;
pub mod read;
pub mod write;

// Re-export centralized path utilities for backward compatibility.
pub use crate::util::paths::{resolve_path, shorten_path};

use std::path::Path;

/// Format a byte count into a human-readable string (e.g. "1.5KB", "2.0MB").
pub(crate) fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
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
