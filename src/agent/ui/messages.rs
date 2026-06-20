use crate::tui::Component;
use crate::tui::Theme;
use crate::tui::components::Text as TuiText;
use crate::tui::util::{visible_width, wrap_text_with_ansi};

/// A rendered display message ready for output.
#[derive(Debug, Clone)]
pub enum DisplayMsg {
    User(String),
    AssistantText(String),
    Thinking {
        text: String,
        level: Option<String>,
    },
    ToolCall {
        name: String,
        args: String,
    },
    ToolResult {
        content: String,
        compact: Option<String>,
        is_error: bool,
    },
    Info(String),
    /// Separator between message groups
    Separator,
}

const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";

/// Render messages matching pi's visual design.
pub fn render_messages(
    messages: &[DisplayMsg],
    width: usize,
    hide_thinking: bool,
    collapse_tool_output: bool,
    theme: &dyn Theme,
) -> Vec<String> {
    let inner = width.saturating_sub(2);

    let mut lines: Vec<String> = Vec::new();
    let msg_count = messages.len();

    for (idx, msg) in messages.iter().enumerate() {
        match msg {
            DisplayMsg::Separator => {
                lines.push(String::new());
            }
            DisplayMsg::User(text) => {
                let lines_start = lines.len();
                // Pi: Box(paddingY=1, userMessageBg) → Markdown content
                let mut msg_box = crate::tui::components::r#box::TuiBox::new(
                    1,
                    1,
                    Some(std::boxed::Box::new(move |s: &str| -> String {
                        format!("\x1b[48;2;52;53;65m{}\x1b[49m", s)
                    })),
                );
                // Pi: TuiText handles wrapping - no pre-wrapping needed
                let text_content = text
                    .lines()
                    .map(|l| theme.fg("text", l))
                    .collect::<Vec<_>>()
                    .join("\n");
                let text_content = if text_content.is_empty() {
                    " ".into()
                } else {
                    text_content
                };
                msg_box.add_child(std::boxed::Box::new(TuiText::new(text_content, 0, 0, None)));
                lines.extend(msg_box.render(width));
                // Pi: OSC133 terminal markers around user messages
                if let Some(first) = lines.get_mut(lines_start) {
                    *first = format!("{}{}", OSC133_ZONE_START, first);
                }
                if let Some(last) = lines.last_mut() {
                    last.push_str(OSC133_ZONE_END);
                    last.push_str(OSC133_ZONE_FINAL);
                }
            }
            DisplayMsg::AssistantText(text) => {
                if text.is_empty() {
                    continue;
                }
                // Pi: blank line before assistant text when following non-assistant content (tool result, etc.)
                if !lines.is_empty() && !lines.last().is_none_or(|l| l.trim().is_empty()) {
                    lines.push(String::new());
                }
                let asst_start = lines.len();
                for line in text.lines() {
                    if line.is_empty() {
                        lines.push(String::new());
                    } else {
                        let wrapped = wrap_text_with_ansi(line, inner);
                        for w in wrapped {
                            let line = format!(" {}", w);
                            lines.push(pad_to_width(&line, width));
                        }
                    }
                }
                // Pi: OSC133 terminal markers around assistant messages
                if let Some(first) = lines.get_mut(asst_start) {
                    *first = format!("{}{}", OSC133_ZONE_START, first);
                }
                if let Some(last) = lines.last_mut() {
                    last.push_str(OSC133_ZONE_END);
                    last.push_str(OSC133_ZONE_FINAL);
                }
            }
            DisplayMsg::Thinking { text, level } => {
                // Pi: blank line before thinking when preceded by other content
                if !lines.is_empty() && !lines.last().is_none_or(|l| l.trim().is_empty()) {
                    lines.push(String::new());
                }
                // Pi-style: italic + muted foreground + thinking background
                if hide_thinking {
                    let content = theme.italic(&theme.fg("thinking_text", " Thinking…"));
                    lines.push(theme.bg(
                        "thinking_bg",
                        &pad_to_width(&format!(" {}", content), width),
                    ));
                } else {
                    let level_color = level
                        .as_deref()
                        .and_then(thinking_level_color)
                        .unwrap_or("thinking_text");
                    for line in text.lines() {
                        let content = format!(" {}", theme.italic(&theme.fg(level_color, line)));
                        lines.push(theme.bg("thinking_bg", &pad_to_width(&content, width)));
                    }
                }
                // Pi: blank line after thinking when any visible content follows
                if idx + 1 < msg_count {
                    let has_content = messages[idx + 1..].iter().any(|m| match m {
                        DisplayMsg::AssistantText(t) if !t.is_empty() => true,
                        DisplayMsg::ToolResult { .. } => true,
                        _ => false,
                    });
                    if has_content {
                        lines.push(String::new());
                    }
                }
            }
            DisplayMsg::ToolCall { name, args } => {
                // Pi: Box(paddingY=1, toolPendingBg) → bold name + muted args
                let mut msg_box = crate::tui::components::r#box::TuiBox::new(
                    1,
                    1,
                    Some(std::boxed::Box::new(move |s: &str| -> String {
                        format!("\x1b[48;2;40;40;50m{}\x1b[49m", s)
                    })),
                );
                let truncated = if args.len() > 80 {
                    format!("{}…", &args[..80])
                } else {
                    args.clone()
                };
                let styled_name = theme.bold(name);
                let content = if truncated.is_empty() || truncated == "{}" {
                    styled_name
                } else {
                    format!("{}  {}", styled_name, theme.fg("muted", &truncated))
                };
                let text_content = if content.is_empty() {
                    " ".into()
                } else {
                    content
                };
                msg_box.add_child(std::boxed::Box::new(TuiText::new(text_content, 0, 0, None)));
                lines.extend(msg_box.render(width));
            }
            DisplayMsg::ToolResult {
                content,
                compact,
                is_error,
            } => {
                if let Some(label) = compact {
                    // Pi-style compact mode: show as a tool-call-like line with pending bg
                    // (no expand/collapse yet - that requires per-message state)
                    let mut msg_box = crate::tui::components::r#box::TuiBox::new(
                        1,
                        1,
                        Some(std::boxed::Box::new(move |s: &str| -> String {
                            format!("\x1b[48;2;40;40;50m{}\x1b[49m", s)
                        })),
                    );
                    msg_box.add_child(std::boxed::Box::new(TuiText::new(
                        theme.fg("toolTitle", label),
                        0,
                        0,
                        None,
                    )));
                    lines.extend(msg_box.render(width));
                } else {
                    // Pi: tool result shares the same Box as tool call - TuiBox(paddingY=0)
                    let bg_code = if *is_error { "60;40;40" } else { "40;50;40" };
                    let fg = if *is_error { "error" } else { "muted" };
                    let mut msg_box = crate::tui::components::r#box::TuiBox::new(
                        1,
                        0,
                        Some(std::boxed::Box::new(move |s: &str| -> String {
                            format!("\x1b[48;2;{}m{}\x1b[49m", bg_code, s)
                        })),
                    );
                    if collapse_tool_output {
                        let first_line = content.lines().next().unwrap_or("");
                        let truncated: String = first_line.chars().take(120).collect();
                        let suffix = if first_line.len() > 120 { "…" } else { "" };
                        let c = theme.fg(fg, &format!("{}{}", truncated, suffix));
                        msg_box.add_child(std::boxed::Box::new(TuiText::new(c, 0, 0, None)));
                    } else {
                        let joined = content
                            .lines()
                            .map(|l| {
                                let truncated: String = l.chars().take(140).collect();
                                theme.fg(fg, &truncated)
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        msg_box.add_child(std::boxed::Box::new(TuiText::new(joined, 0, 0, None)));
                    }
                    lines.extend(msg_box.render(width));
                }
            }
            DisplayMsg::Info(text) => {
                // Pi: info messages stack directly, visual separation comes from styling
                for line in text.lines() {
                    let content = theme.fg("dim", &format!(" {}", line));
                    lines.push(pad_to_width(&content, width));
                }
            }
        }
    }

    if lines.is_empty() {
        lines.push(theme.fg("dim", " Type a message and press Enter to send."));
    }

    lines
}

