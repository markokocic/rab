use crate::tui::Theme;
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

    for msg in messages {
        match msg {
            DisplayMsg::Separator => {
                lines.push(String::new());
            }
            DisplayMsg::User(text) => {
                // Pi: blank line before user messages when there's already content in the chat
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                // Pi: blue-grey background, 1px padding, userMessageText foreground.
                for line in text.lines() {
                    let wrapped = wrap_text_with_ansi(line, inner.saturating_sub(2));
                    for w in wrapped {
                        let content = theme.fg("text", &format!("  {}", w));
                        let padded = pad_to_width(&content, width);
                        lines.push(theme.bg("user_message_bg", &padded));
                    }
                }
            }
            DisplayMsg::AssistantText(text) => {
                if text.is_empty() {
                    continue;
                }
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
            }
            DisplayMsg::Thinking(text) => {
                // Pi-style: italic + muted foreground on regular background, NO special bg
                if hide_thinking {
                    let content = theme.italic(&theme.fg("thinking_text", " Thinking…"));
                    let padded = pad_to_width(&format!(" {}", content), width);
                    lines.push(padded);
                    continue;
                }
                for line in text.lines() {
                    let content = format!(" {}", theme.italic(&theme.fg("thinking_text", line)));
                    lines.push(pad_to_width(&content, width));
                }
            }
            DisplayMsg::ToolCall { name, args } => {
                // Pi: tool calls are Box(paddingY=1) — blank line before
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                // Pi-style: bold tool name (title color) + muted args on toolPendingBg
                let truncated = if args.len() > 80 {
                    format!("{}…", &args[..80])
                } else {
                    args.clone()
                };
                let styled_name = theme.bold(name);
                let content = if truncated.is_empty() || truncated == "{}" {
                    format!(" {} ", styled_name)
                } else {
                    format!(" {}  {}", styled_name, theme.fg("muted", &truncated))
                };
                let padded = pad_to_width(&content, width);
                lines.push(theme.bg("tool_pending_bg", &padded));
            }
            DisplayMsg::ToolResult { content, is_error } => {
                // Pi: tool result is part of the same Box as the tool call — no extra blank line
                let bg = if *is_error {
                    "tool_error_bg"
                } else {
                    "tool_success_bg"
                };
                // Pi uses toolOutput color (muted/gray) for result content, error fg for errors
                let fg = if *is_error { "error" } else { "muted" };

                if collapse_tool_output {
                    let first_line = content.lines().next().unwrap_or("");
                    let truncated: String = first_line.chars().take(120).collect();
                    let suffix = if first_line.len() > 120 { "…" } else { "" };
                    let content = theme.fg(fg, &format!(" {}{}", truncated, suffix));
                    let padded = pad_to_width(&content, width);
                    lines.push(theme.bg(bg, &padded));
                } else {
                    for line_content in content.lines() {
                        let truncated: String = line_content.chars().take(140).collect();
                        let content = theme.fg(fg, &format!(" {}", truncated));
                        let padded = pad_to_width(&content, width);
                        lines.push(theme.bg(bg, &padded));
                    }
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
