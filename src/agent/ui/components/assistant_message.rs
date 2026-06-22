use std::cell::RefCell;

use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::components::markdown::Markdown;

const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";

/// Assistant message component - matches pi's AssistantMessageComponent.
/// Renders text content with Markdown, optional thinking blocks.
pub struct AssistantMessageComponent {
    text: String,
    thinking: Vec<ThinkingBlock>,
    hide_thinking: bool,
    cached_lines: RefCell<Option<Vec<String>>>,
    cached_width: RefCell<usize>,
}

pub struct ThinkingBlock {
    pub text: String,
    pub level: Option<String>,
}

impl AssistantMessageComponent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            thinking: Vec::new(),
            hide_thinking: false,
            cached_lines: RefCell::new(None),
            cached_width: RefCell::new(0),
        }
    }

    pub fn set_hide_thinking(&mut self, hide: bool) {
        self.hide_thinking = hide;
        self.invalidate();
    }

    pub fn add_thinking(&mut self, text: impl Into<String>, level: Option<String>) {
        let text = text.into();
        // Some providers (e.g. Ollama) send the FULL accumulated content in each
        // chunk instead of a delta. Detect this: if the new text starts with the
        // existing text, it's a full accumulation — replace instead of append.
        if let Some(last) = self.thinking.last_mut() {
            if text.starts_with(&last.text) && !text.is_empty() {
                last.text = text;
            } else {
                last.text.push_str(&text);
            }
        } else {
            self.thinking.push(ThinkingBlock { text, level });
        }
        self.invalidate();
    }

    pub fn append_text(&mut self, delta: &str) {
        // Some providers (e.g. Ollama) send FULL accumulated content in each chunk
        // instead of a delta. Detect this: if delta starts with existing text,
        // replace instead of append.
        if delta.starts_with(&self.text) && !delta.is_empty() {
            self.text = delta.to_string();
        } else {
            self.text.push_str(delta);
        }
        self.invalidate();
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.invalidate();
    }
}

impl Component for AssistantMessageComponent {
    // No-op: expand/collapse is controlled separately via set_hide_thinking.
    // Pi keeps app.thinking.toggle (Ctrl+T) and app.tools.expand (Ctrl+O)
    // as independent concerns — tool expansion must not affect thinking visibility.

    fn render(&self, width: usize) -> Vec<String> {
        let cached = self.cached_lines.borrow();
        if *self.cached_width.borrow() == width
            && let Some(ref lines) = *cached
        {
            return lines.clone();
        }
        drop(cached);

        let mut lines: Vec<String> = Vec::new();

        // Render thinking blocks first
        for block in &self.thinking {
            if block.text.trim().is_empty() {
                continue;
            }
            if self.hide_thinking {
                let theme = current_theme();
                let label = theme.italic(&theme.fg("thinkingText", " Thinking... "));
                lines.push(label);
            } else {
                let theme = current_theme();
                let color_name = block
                    .level
                    .as_deref()
                    .and_then(|l| match l {
                        "minimal" | "low" => Some("thinking_level_low"),
                        "medium" => Some("thinking_level_medium"),
                        "high" => Some("thinking_level_high"),
                        "xhigh" | "max" => Some("thinking_level_xhigh"),
                        _ => None,
                    })
                    .unwrap_or("thinkingText");

                let color_ansi = theme.fg_ansi(color_name).to_string();
                let italic_style = "\x1b[3m";
                let reset = "\x1b[23m\x1b[39m";

                let styled_text = block
                    .text
                    .lines()
                    .map(|line| format!("{}{}{}{}", italic_style, color_ansi, line, reset))
                    .collect::<Vec<_>>()
                    .join("\n");
                lines.push(styled_text);
            }
        }

        // Add spacer between thinking and main text (one blank line, not one per block)
        let has_thinking =
            !self.thinking.is_empty() && self.thinking.iter().any(|b| !b.text.trim().is_empty());
        let has_text = !self.text.trim().is_empty();
        if has_thinking && has_text {
            lines.push(String::new());
        }

        // Render main text content
        if has_text {
            let md_theme = crate::agent::ui::theme::get_markdown_theme();
            let md = Markdown::new(self.text.clone(), 1, 0, md_theme, None, None);
            let md_lines = md.render(width);
            lines.extend(md_lines);
        }

        // Add OSC133 zones
        if !lines.is_empty() {
            lines[0] = format!("{}{}", OSC133_ZONE_START, &lines[0]);
            if let Some(last) = lines.last_mut() {
                last.push_str(OSC133_ZONE_END);
                last.push_str(OSC133_ZONE_FINAL);
            }
        }

        let result = lines.clone();
        *self.cached_lines.borrow_mut() = Some(lines);
        *self.cached_width.borrow_mut() = width;
        result
    }

    fn invalidate(&mut self) {
        *self.cached_lines.borrow_mut() = None;
    }
}
