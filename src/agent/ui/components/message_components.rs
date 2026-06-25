use crate::agent::ui::components::AssistantMessageComponent;
use crate::agent::ui::components::UserMessageComponent;
use crate::agent::ui::components::bash_execution::{BashExecution, BashStatus};
use crate::agent::ui::components::info_message::InfoMessageComponent;
use crate::agent::ui::messages::DisplayMsg;
use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::{RabTheme, current_theme};
use crate::tui::Component;
use crate::tui::components::Spacer;
use crate::tui::components::Text;
use crate::tui::components::r#box::TuiBox;
/// Convert a single DisplayMsg to a `Box<dyn Component>` for use in chat_container.
/// This is used for initial session message loading.
/// New messages during the session should be added as Components directly in handle_agent_event.
pub fn display_msg_to_component(msg: &DisplayMsg) -> Option<std::boxed::Box<dyn Component>> {
    // Clone the theme so we can use it in closures without borrow issues
    let theme: RabTheme = current_theme().clone();
    match msg {
        DisplayMsg::User(text) => Some(std::boxed::Box::new(UserMessageComponent::new(
            text.clone(),
        ))),
        DisplayMsg::AssistantText(text) => {
            if text.is_empty() {
                return None;
            }
            Some(std::boxed::Box::new(AssistantMessageComponent::new(
                text.clone(),
            )))
        }
        DisplayMsg::Thinking { text, level: _ } => {
            let styled = theme.fg_key(ThemeKey::ThinkingText, &theme.italic(text));
            Some(std::boxed::Box::new(Text::new(styled, 0, 0, None)))
        }
        DisplayMsg::ToolCall { name, args } => {
            let theme: RabTheme = current_theme().clone();
            if name == "bash"
                && let Ok(val) = serde_json::from_str::<serde_json::Value>(args)
            {
                let cmd = val.get("command").and_then(|v| v.as_str()).unwrap_or("");
                let timeout = val.get("timeout").and_then(|v| v.as_i64());
                let timeout_suffix = timeout
                    .map(|t| theme.fg_key(ThemeKey::Muted, &format!(" (timeout {}s)", t)))
                    .unwrap_or_default();
                let content = format!(
                    "{}{}",
                    theme.fg("toolTitle", &theme.bold(&format!("$ {}", cmd))),
                    timeout_suffix
                );
                let bg_ansi = theme.bg_ansi_key(ThemeKey::ToolPendingBg).to_string();
                let mut msg_box = TuiBox::new(1, 1, Some(crate::tui::Style::new().bg(bg_ansi)));
                msg_box.add_child(std::boxed::Box::new(Text::new(content, 0, 0, None)));
                return Some(std::boxed::Box::new(msg_box));
            }
            // Generic tool call header
            let bg_ansi = theme.bg_ansi_key(ThemeKey::ToolPendingBg).to_string();
            let truncated = if args.len() > 80 {
                format!("{}…", &args[..80])
            } else {
                args.clone()
            };
            let content = if truncated.is_empty() || truncated == "{}" {
                theme.fg("toolTitle", &theme.bold(name))
            } else {
                format!(
                    "{}  {}",
                    theme.fg("toolTitle", &theme.bold(name)),
                    theme.fg_key(ThemeKey::Muted, &truncated)
                )
            };
            let mut msg_box = TuiBox::new(1, 1, Some(crate::tui::Style::new().bg(bg_ansi)));
            msg_box.add_child(std::boxed::Box::new(Text::new(content, 0, 0, None)));
            Some(std::boxed::Box::new(msg_box))
        }
        DisplayMsg::ToolResult {
            content,
            compact: _,
            is_error,
        } => {
            let color = if *is_error { "error" } else { "toolOutput" };
            let styled = theme.fg(color, content);
            let bg_key = if *is_error {
                "toolErrorBg"
            } else {
                "toolSuccessBg"
            };
            let bg_ansi = theme.bg_ansi(bg_key).to_string();
            let mut msg_box = TuiBox::new(1, 1, Some(crate::tui::Style::new().bg(bg_ansi)));
            msg_box.add_child(std::boxed::Box::new(Text::new(styled, 0, 0, None)));
            Some(std::boxed::Box::new(msg_box))
        }
        DisplayMsg::BashCommand {
            command,
            output_lines,
            status,
            expanded: _,
        } => {
            let mut bash = BashExecution::new(command.clone());
            for line in output_lines {
                bash.append_output(line.clone());
            }
            match status {
                BashStatus::Running => {}
                BashStatus::Complete { exit_code } => {
                    bash.set_complete(*exit_code);
                }
                BashStatus::Cancelled => {
                    bash.set_cancelled();
                }
                BashStatus::Error(msg) => {
                    bash.set_error(msg.clone());
                }
            }
            Some(std::boxed::Box::new(bash))
        }
        DisplayMsg::Info(text) => Some(std::boxed::Box::new(InfoMessageComponent::new(text))),
        DisplayMsg::Separator => Some(std::boxed::Box::new(Spacer::new(1))),
    }
}