/// Convert session AgentMessages to display messages for the UI.
pub fn session_messages_to_display(
    messages: &[crate::agent::types::AgentMessage],
) -> Vec<DisplayMsg> {
    messages
        .iter()
        .map(|m| match m.role {
            crate::agent::types::Role::User => DisplayMsg::User(m.content.clone()),
            crate::agent::types::Role::Assistant => DisplayMsg::AssistantText(m.content.clone()),
            crate::agent::types::Role::ToolResult => {
                let prefix = if m.is_error { "✗" } else { "✓" };
                DisplayMsg::ToolResult {
                    content: format!("{} {}", prefix, m.content),
                    compact: None,
                    is_error: m.is_error,
                }
            }
        })
        .collect()
}

pub fn pad_to_width(s: &str, width: usize) -> String {
    let vw = visible_width(s);
    if vw > width {
        // Truncate if wider than target - prevents terminal overflow.
        crate::tui::util::truncate_to_width(s, width, "", false)
    } else if vw < width {
        format!("{}{}", s, " ".repeat(width - vw))
    } else {
        s.to_string()
    }
}

/// Map a thinking level string to a theme color name for per-level colors.
/// Pi has 6 levels: off, low, medium, high, xhigh (plus aliases).
pub fn thinking_level_color(level: &str) -> Option<&'static str> {
    match level {
        "off" | "none" => None, // should be hidden, use default
        "minimal" => Some("thinking_level_low"),
        "low" => Some("thinking_level_low"),
        "medium" => Some("thinking_level_medium"),
        "high" => Some("thinking_level_high"),
        "xhigh" | "max" => Some("thinking_level_xhigh"),
        _ => None,
    }
}

