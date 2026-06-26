/// Pad (or truncate) a string to a given visible width.
pub fn pad_to_width(s: &str, width: usize) -> String {
    let vw = crate::tui::util::visible_width(s);
    if vw > width {
        crate::tui::util::truncate_to_width(s, width, "", false)
    } else if vw < width {
        format!("{}{}", s, " ".repeat(width - vw))
    } else {
        s.to_string()
    }
}

/// Map a thinking level string to a theme color name for per-level colors.
pub fn thinking_level_color(level: &str) -> Option<&'static str> {
    match level {
        "off" | "none" => None,
        "minimal" => Some("thinking_level_low"),
        "low" => Some("thinking_level_low"),
        "medium" => Some("thinking_level_medium"),
        "high" => Some("thinking_level_high"),
        "xhigh" | "max" => Some("thinking_level_xhigh"),
        _ => None,
    }
}
