use std::cell::RefCell;

use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::components::r#box::TuiBox;
use crate::tui::components::markdown::{DefaultTextStyle, Markdown, MarkdownOptions};

const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";

/// User message component — matches pi's UserMessageComponent.
/// Renders text in a Box with `userMessageBg` background, `userMessageText` color.
pub struct UserMessageComponent {
    box_component: TuiBox,
    cached_lines: RefCell<Option<Vec<String>>>,
    cached_width: RefCell<usize>,
}

impl UserMessageComponent {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let theme = current_theme();
        let bg_ansi = theme.bg_ansi("userMessageBg").to_string();
        drop(theme);

        let mut msg_box = TuiBox::new(
            1,
            1,
            Some(std::boxed::Box::new(move |s: &str| -> String {
                format!("{}{}\x1b[49m", bg_ansi, s)
            })),
        );

        // Build the markdown renderer with userMessageText color
        let md_theme = crate::agent::ui::theme::get_markdown_theme();
        let default_style = DefaultTextStyle {
            color: Some(std::sync::Arc::new(|s: &str| -> String {
                let t = current_theme();
                t.fg("userMessageText", s)
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
            Some(MarkdownOptions {
                preserve_ordered_list_markers: true,
            }),
        );
        msg_box.add_child(std::boxed::Box::new(md));

        Self {
            box_component: msg_box,
            cached_lines: RefCell::new(None),
            cached_width: RefCell::new(0),
        }
    }
}

impl Component for UserMessageComponent {
    fn set_expanded(&mut self, _expanded: bool) {
        // User messages are always fully visible
    }

    fn render(&self, width: usize) -> Vec<String> {
        if *self.cached_width.borrow() == width
            && let Some(ref lines) = *self.cached_lines.borrow()
        {
            return lines.clone();
        }

        let mut lines = self.box_component.render(width);
        if !lines.is_empty() {
            lines[0] = format!("{}{}", OSC133_ZONE_START, &lines[0]);
            if let Some(last) = lines.last_mut() {
                last.push_str(OSC133_ZONE_END);
                last.push_str(OSC133_ZONE_FINAL);
            }
        }

        // Cache
        let result = lines.clone();
        *self.cached_lines.borrow_mut() = Some(lines);
        *self.cached_width.borrow_mut() = width;
        result
    }

    fn invalidate(&mut self) {
        *self.cached_lines.borrow_mut() = None;
        self.box_component.invalidate();
    }
}
