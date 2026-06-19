use super::app::{App, TuiConfig, collect_commands};
use crate::extension::SlashCommand;
use crate::types::AgentMessage;
use ratatui::text::{Line, Span, Text};

// ── Display messages ───────────────────────────────────────────────

#[derive(Debug)]
pub(crate) enum DisplayMsg {
    User(String),
    AssistantText(String),
    Thinking(String),
    ToolCall { name: String, args: String },
    ToolResult { content: String, is_error: bool },
    Info(String),
}

pub(crate) fn welcome_messages(config: &TuiConfig) -> Vec<DisplayMsg> {
    let model_display = config.model.replace("opencode_go::", "");
    let cwd_str = config.cwd.to_str().unwrap_or("?");
    let tool_names: Vec<String> = config.tools.iter().map(|t| t.name.clone()).collect();

    // Collect slash commands from all extensions
    let commands: Vec<SlashCommand> = config
        .extensions
        .iter()
        .flat_map(|e| e.commands())
        .collect();
    let cmd_names: Vec<String> = commands.iter().map(|c| format!("/{}", c.name)).collect();

    let mut msgs = Vec::new();
    msgs.push(DisplayMsg::Info(format!(
        "rab · model {model_display} · {cwd_str}"
    )));
    msgs.push(DisplayMsg::Info(format!(
        "Tools: {}",
        tool_names.join(", ")
    )));
    if !cmd_names.is_empty() {
        msgs.push(DisplayMsg::Info(format!(
            "Commands: {}",
            cmd_names.join(", ")
        )));
    }
    msgs.push(DisplayMsg::Info(
        "Enter  submit · Ctrl+C  interrupt/clear · Ctrl+D  quit · F1  help · Ctrl+L  model · Ctrl+T  thinking · Ctrl+O  tools\n\
         Ctrl+J  newline · Esc  clear/abort · ↑↓  history · !  bash · Tab  complete"
            .to_string(),
    ));
    msgs
}

/// Convert session AgentMessages to display messages for the TUI.

pub(crate) fn session_messages_to_display(messages: &[AgentMessage]) -> Vec<DisplayMsg> {
    messages
        .iter()
        .map(|m| match m.role {
            crate::types::Role::User => DisplayMsg::User(m.content.clone()),
            crate::types::Role::Assistant => DisplayMsg::AssistantText(m.content.clone()),
            crate::types::Role::ToolResult => {
                let prefix = if m.is_error { "✗" } else { "✓" };
                DisplayMsg::ToolResult {
                    content: format!("{} {}", prefix, m.content),
                    is_error: m.is_error,
                }
            }
        })
        .collect()
}

