use crate::tui::Component;
use crate::tui::Theme;
use crate::tui::components::Text as TuiText;
use crate::tui::util::{visible_width, wrap_text_with_ansi};

/// A rendered display message ready for output.
#[derive(Debug, Clone)]
pub enum DisplayMsg {
    User(String),
    AssistantText(String),
    Thinking(String),
    ToolCall {
        name: String,
        args: String,
    },
    ToolResult {
        content: String,
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
        // Pi: track whether visible content follows (for thinking block spacing)
        let has_visible_after = |start: usize| -> bool {
            messages[start..]
                .iter()
                .any(|m| matches!(m, DisplayMsg::AssistantText(t) if !t.is_empty()))
        };
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
                let joined = text
                    .lines()
                    .flat_map(|l| wrap_text_with_ansi(l, inner.saturating_sub(2)))
                    .fold(String::new(), |mut acc, line| {
                        if !acc.is_empty() {
                            acc.push('\n');
                        }
                        acc.push_str(&theme.fg("text", &line));
                        acc
                    });
                let text_content = if joined.is_empty() {
                    " ".into()
                } else {
                    joined
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
            DisplayMsg::Thinking(text) => {
                // Pi-style: italic + muted foreground, no background
                if hide_thinking {
                    let content = theme.italic(&theme.fg("thinking_text", " Thinking…"));
                    let padded = pad_to_width(&format!(" {}", content), width);
                    lines.push(padded);
                } else {
                    for line in text.lines() {
                        let content =
                            format!(" {}", theme.italic(&theme.fg("thinking_text", line)));
                        lines.push(pad_to_width(&content, width));
                    }
                }
                // Pi: Spacer after thinking if visible content follows
                if idx + 1 < msg_count && has_visible_after(idx + 1) {
                    lines.push(String::new());
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
            DisplayMsg::ToolResult { content, is_error } => {
                // Pi: tool result uses same Box pattern with success/error background
                let bg_code = if *is_error { "60;40;40" } else { "40;50;40" };
                let fg = if *is_error { "error" } else { "muted" };
                let mut msg_box = crate::tui::components::r#box::TuiBox::new(
                    1,
                    1,
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
                    is_error: m.is_error,
                }
            }
        })
        .collect()
}

pub fn pad_to_width(s: &str, width: usize) -> String {
    let vw = visible_width(s);
    if vw > width {
        // Truncate if wider than target — prevents terminal overflow.
        crate::tui::util::truncate_to_width(s, width, "", false)
    } else if vw < width {
        format!("{}{}", s, " ".repeat(width - vw))
    } else {
        s.to_string()
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
