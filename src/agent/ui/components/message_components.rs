use crate::agent::ui::components::AssistantMessageComponent;
use crate::agent::ui::components::UserMessageComponent;
use crate::agent::ui::components::bash_execution::{BashExecution, BashStatus};
use crate::agent::ui::components::info_message::InfoMessageComponent;
use crate::agent::ui::messages::DisplayMsg;
use crate::agent::ui::theme::{RabTheme, current_theme};
use crate::tui::Component;
use crate::tui::components::Spacer;
use crate::tui::components::Text;
use crate::tui::components::r#box::TuiBox;

/// Convert a single DisplayMsg to a Box<dyn Component> for use in chat_container.
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
            let styled = theme.fg("thinkingText", &theme.italic(text));
            Some(std::boxed::Box::new(Text::new(styled, 0, 0, None)))
        }
        DisplayMsg::ToolCall { name: _, args: _ } => {
            // ToolCall uses ToolExecutionComponent - skip for initial load
            None
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
            let mut msg_box = TuiBox::new(
                1,
                1,
                Some(std::boxed::Box::new(move |s: &str| -> String {
                    format!("{}{}\x1b[49m", bg_ansi, s)
                })),
            );
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
