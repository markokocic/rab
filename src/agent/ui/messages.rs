use std::sync::Arc;

use crate::tui::Component;
use crate::tui::Theme;
use crate::tui::components::Text as TuiText;
use crate::tui::components::markdown::{DefaultTextStyle, Markdown, MarkdownTheme};
use crate::tui::util::visible_width;

use super::components::bash_execution::{BashExecution, BashStatus};

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
    /// Bash command execution with styled rendering.
    BashCommand {
        command: String,
        output_lines: Vec<String>,
        status: BashStatus,
        expanded: bool,
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
    let mut lines: Vec<String> = Vec::new();
    let msg_count = messages.len();

    for (idx, msg) in messages.iter().enumerate() {
        match msg {
            DisplayMsg::Separator => {
                lines.push(String::new());
            }
            DisplayMsg::User(text) => {
                let lines_start = lines.len();
                let md_theme = get_md_theme();
                let default_style = DefaultTextStyle {
                    color: Some(Arc::new(|s: &str| {
                        crate::agent::ui::theme::current_theme().fg("userMessageText", s)
                    })),
                    bg_color: None,
                    bold: false,
                    italic: false,
                    strikethrough: false,
                    underline: false,
                };
                let md = Markdown::new(
                    text.clone(),
                    0,
                    0,
                    md_theme,
                    Some(default_style),
                    Some(crate::tui::components::markdown::MarkdownOptions {
                        preserve_ordered_list_markers: true,
                    }),
                );
                let mut msg_box = crate::tui::components::r#box::TuiBox::new(
                    1,
                    1,
                    Some(std::boxed::Box::new(|s: &str| -> String {
                        crate::agent::ui::theme::current_theme().bg("userMessageBg", s)
                    })),
                );
                msg_box.add_child(std::boxed::Box::new(md));
                lines.extend(msg_box.render(width));
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
                if !lines.is_empty() && !lines.last().is_none_or(|l| l.trim().is_empty()) {
                    lines.push(String::new());
                }
                let asst_start = lines.len();
                let md_theme = get_md_theme();
                let md = Markdown::new(text.clone(), 1, 0, md_theme, None, None);
                lines.extend(md.render(width));
                if let Some(first) = lines.get_mut(asst_start) {
                    *first = format!("{}{}", OSC133_ZONE_START, first);
                }
                if let Some(last) = lines.last_mut() {
                    last.push_str(OSC133_ZONE_END);
                    last.push_str(OSC133_ZONE_FINAL);
                }
            }
            DisplayMsg::Thinking { text, level } => {
                if !lines.is_empty() && !lines.last().is_none_or(|l| l.trim().is_empty()) {
                    lines.push(String::new());
                }
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
                    let color_fn = {
                        let lc = level_color.to_string();
                        move |s: &str| -> String {
                            crate::agent::ui::theme::current_theme().fg(&lc, s)
                        }
                    };
                    let default_style = DefaultTextStyle {
                        color: Some(Arc::new(color_fn)),
                        bg_color: None,
                        bold: false,
                        italic: true,
                        strikethrough: false,
                        underline: false,
                    };
                    let md_theme = get_md_theme();
                    let md = Markdown::new(text.clone(), 1, 0, md_theme, Some(default_style), None);
                    let mut md_box = crate::tui::components::r#box::TuiBox::new(
                        1,
                        0,
                        Some(std::boxed::Box::new(|s: &str| -> String {
                            crate::agent::ui::theme::current_theme().bg("thinking_bg", s)
                        })),
                    );
                    md_box.add_child(std::boxed::Box::new(md));
                    lines.extend(md_box.render(width));
                }
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
                let mut msg_box = crate::tui::components::r#box::TuiBox::new(
                    1,
                    1,
                    Some(std::boxed::Box::new(move |s: &str| -> String {
                        crate::agent::ui::theme::current_theme().bg("toolPendingBg", s)
                    })),
                );
                let truncated = if args.len() > 80 {
                    format!("{}…", &args[..80])
                } else {
                    args.clone()
                };
                let styled_name = theme.fg("toolTitle", &theme.bold(name));
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
                    let mut msg_box = crate::tui::components::r#box::TuiBox::new(
                        1,
                        1,
                        Some(std::boxed::Box::new(move |s: &str| -> String {
                            crate::agent::ui::theme::current_theme().bg("toolPendingBg", s)
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
                    let bg_key = if *is_error {
                        "toolErrorBg"
                    } else {
                        "toolSuccessBg"
                    };
                    let fg = if *is_error { "error" } else { "muted" };
                    let mut msg_box = crate::tui::components::r#box::TuiBox::new(
                        1,
                        0,
                        Some(std::boxed::Box::new(move |s: &str| -> String {
                            crate::agent::ui::theme::current_theme().bg(bg_key, s)
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
            DisplayMsg::BashCommand {
                command,
                output_lines,
                status,
                expanded: _,
            } => {
                // Use BashExecution component for styled rendering
                let mut bash = BashExecution::new(command.clone());
                for line in output_lines {
                    bash.append_output(line.clone());
                }
                match status {
                    BashStatus::Running => {}
                    BashStatus::Complete { exit_code } => bash.set_complete(*exit_code),
                    BashStatus::Cancelled => bash.set_cancelled(),
                    BashStatus::Error(msg) => bash.set_error(msg.clone()),
                }
                lines.extend(bash.render(width));
            }
            DisplayMsg::Info(text) => {
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

/// Format token count for compact display (pi style).
/// Get a `MarkdownTheme` from the current RabTheme.
pub fn get_md_theme() -> MarkdownTheme {
    crate::agent::ui::theme::get_markdown_theme()
}

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

    fn current_theme() -> crate::agent::ui::theme::RabTheme {
        crate::agent::ui::theme::current_theme().clone()
    }

    #[test]
    fn test_render_thinking_visible() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = current_theme();
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
        assert!(all.contains("\x1b[3m"), "Should contain italic escape");
        assert!(
            all.contains("\x1b[48;2;"),
            "Should contain thinking background"
        );
    }

    #[test]
    fn test_render_thinking_hidden() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = current_theme();
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
            all.contains("\x1b[48;2;"),
            "Should contain thinking background"
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

    #[test]
    fn test_bash_command_render() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = current_theme();
        let msgs = vec![DisplayMsg::BashCommand {
            command: "echo hello".into(),
            output_lines: vec!["hello".into()],
            status: BashStatus::Complete { exit_code: 0 },
            expanded: false,
        }];
        let lines = render_messages(&msgs, 80, false, false, &theme);
        let all = lines.join("\n");
        assert!(all.contains("echo hello"), "Should show command");
        assert!(all.contains("hello"), "Should show output");
        assert!(all.contains('─'), "Should have borders");
    }

    #[test]
    fn test_bash_command_error_render() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = current_theme();
        let msgs = vec![DisplayMsg::BashCommand {
            command: "false".into(),
            output_lines: vec![],
            status: BashStatus::Complete { exit_code: 1 },
            expanded: false,
        }];
        let lines = render_messages(&msgs, 80, false, false, &theme);
        let all = lines.join("\n");
        assert!(all.contains("false"), "Should show command");
        assert!(all.contains("exit 1"), "Should show exit code");
    }
}
