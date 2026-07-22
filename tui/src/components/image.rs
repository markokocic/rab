use crate::Component;

/// Options for Image component rendering.
pub struct ImageOptions {
    /// Maximum width in terminal cells (columns).
    pub max_width_cells: usize,
    /// Explicit image width in pixels. If not provided, extracted from image data.
    pub width_px: Option<u32>,
    /// Explicit image height in pixels. If not provided, extracted from image data.
    pub height_px: Option<u32>,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            max_width_cells: 60,
            width_px: None,
            height_px: None,
        }
    }
}

/// An inline image rendered via the Kitty terminal image protocol.
///
/// Falls back to a text summary when images are not supported by the terminal.
/// Matches pi's `Image` component in `packages/tui/src/components/image.ts`.
pub struct Image {
    /// Base64-encoded image data.
    base64_data: String,
    /// MIME type (e.g. "image/png", "image/jpeg").
    mime_type: String,
    /// Image dimensions in pixels.
    width_px: u32,
    height_px: u32,
    /// Rendering options.
    options: ImageOptions,
    /// Cached render output.
    cached_lines: Option<Vec<String>>,
    /// Cached width at which cache is valid.
    cached_width: Option<usize>,
}

impl Image {
    pub fn new(
        base64_data: impl Into<String>,
        mime_type: impl Into<String>,
        options: ImageOptions,
    ) -> Self {
        let base64_data = base64_data.into();
        let mime_type = mime_type.into();

        // Try to extract dimensions from image data
        let (width_px, height_px) = if let Some(dims) = extract_dimensions(&base64_data, &mime_type)
        {
            dims
        } else {
            (
                options.width_px.unwrap_or(800),
                options.height_px.unwrap_or(600),
            )
        };

        Self {
            base64_data,
            mime_type,
            width_px,
            height_px,
            options,
            cached_lines: None,
            cached_width: None,
        }
    }
}

impl Component for Image {
    fn render(&mut self, width: usize) -> Vec<String> {
        if let Some(ref lines) = self.cached_lines
            && self.cached_width == Some(width)
        {
            return lines.clone();
        }

        let supports_images = crate::components::markdown::kitty_images_supported();
        let max_width = (width.saturating_sub(2))
            .min(self.options.max_width_cells)
            .max(1);

        let lines: Vec<String> = if supports_images {
            // Decode base64 and generate Kitty sequence
            use base64::Engine as _;
            let engine = base64::engine::general_purpose::STANDARD;
            if let Ok(data) = engine.decode(&self.base64_data) {
                let seq = crate::components::markdown::kitty_image_sequence(&data, &self.mime_type);

                // Calculate how many terminal rows the image occupies
                // Based on cell dimensions (assumed ~8x16px if unknown)
                let cell_w = 8.0;
                let cell_h = 16.0;
                let cols_needed = (self.width_px as f64 / cell_w).ceil() as usize;
                let scale = if cols_needed > max_width {
                    max_width as f64 / cols_needed as f64
                } else {
                    1.0
                };
                let rows = ((self.height_px as f64 * scale) / cell_h).ceil() as usize;
                let rows = rows.max(1);

                let mut result = vec![seq];
                // Additional empty lines account for the image height
                for _ in 1..rows {
                    result.push(String::new());
                }
                result
            } else {
                // Base64 decode failed — fallback
                vec![fallback_text(
                    &self.mime_type,
                    self.width_px,
                    self.height_px,
                )]
            }
        } else {
            // No image support — text fallback
            vec![fallback_text(
                &self.mime_type,
                self.width_px,
                self.height_px,
            )]
        };

        self.cached_lines = Some(lines.clone());
        self.cached_width = Some(width);
        lines
    }

    fn invalidate(&mut self) {
        self.cached_lines = None;
        self.cached_width = None;
    }
}

/// Generate a fallback text description for when images can't be rendered.
fn fallback_text(mime_type: &str, width_px: u32, height_px: u32) -> String {
    let type_label = match mime_type {
        "image/png" => "[PNG]",
        "image/jpeg" | "image/jpg" => "[JPEG]",
        "image/gif" => "[GIF]",
        "image/webp" => "[WebP]",
        _ => "[Image]",
    };
    format!("{} {}x{}px", type_label, width_px, height_px)
}

/// Extract image dimensions from base64-encoded data by parsing image headers.
/// Currently supports PNG. Returns `(width, height)` in pixels.
fn extract_dimensions(base64_data: &str, mime_type: &str) -> Option<(u32, u32)> {
    match mime_type {
        "image/png" => extract_png_dimensions(base64_data),
        _ => None,
    }
}

/// Parse PNG IHDR chunk to extract dimensions.
/// PNG format: 8-byte signature, then IHDR chunk (13 bytes: width(4) + height(4) + ...)
fn extract_png_dimensions(base64_data: &str) -> Option<(u32, u32)> {
    use base64::Engine as _;
    let engine = base64::engine::general_purpose::STANDARD;
    let data = engine.decode(base64_data).ok()?;

    // PNG signature: 8 bytes
    // IHDR chunk: 4 bytes length + 4 bytes "IHDR" + 13 bytes data + 4 bytes CRC
    if data.len() < 33 {
        return None;
    }
    // Verify PNG signature
    if data[0..8] != [137, 80, 78, 71, 13, 10, 26, 10] {
        return None;
    }
    // Check that the first chunk is IHDR
    if &data[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid PNG (1x1 pixel, white).
    /// Generated from: python3 -c "import struct,zlib; ...
    fn minimal_png_base64() -> String {
        // Base64 of a minimal 1x1 white PNG
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==".to_string()
    }

    #[test]
    fn test_png_dimension_extraction() {
        let b64 = minimal_png_base64();
        let dims = extract_png_dimensions(&b64);
        assert!(dims.is_some());
        assert_eq!(dims.unwrap(), (1, 1));
    }

    #[test]
    fn test_fallback_text() {
        let text = fallback_text("image/png", 800, 600);
        assert!(text.contains("[PNG]"));
        assert!(text.contains("800x600"));
    }

    #[test]
    fn test_image_render_no_support() {
        let mut img = Image::new(minimal_png_base64(), "image/png", ImageOptions::default());
        // Without terminal support, should render fallback text
        let lines = img.render(80);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("[PNG]"));
    }
}