pub(crate) fn build_message_text(app: &App) -> Text<'static> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    if app.show_help {
        lines.extend(help_lines(app));
        return Text::from(lines);
    }

    let th = &app.theme;

    for msg in &app.messages {
        match msg {
            DisplayMsg::User(text) => {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                for line in text.lines() {
                    lines.push(
                        Line::from(Span::styled(format!(" {line}"), th.user_msg_style()))
                            .style(th.user_msg_style()),
                    );
                }
            }
            DisplayMsg::AssistantText(text) => {
                for line in text.lines() {
                    if line.is_empty() {
                        lines.push(Line::from(""));
                    } else {
                        lines.push(Line::from(line.to_string()));
                    }
                }
            }
            DisplayMsg::Thinking(text) => {
                if app.hide_thinking {
                    if !lines.is_empty()
                        && !lines.last().is_none_or(|l| {
                            l.spans.is_empty() || l.spans.iter().all(|s| s.content.is_empty())
                        })
                    {
                        lines.push(Line::from(""));
                    }
                    lines.push(
                        Line::from(Span::styled(" Thinking…", th.thinking_label_style()))
                            .style(th.thinking_label_style()),
                    );
                    continue;
                }
                for line in text.lines() {
                    lines.push(
                        Line::from(Span::styled(format!(" {line}"), th.thinking_style()))
                            .style(th.thinking_style()),
                    );
                }
            }
            DisplayMsg::ToolCall { name, args, .. } => {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                let truncated = if args.len() > 80 {
                    format!("{}…", &args[..80])
                } else {
                    args.clone()
                };
                let line_text = if truncated == "{}" || truncated.is_empty() {
                    format!(" {name} ")
                } else {
                    format!(" {name}  {truncated}")
                };
                lines.push(
                    Line::from(Span::styled(line_text, th.tool_pending_style()))
                        .style(th.tool_pending_style()),
                );
            }
            DisplayMsg::ToolResult {
                content, is_error, ..
            } => {
                let style = if *is_error {
                    th.tool_error_style()
                } else {
                    th.tool_success_style()
                };
                if app.tool_output_collapsed {
                    let first = content.lines().next().unwrap_or("");
                    let truncated: String = first.chars().take(120).collect();
                    let suffix = if first.len() > 120 { "…" } else { "" };
                    lines.push(
                        Line::from(Span::styled(format!(" {truncated}{suffix}"), style))
                            .style(style),
                    );
                } else {
                    for line_content in content.lines() {
                        let truncated: String = line_content.chars().take(140).collect();
                        lines.push(
                            Line::from(Span::styled(format!(" {truncated}"), style)).style(style),
                        );
                    }
                }
            }
            DisplayMsg::Info(text) => {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(text.clone(), th.dim_style())));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Type a message and press Enter to send.",
            th.dim_style(),
        )));
    }

    Text::from(lines)
}

pub(crate) fn help_lines(app: &App) -> Vec<Line<'static>> {
    let th = &app.theme;
    let dim = th.dim_style();
    let accent = th.accent_style();

    let mut lines = vec![
        Line::from(Span::styled("Keyboard Shortcuts", accent)),
        Line::from(""),
        Line::from(Span::styled("  Enter              Submit message", dim)),
        Line::from(Span::styled("  Ctrl+J             Newline", dim)),
        Line::from(Span::styled(
            "  Ctrl+C             Interrupt / clear editor",
            dim,
        )),
        Line::from(Span::styled(
            "  Ctrl+D             Quit (empty) / interrupt",
            dim,
        )),
        Line::from(Span::styled("  Escape             Clear editor", dim)),
        Line::from(Span::styled(
            "  Ctrl+L             Open model selector",
            dim,
        )),
        Line::from(Span::styled("  !<command>         Run bash inline", dim)),
        Line::from(Span::styled(
            "  !!<command>        Run bash (excluded from context)",
            dim,
        )),
        Line::from(Span::styled("  Ctrl+T             Toggle thinking", dim)),
        Line::from(Span::styled("  Ctrl+O             Toggle tool output", dim)),
        Line::from(Span::styled("  F1                 Show this help", dim)),
        Line::from(Span::styled(
            "  ↑↓                 History (editor empty)",
            dim,
        )),
        Line::from(Span::styled("  PgUp / PgDn        Page scroll", dim)),
        Line::from(Span::styled("  Mouse wheel        Scroll", dim)),
    ];

    // List slash commands from extensions
    let commands = collect_commands(app);
    if !commands.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Slash Commands", accent)));
        lines.push(Line::from(""));
        for cmd in &commands {
            lines.push(Line::from(Span::styled(
                format!("  /{:<20} {}", cmd.name, cmd.description),
                dim,
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press any key to close help.",
        dim,
    )));
    lines
}

pub(crate) fn fmt_tokens(n: i32) -> String {
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

pub(crate) fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

pub(crate) fn pad_right(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{}{}", " ".repeat(width - len), s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Role;

    fn make_msg(role: Role, content: &str, is_error: bool) -> AgentMessage {
        let is_tool = role == Role::ToolResult;
        AgentMessage {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role,
            content: content.to_string(),
            tool_calls: vec![],
            tool_call_id: if is_tool {
                Some("tc1".to_string())
            } else {
                None
            },
            usage: None,
            is_error,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}
