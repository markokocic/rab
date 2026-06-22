/// Image handling utilities.
///
/// Provides:
/// - Image format detection from file extensions
/// - Data URL creation (data:image/...;base64,...)
/// - Kitty image protocol sequence generation for terminal display
/// - Detection of data URL lines in rendered output
use base64::Engine;

/// Image file extensions we support.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"];

/// MIME type mapping for image formats.
fn mime_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// Check if a file extension corresponds to a supported image format.
pub fn is_image_extension(ext: &str) -> bool {
    let ext = ext.trim_start_matches('.');
    IMAGE_EXTENSIONS
        .iter()
        .any(|&e| ext.eq_ignore_ascii_case(e))
}

/// Check if a file path is a supported image file.
pub fn is_image_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_image_extension)
}

/// Read an image file and return it as a data URL string.
/// Format: `data:image/png;base64,iVBORw0KGgo...`
pub fn file_to_data_url(path: &std::path::Path) -> Result<String, std::io::Error> {
    let bytes = std::fs::read(path)?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
    Ok(bytes_to_data_url(&bytes, ext))
}

/// Convert raw image bytes to a data URL string.
pub fn bytes_to_data_url(bytes: &[u8], ext: &str) -> String {
    let mime = mime_type(ext);
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{};base64,{}", mime, b64)
}

/// Check if a string is a data URL for an image.
/// Matches `data:image/...;base64,...`
pub fn is_data_url(s: &str) -> bool {
    s.trim().starts_with("data:image/") && s.contains(";base64,")
}

/// Extract the MIME type from a data URL.
/// e.g. `data:image/png;base64,...` → `image/png`
pub fn extract_mime_from_data_url(data_url: &str) -> Option<&str> {
    let s = data_url.trim();
    if s.starts_with("data:")
        && let Some(end) = s.find(';')
    {
        Some(&s[5..end])
    } else {
        None
    }
}

/// Generate a Kitty image protocol sequence for an image.
///
/// Format:
/// ```text
/// ESC_Ga=T,f=<format>,m=0ESC\<base64 data>ESC\\
/// ```
///
/// This renders the image inline in terminals that support the Kitty protocol
/// (kitty, WezTerm, iTerm2, Konsole, etc.).
///
/// For simplicity, we send the entire base64 data in one chunk (`m=0`).
/// The `f=100` format means RGBA (24-bit RGB), `a=T` means transmit and display.
pub fn kitty_image_sequence(data_url: &str) -> String {
    let data_url = data_url.trim();
    if !is_data_url(data_url) {
        return String::new();
    }

    // Extract base64 data (after the comma)
    let b64_data = if let Some(comma_pos) = data_url.find(',') {
        &data_url[comma_pos + 1..]
    } else {
        return String::new();
    };

    // Determine format code from MIME type
    let mime = extract_mime_from_data_url(data_url).unwrap_or("image/png");
    let format = match mime {
        "image/png" => 100,  // PNG
        "image/jpeg" => 101, // JPEG
        "image/gif" => 102,  // GIF
        "image/webp" => 103, // WebP
        _ => 100,            // Default to PNG
    };

    // Kitty protocol: a=T (transmit), f=<format>, m=0 (single chunk)
    format!(
        "\x1b_Ga=T,f={},m=0\x1b\\{}\x1b\\\\",
        format,
        b64_data.trim()
    )
}

/// Generate a Kitty protocol sequence to place a previously transmitted image.
/// This can be used to show an image that was already sent.
pub fn kitty_place_image(placement_id: &str) -> String {
    format!("\x1b_Ga=p,i={},m=0\x1b\\\\", placement_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_image_extension() {
        assert!(is_image_extension("png"));
        assert!(is_image_extension("jpg"));
        assert!(is_image_extension("jpeg"));
        assert!(is_image_extension("gif"));
        assert!(is_image_extension("webp"));
        assert!(!is_image_extension("txt"));
        assert!(!is_image_extension("rs"));
        assert!(!is_image_extension("md"));
    }

    #[test]
    fn test_is_image_extension_with_dot() {
        assert!(is_image_extension(".png"));
        assert!(is_image_extension(".jpg"));
    }

    #[test]
    fn test_is_image_extension_case_insensitive() {
        assert!(is_image_extension("PNG"));
        assert!(is_image_extension("JPG"));
        assert!(is_image_extension("WebP"));
    }

    #[test]
    fn test_mime_type() {
        assert_eq!(mime_type("png"), "image/png");
        assert_eq!(mime_type("jpg"), "image/jpeg");
        assert_eq!(mime_type("jpeg"), "image/jpeg");
        assert_eq!(mime_type("gif"), "image/gif");
        assert_eq!(mime_type("webp"), "image/webp");
    }

    #[test]
    fn test_bytes_to_data_url() {
        let bytes = [0x89, 0x50, 0x4E, 0x47]; // PNG magic bytes
        let url = bytes_to_data_url(&bytes, "png");
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(!url.ends_with(","));
        // The base64 of [0x89, 0x50, 0x4E, 0x47]
        assert!(url.contains("iVBOR"));
    }

    #[test]
    fn test_is_data_url() {
        assert!(is_data_url("data:image/png;base64,iVBORw0KGgo="));
        assert!(!is_data_url("data:text/plain;base64,hello"));
        assert!(!is_data_url("hello world"));
    }

    #[test]
    fn test_extract_mime_from_data_url() {
        assert_eq!(
            extract_mime_from_data_url("data:image/png;base64,iVBOR"),
            Some("image/png")
        );
        assert_eq!(
            extract_mime_from_data_url("data:image/jpeg;base64,/9j/4"),
            Some("image/jpeg")
        );
    }

    #[test]
    fn test_kitty_image_sequence_format() {
        let url = "data:image/png;base64,iVBORw0KGgo=";
        let seq = kitty_image_sequence(url);
        assert!(seq.starts_with("\x1b_Ga=T"));
        assert!(seq.contains("f=100")); // PNG format code
        assert!(seq.contains("m=0"));
        assert!(seq.contains("iVBORw0KGgo"));
        assert!(seq.ends_with("\x1b\\\\"));
    }

    #[test]
    fn test_kitty_image_sequence_jpeg() {
        let url = "data:image/jpeg;base64,/9j/4AAQ";
        let seq = kitty_image_sequence(url);
        assert!(seq.contains("f=101")); // JPEG format code
    }

    #[test]
    fn test_kitty_image_sequence_invalid() {
        assert_eq!(kitty_image_sequence("not a data url"), "");
    }
}