/// Format token count for compact display (pi style).
pub fn fmt_tokens(count: f64) -> String {
    if count < 1000.0 {
        format!("{}", count as u64)
    } else if count < 10000.0 {
        format!("{:.1}k", count / 1000.0)
    } else if count < 1_000_000.0 {
        format!("{}k", (count / 1000.0) as u64)
    } else if count < 10_000_000.0 {
        format!("{:.1}M", count / 1_000_000.0)
    } else {
        format!("{}M", (count / 1_000_000.0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ui::theme::RabTheme;

    #[test]
    fn test_render_thinking_visible() {
        let theme = RabTheme;
        let msgs = vec![DisplayMsg::Thinking {
            text: "thinking text".into(),
            level: None,
        }];
        let lines = render_messages(&msgs, 80, false, false, &theme);
        let all = lines.join("\n");
        assert!(
            all.contains("thinking text"),
            "Thinking text should be visible"
        );
        // Should have italic escape
        assert!(all.contains("\x1b[3m"), "Should contain italic escape");
        // Should have thinking background
        assert!(
            all.contains("\x1b[48;2;44;44;54m"),
            "Should contain thinking background"
        );
    }

    #[test]
    fn test_render_thinking_hidden() {
        let theme = RabTheme;
        let msgs = vec![DisplayMsg::Thinking {
            text: "hidden thinking".into(),
            level: None,
        }];
        let lines = render_messages(&msgs, 80, true, false, &theme);
        let all = lines.join("\n");
        assert!(
            !all.contains("hidden thinking"),
            "Thinking text should be hidden"
        );
        assert!(
            all.contains("Thinking…"),
            "Should show ellipsis placeholder"
        );
        assert!(all.contains("\x1b[3m"), "Should contain italic escape");
        assert!(
            all.contains("\x1b[48;2;44;44;54m"),
            "Should contain thinking background"
        );
    }

    #[test]
    fn test_render_thinking_with_level_color() {
        let theme = RabTheme;
        // Each level should map to a distinct ANSI color code
        let levels = [
            (Some("low"), "[38;2;100;100;115m"),
            (Some("medium"), "[38;2;130;130;150m"),
            (Some("high"), "[38;2;160;160;180m"),
            (Some("xhigh"), "[38;2;190;190;210m"),
            (None, "[38;2;128;128;128m"), // default thinking_text
        ];
        for (level, expected_code) in &levels {
            let msg = "level test";
            let msgs = vec![DisplayMsg::Thinking {
                text: msg.into(),
                level: level.map(|s| s.to_string()),
            }];
            let lines = render_messages(&msgs, 80, false, false, &theme);
            let all = lines.join("\n");
            assert!(
                all.contains(expected_code),
                "Level {:?} should use color code {}",
                level,
                expected_code
            );
        }
    }

    #[test]
    fn test_render_thinking_blank_lines() {
        let theme = RabTheme;
        // Thinking followed by assistant text should add blank line between
        let msgs = vec![
            DisplayMsg::Thinking {
                text: "thinking".into(),
                level: None,
            },
            DisplayMsg::AssistantText("response".into()),
        ];
        let lines = render_messages(&msgs, 80, false, false, &theme);
        let all = lines.join("\n");
        assert!(all.contains("thinking"), "Should contain thinking text");
        assert!(all.contains("response"), "Should contain assistant text");
        // Find positions of these markers in the joined string
        let think_pos = all.find("thinking").unwrap();
        let resp_pos = all.find("response").unwrap();
        // The response should appear after the thinking block
        assert!(resp_pos > think_pos, "response should come after thinking");
        // There should be at least one blank line between them
        let between = &all[think_pos + "thinking".len()..resp_pos];
        assert!(
            between.contains("\n\n"),
            "Should have at least one blank line between thinking and assistant. Between: {:?}",
            between
        );
    }

    #[test]
    fn test_thinking_level_color_mapping() {
        assert_eq!(thinking_level_color("off"), None);
        assert_eq!(thinking_level_color("none"), None);
        assert_eq!(thinking_level_color("minimal"), Some("thinking_level_low"));
        assert_eq!(thinking_level_color("low"), Some("thinking_level_low"));
        assert_eq!(
            thinking_level_color("medium"),
            Some("thinking_level_medium")
        );
        assert_eq!(thinking_level_color("high"), Some("thinking_level_high"));
        assert_eq!(thinking_level_color("xhigh"), Some("thinking_level_xhigh"));
        assert_eq!(thinking_level_color("max"), Some("thinking_level_xhigh"));
        assert_eq!(thinking_level_color("unknown"), None);
    }
}
