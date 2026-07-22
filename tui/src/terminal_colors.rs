//! Terminal color scheme detection — parses OSC 11 responses and terminal
//! color scheme reports (matching pi's `packages/tui/src/terminal-colors.ts`).

/// An RGB color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Detected terminal color scheme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TerminalColorScheme {
    Dark,
    Light,
}

/// Check if the given data looks like an OSC 11 background color response.
/// Pattern: `\x1b]11;...\x07` or `\x1b]11;...\x1b\\`
pub fn is_osc11_background_color_response(data: &str) -> bool {
    if !data.starts_with("\x1b]11;") {
        return false;
    }
    data.ends_with('\x07') || data.ends_with("\x1b\\")
}

/// Parse an OSC 11 background color response into an `RgbColor`.
/// Supports formats:
/// - `\x1b]11;rgb:RRRR/GGGG/BBBB\x07`
/// - `\x1b]11;#RRGGBB\x07`
/// - `\x1b]11;#RRRRGGGGBBBB\x07`
pub fn parse_osc11_background_color(data: &str) -> Option<RgbColor> {
    // Extract content between \x1b]11; and terminator (\x07 or \x1b\\)
    let value = data
        .strip_prefix("\x1b]11;")?
        .trim_end_matches(['\x07', '\\'])
        .trim_end_matches('\x1b')
        .trim();

    // #RRGGBB or #RRRRGGGGBBBB
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex_color(hex);
    }

    // rgb:RRRR/GGGG/BBBB or rgba:RRRR/GGGG/BBBB
    let rgb_value = value
        .strip_prefix("rgb:")
        .or_else(|| value.strip_prefix("rgba:"))?;
    let parts: Vec<&str> = rgb_value.split('/').collect();
    if parts.len() < 3 {
        return None;
    }
    let r = parse_osc_hex_channel(parts[0])?;
    let g = parse_osc_hex_channel(parts[1])?;
    let b = parse_osc_hex_channel(parts[2])?;
    Some(RgbColor { r, g, b })
}

/// Parse a terminal color scheme report.
/// Response format: `\x1b[?997;1n` (dark) or `\x1b[?997;2n` (light)
pub fn parse_terminal_color_scheme_report(data: &str) -> Option<TerminalColorScheme> {
    let data = data.trim_end_matches('n');
    if data == "\x1b[?997;2" {
        Some(TerminalColorScheme::Light)
    } else if data == "\x1b[?997;1" {
        Some(TerminalColorScheme::Dark)
    } else {
        None
    }
}

/// Parse a hex color string (6 or 12 hex digits, with or without #).
fn parse_hex_color(hex: &str) -> Option<RgbColor> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(RgbColor { r, g, b })
        }
        12 => {
            let r = parse_osc_hex_channel(&hex[0..4])?;
            let g = parse_osc_hex_channel(&hex[4..8])?;
            let b = parse_osc_hex_channel(&hex[8..12])?;
            Some(RgbColor { r, g, b })
        }
        _ => None,
    }
}

/// Parse a single 16-bit OSC hex channel value (e.g. "FFFF" → 255).
fn parse_osc_hex_channel(channel: &str) -> Option<u8> {
    if channel.is_empty() || !channel.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let max = 16usize.pow(channel.len() as u32).saturating_sub(1);
    if max == 0 {
        return None;
    }
    let val = usize::from_str_radix(channel, 16).ok()?;
    Some((val * 255 / max) as u8)
}

/// Determine color scheme from a background color (luminance-based).
/// Pi uses this: if luminance > 0.5, it's light; otherwise dark.
pub fn color_scheme_from_background(bg: &RgbColor) -> TerminalColorScheme {
    // Relative luminance formula (sRGB)
    let lum = 0.2126 * (bg.r as f64 / 255.0)
        + 0.7152 * (bg.g as f64 / 255.0)
        + 0.0722 * (bg.b as f64 / 255.0);
    if lum > 0.5 {
        TerminalColorScheme::Light
    } else {
        TerminalColorScheme::Dark
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rgb_format() {
        let data = "\x1b]11;rgb:1111/2222/3333\x07";
        let color = parse_osc11_background_color(data).unwrap();
        assert_eq!(color.r, 17);
        assert_eq!(color.g, 34);
        assert_eq!(color.b, 51);
    }

    #[test]
    fn test_parse_hex_6() {
        let data = "\x1b]11;#ff8800\x07";
        let color = parse_osc11_background_color(data).unwrap();
        assert_eq!(color.r, 255);
        assert_eq!(color.g, 136);
        assert_eq!(color.b, 0);
    }

    #[test]
    fn test_parse_hex_12() {
        let data = "\x1b]11;#ffff88880000\x07";
        let color = parse_osc11_background_color(data).unwrap();
        assert_eq!(color.r, 255);
        assert_eq!(color.g, 136);
        assert_eq!(color.b, 0);
    }

    #[test]
    fn test_parse_st_instead_of_bel() {
        let data = "\x1b]11;rgb:1111/2222/3333\x1b\\";
        assert!(is_osc11_background_color_response(data));
        let color = parse_osc11_background_color(data).unwrap();
        assert_eq!(color.r, 17);
    }

    #[test]
    fn test_is_osc11_response() {
        assert!(is_osc11_background_color_response("\x1b]11;rgb:1/2/3\x07"));
        assert!(!is_osc11_background_color_response("hello"));
        assert!(!is_osc11_background_color_response("\x1b[?997;1n"));
    }

    #[test]
    fn test_parse_color_scheme_dark() {
        let scheme = parse_terminal_color_scheme_report("\x1b[?997;1n");
        assert_eq!(scheme, Some(TerminalColorScheme::Dark));
    }

    #[test]
    fn test_parse_color_scheme_light() {
        let scheme = parse_terminal_color_scheme_report("\x1b[?997;2n");
        assert_eq!(scheme, Some(TerminalColorScheme::Light));
    }

    #[test]
    fn test_color_scheme_from_background() {
        let dark = RgbColor { r: 0, g: 0, b: 0 };
        let light = RgbColor {
            r: 255,
            g: 255,
            b: 255,
        };
        assert_eq!(
            color_scheme_from_background(&dark),
            TerminalColorScheme::Dark
        );
        assert_eq!(
            color_scheme_from_background(&light),
            TerminalColorScheme::Light
        );
    }

    #[test]
    fn test_invalid_data() {
        assert!(parse_osc11_background_color("garbage").is_none());
        assert!(parse_osc11_background_color("\x1b]11;bad\x07").is_none());
    }
}
