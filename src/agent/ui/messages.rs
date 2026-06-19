use crate::agent::ui::theme::RabTheme;
use crate::tui::Theme;
use crate::tui::util::{visible_width, wrap_text_with_ansi};

/// A rendered display message ready for output.
#[derive(Debug, Clone)]
pub enum DisplayMsg {
    User(String),
    AssistantText(String),
    Thinking(String),
    ToolCall { name: String, args: String },
    ToolResult { content: String, is_error: bool },
    Info(String),
}

/// Render conversation messages into styled lines for the terminal.
pub fn render_messages(
    messages: &[DisplayMsg],
    width: usize,
    hide_thinking: bool,
    collapse_tool_output: bool,
    theme: &RabTheme,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let inner_width = width.saturating_sub(2); // 1 char padding each side

    for msg in messages {
        match msg {
            DisplayMsg::User(text) => {
                if !lines.is_empty() {
                    lines.push(String::new()); // blank line before user message
                }
                for line in text.lines() {
                    let styled = theme.user_msg_bg(&format!(" {}", line));
                    lines.push(pad_to_width(&styled, width));
                }
            }
            DisplayMsg::AssistantText(text) => {
                for line in text.lines() {
                    if line.is_empty() {
                        lines.push(String::new());
                    } else {
                        let wrapped = wrap_text_with_ansi(line, inner_width);
                        for wline in wrapped {
                            lines.push(format!(" {}", wline));
                        }
                    }
                }
            }
            DisplayMsg::Thinking(text) => {
                if hide_thinking {
                    lines.push(format!(" {}", theme.thinking_bg(" Thinking…")));
                    continue;
                }
                for line in text.lines() {
                    let styled = theme.thinking_bg(&format!(" {}", line));
                    lines.push(pad_to_width(&styled, width));
                }
            }
            DisplayMsg::ToolCall { name, args } => {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                let truncated = if args.len() > 80 {
                    format!("{}…", &args[..80])
                } else {
                    args.clone()
                };
                let line_text = if truncated == "{}" || truncated.is_empty() {
                    format!(" {} ", name)
                } else {
                    format!(" {}  {}", name, truncated)
                };
                let styled = theme.bg("tool_pending_bg", &line_text);
                lines.push(pad_to_width(&styled, width));
            }
            DisplayMsg::ToolResult {
                content, is_error, ..
            } => {
                let bg = if *is_error {
                    "tool_error_bg"
                } else {
                    "tool_success_bg"
                };
                if collapse_tool_output {
                    let first = content.lines().next().unwrap_or("");
                    let truncated: String = first.chars().take(120).collect();
                    let suffix = if first.len() > 120 { "…" } else { "" };
                    let styled = theme.bg(bg, &format!(" {}{}", truncated, suffix));
                    lines.push(pad_to_width(&styled, width));
                } else {
                    for line_content in content.lines() {
                        let truncated: String = line_content.chars().take(140).collect();
                        let styled = theme.bg(bg, &format!(" {}", truncated));
                        lines.push(pad_to_width(&styled, width));
                    }
                }
            }
            DisplayMsg::Info(text) => {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                let styled = theme.dim(text);
                lines.push(pad_to_width(&styled, width));
            }
        }
    }

    if lines.is_empty() {
        lines.push(theme.dim("Type a message and press Enter to send."));
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

/// Pad a string to a given terminal width.
fn pad_to_width(s: &str, width: usize) -> String {
    let vw = visible_width(s);
    if vw >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - vw))
    }
}

/// Format token count for display.
pub fn fmt_tokens(n: i32) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 10000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else if n < 1_000_000 {
        format!("{}k", n / 1000)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}
